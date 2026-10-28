//! Implementation behind the commands in `send.rs`: converting between the core,
//! engine and IPC types, spooling response bodies and tracking in-flight sends.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::{HeaderMap, HeaderName, HeaderValue, Method};
use ironcall_core::content;
use ironcall_core::model::{HttpVersion, RequestFile};
use ironcall_core::recent::{RecentSends, rfc3339_utc};
use ironcall_core::redact;
use ironcall_core::url::UrlError;
use ironcall_core::wire::{self, BuildError, DerivedKind, PartContent, WireBody, WireContext, WireRequest};
use ironcall_http::{
    Body, CancellationToken, Engine, ErrorKind, HttpVersionPref, Part, Phase, Progress, ResponseHead, Stage,
    Timings,
};
use ironcall_secrets::SecretString;
use ironcall_store::body::{BodyId, BodyStore};
use ironcall_store::spool::{Spool, SpoolEntry};
use tauri_specta::Event;
use tokio::io::AsyncWriteExt;

use crate::network::NetOptions;
use crate::send::{
    BodyKind, CertificateInfo, HeaderRow, HeaderSource, InformationalResponse, PreviewHeader, RecentSend,
    RedirectHop, RequestPreview, ResponseBodyInfo, ResponseSummary, SMALL_BODY_LIMIT, SendError,
    SendErrorKind, SendProgress, SendStage, TimelineEntry, TimelineKind, TimingPhase, TimingsInfo, TlsInfo,
};

/// A request on its way out: the resolved file, its build context and the wire form.
struct Outgoing<'a> {
    request: &'a RequestFile,
    ctx: &'a WireContext,
    wire: &'a WireRequest,
    /// Secret values to mask in the timeline.
    secrets: &'a [SecretString],
}

/// What we remember about each spooled response body.
#[derive(Clone, Debug)]
struct BodyMeta {
    kind: content::BodyKind,
    stem: String,
    /// The response's `Content-Type` media type, without parameters.
    media_type: Option<String>,
}

/// How a stored body may be served to the webview (see `protocol.rs`).
#[derive(Clone, Debug)]
pub struct ServeInfo {
    pub kind: content::BodyKind,
    pub media_type: Option<String>,
}

/// Keeps a send registered for cancelling until the send starts (see [`Sender::begin`]).
pub struct Reservation<'a> {
    sender: &'a Sender,
    send_id: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.sender
            .reserved
            .lock()
            .expect("reserved lock poisoned")
            .remove(&self.send_id);
    }
}

/// Everything sending needs, shared through `AppState`.
pub struct Sender {
    engine: Engine,
    spool: Spool,
    bodies: Arc<BodyStore>,
    in_flight: Mutex<HashMap<String, CancellationToken>>,
    /// Tokens registered by [`Sender::begin`] before the send itself starts (while
    /// variables and secrets are resolved), so an early cancel isn't lost.
    reserved: Mutex<HashMap<String, CancellationToken>>,
    recent: RecentSends<RecentSend>,
    body_meta: Mutex<HashMap<BodyId, BodyMeta>>,
}

impl Sender {
    pub fn new(engine: Engine, spool: Spool, bodies: Arc<BodyStore>) -> Self {
        Self {
            engine,
            spool,
            bodies,
            in_flight: Mutex::new(HashMap::new()),
            reserved: Mutex::new(HashMap::new()),
            recent: RecentSends::new(),
            body_meta: Mutex::new(HashMap::new()),
        }
    }

    pub fn recent(&self) -> Vec<RecentSend> {
        self.recent.list()
    }

    /// Registers `send_id` before the send starts (a child of `parent`, e.g. a run's
    /// token), so [`Sender::cancel`] works from the first moment. The reservation is
    /// dropped with the guard if the send never starts.
    pub fn begin(&self, send_id: &str, parent: Option<&CancellationToken>) -> Reservation<'_> {
        let token = parent.map_or_else(CancellationToken::new, CancellationToken::child_token);
        self.reserved
            .lock()
            .expect("reserved lock poisoned")
            .insert(send_id.to_string(), token);
        Reservation {
            sender: self,
            send_id: send_id.to_string(),
        }
    }

    pub fn cancel(&self, send_id: &str) {
        let token = self
            .in_flight
            .lock()
            .expect("in-flight lock poisoned")
            .get(send_id)
            .cloned()
            .or_else(|| {
                self.reserved
                    .lock()
                    .expect("reserved lock poisoned")
                    .get(send_id)
                    .cloned()
            });
        if let Some(token) = token {
            token.cancel();
        }
    }

    /// Deletes all spooled bodies (called on exit).
    pub fn clear_spool(&self) {
        let _ = self.spool.clear();
    }

    /// Closes a body and deletes its spooled files.
    pub fn close_body(&self, id: BodyId) {
        self.bodies.close(id);
        let meta = self
            .body_meta
            .lock()
            .expect("body meta lock poisoned")
            .remove(&id);
        if let Some(meta) = meta {
            self.spool.remove_entry(&meta.stem);
        }
    }

    /// `request` must already have its variables resolved; `secrets` are the values
    /// substituted, masked in the preview unless `reveal`.
    pub fn preview(
        &self,
        request: &RequestFile,
        ctx: &WireContext,
        net: Option<&NetOptions>,
        secrets: &[SecretString],
        reveal: bool,
    ) -> Result<RequestPreview, SendError> {
        let wire = wire::build(request, ctx).map_err(build_error)?;
        let engine_req = to_engine_request(&wire, net, None)?;
        let preview = preview_of(request, ctx, &wire, &engine_req);
        Ok(if reveal {
            preview
        } else {
            mask_preview(preview, secrets)
        })
    }

    /// What the body is, for serving it over the custom schemes. `None` for bodies that
    /// aren't responses (e.g. files opened with `open_body`).
    pub fn serve_info(&self, id: BodyId) -> Option<ServeInfo> {
        self.body_meta
            .lock()
            .expect("body meta lock poisoned")
            .get(&id)
            .map(|m| ServeInfo {
                kind: m.kind,
                media_type: m.media_type.clone(),
            })
    }

    /// The first `cap` bytes of a body, and whether it was longer.
    pub fn body_prefix(&self, id: BodyId, cap: u64) -> Option<(Vec<u8>, bool)> {
        let body = self.bodies.get(id)?;
        let len = body.index().byte_len();
        let bytes = body.read_range(0, len.min(cap)).ok()?;
        Some((bytes, len > cap))
    }

    pub fn body_kind(&self, id: BodyId) -> Option<content::BodyKind> {
        self.body_meta
            .lock()
            .expect("body meta lock poisoned")
            .get(&id)
            .map(|m| m.kind)
    }

    /// The decoded bytes as received (before charset conversion), for "Save as".
    pub fn raw_path(&self, id: BodyId) -> Option<std::path::PathBuf> {
        let stem = self
            .body_meta
            .lock()
            .expect("body meta lock poisoned")
            .get(&id)?
            .stem
            .clone();
        let raw = self.spool.path_of(&stem, "raw");
        // Derived and history bodies have only their text file.
        Some(if raw.exists() {
            raw
        } else {
            self.spool.path_of(&stem, "txt")
        })
    }

    /// Writes a new spooled body with `write` and registers it like a response body.
    fn new_body(
        &self,
        kind: content::BodyKind,
        media_type: Option<String>,
        write: impl FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>,
    ) -> std::io::Result<(BodyId, u64, usize)> {
        let stem = format!("d-{}", ulid::Ulid::generate());
        let (entry, evicted) = self.spool.create_entry(&stem)?;
        self.evict(&evicted);
        let mut file = std::io::BufWriter::new(entry.create("txt")?);
        let written = write(&mut file).and_then(|()| std::io::Write::flush(&mut file));
        drop(file);
        if let Err(e) = written {
            self.spool.remove_entry(&stem);
            return Err(e);
        }
        let (id, body) = self.bodies.open(&entry.path("txt"))?;
        self.body_meta.lock().expect("body meta lock poisoned").insert(
            id,
            BodyMeta {
                kind,
                stem,
                media_type,
            },
        );
        Ok((id, body.index().byte_len(), body.index().row_count()))
    }

    /// Pretty-prints a JSON or XML body of any size into a new body (blocking; streams).
    pub fn pretty_body(&self, id: BodyId) -> Result<(BodyId, u64, usize), String> {
        let body = self.bodies.get(id).ok_or("unknown body")?;
        let (kind, media_type) = {
            let meta = self.body_meta.lock().expect("body meta lock poisoned");
            let m = meta.get(&id).ok_or("unknown body")?;
            (m.kind, m.media_type.clone())
        };
        if !kind.can_pretty_print() {
            return Err("only JSON and XML bodies can be pretty-printed".into());
        }
        let source = std::fs::File::open(body.path()).map_err(|e| e.to_string())?;
        let mut failure = None;
        let result = self.new_body(kind, media_type, |out| {
            content::pretty_stream(kind, std::io::BufReader::new(source), out)
                .map(|_| ())
                .map_err(|e| {
                    let message = e.to_string();
                    failure = Some(message.clone());
                    std::io::Error::other(message)
                })
        });
        result.map_err(|e| failure.unwrap_or_else(|| e.to_string()))
    }

    /// Registers bytes (e.g. a body stored in history) as a viewable body.
    pub fn open_stored_body(
        &self,
        bytes: &[u8],
        kind: content::BodyKind,
        media_type: Option<String>,
    ) -> std::io::Result<(BodyId, u64, usize)> {
        self.new_body(kind, media_type, |out| std::io::Write::write_all(out, bytes))
    }

    /// Reads a small body's text, pretty-printed if asked and supported.
    pub fn body_text(&self, id: BodyId, pretty: bool) -> Result<String, String> {
        let body = self.bodies.get(id).ok_or("unknown body")?;
        let len = body.index().byte_len();
        if len > SMALL_BODY_LIMIT {
            return Err(format!("body is {len} bytes; use the paged viewer"));
        }
        let bytes = body.read_range(0, len).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let kind = self
            .body_meta
            .lock()
            .expect("body meta lock poisoned")
            .get(&id)
            .map(|m| m.kind);
        Ok(match kind {
            Some(kind) if pretty => content::pretty(kind, &text).unwrap_or(text),
            _ => text,
        })
    }

    pub async fn send<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        send_id: String,
        request: RequestFile,
        ctx: &WireContext,
        net: NetOptions,
        secrets: Vec<SecretString>,
    ) -> Result<ResponseSummary, SendError> {
        let wire = wire::build(&request, ctx).map_err(|e| {
            let mut err = build_error(e);
            // URL errors quote the resolved URL, which may hold secrets.
            err.message = redact::scrub(&redact::mask_values(&err.message, &secrets));
            err
        })?;
        let token = self
            .reserved
            .lock()
            .expect("reserved lock poisoned")
            .remove(&send_id)
            .unwrap_or_default();
        if token.is_cancelled() {
            return Err(SendError::new(SendErrorKind::Cancelled, "Request cancelled"));
        }
        {
            let mut in_flight = self.in_flight.lock().expect("in-flight lock poisoned");
            if in_flight.contains_key(&send_id) {
                return Err(SendError::new(
                    SendErrorKind::InvalidRequest,
                    "a send with this ID is already running",
                ));
            }
            in_flight.insert(send_id.clone(), token.clone());
        }
        self.recent.push(RecentSend {
            send_id: send_id.clone(),
            at: rfc3339_utc(SystemTime::now()),
            method: wire.method.clone(),
            url: redact::redact_url_masking(&wire.url, &secrets),
            status: None,
            error: None,
            total_ms: None,
        });

        let result = self
            .send_inner(
                app,
                &send_id,
                Outgoing {
                    request: &request,
                    ctx,
                    wire: &wire,
                    secrets: &secrets,
                },
                &net,
                token,
            )
            .await
            // The summary goes to the UI and may be kept; never include secret values.
            .map(|mut summary| {
                summary.request = mask_preview(summary.request, &secrets);
                for hop in &mut summary.redirects {
                    hop.url = redact::redact_url_masking(&hop.url, &secrets);
                }
                summary
            })
            // Errors quote URLs and `Location`s (e.g. a resolved URL with an API key);
            // they're shown and stored in history.
            .map_err(|mut err| {
                err.message = redact::scrub(&redact::mask_values(&err.message, &secrets));
                err.detail = err
                    .detail
                    .map(|d| redact::scrub(&redact::mask_values(&d, &secrets)));
                err
            });

        self.in_flight
            .lock()
            .expect("in-flight lock poisoned")
            .remove(&send_id);
        self.recent.update(
            |e| e.send_id == send_id,
            |e| match &result {
                Ok(summary) => {
                    e.status = Some(summary.status);
                    e.total_ms = Some(summary.timings.total_ms);
                }
                Err(err) => e.error = Some(err.kind),
            },
        );
        result
    }

    async fn send_inner<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        send_id: &str,
        out: Outgoing<'_>,
        net: &NetOptions,
        token: CancellationToken,
    ) -> Result<ResponseSummary, SendError> {
        let Outgoing {
            request,
            ctx,
            wire,
            secrets,
        } = out;
        let mut engine_req = to_engine_request(wire, Some(net), Some(token))?;
        // A redirect to another origin must not carry credentials: the sensitive headers,
        // and whatever header an API key auth uses.
        engine_req.options.strip_on_cross_origin = redact::sensitive_header_names(&net.sensitive_headers);
        if let ironcall_core::model::Auth::ApiKey {
            key,
            location: ironcall_core::model::ApiKeyLocation::Header,
            ..
        } = &ctx.auth
        {
            engine_req
                .options
                .strip_on_cross_origin
                .push(key.trim().to_string());
        }
        let preview = preview_of(request, ctx, wire, &engine_req);

        let (entry, evicted) = self.spool.create_entry(send_id).map_err(io_error)?;
        self.evict(&evicted);

        let progress_app = app.clone();
        let progress_id = send_id.to_string();
        let progress = move |p: Progress| {
            let stage = match p.stage {
                Stage::Connecting => SendStage::Connecting,
                Stage::Uploading => SendStage::Uploading,
                Stage::Waiting => SendStage::Waiting,
                Stage::Downloading => SendStage::Downloading,
            };
            let _ = SendProgress {
                send_id: progress_id.clone(),
                stage,
                bytes: p.bytes,
                uploaded: p.uploaded,
                upload_total: p.upload_total,
            }
            .emit(&progress_app);
        };

        let file = entry.create("raw").map_err(io_error)?;
        let mut sink = tokio::io::BufWriter::new(tokio::fs::File::from_std(file));
        let sent = self.engine.send_to_traced(engine_req, &mut sink, &progress).await;
        let flushed = sink.shutdown().await;
        let head = match sent {
            Ok(head) => head,
            Err(failure) => {
                self.spool.remove_entry(send_id);
                let mut err = engine_error(&failure.error);
                err.timeline = timeline(&failure.timeline, secrets, &net.sensitive_headers);
                return Err(err);
            }
        };
        flushed.map_err(io_error)?;

        let body = self.store_body(entry, &head, &wire.method).await?;
        Ok(ResponseSummary {
            send_id: send_id.to_string(),
            status: head.status.as_u16(),
            status_text: head.reason.clone(),
            http_version: head.protocol.as_str().to_string(),
            headers: header_rows(&head.headers),
            body,
            timings: timings_info(&head.timings, Some(head.total)),
            redirects: head
                .redirects
                .iter()
                .map(|hop| RedirectHop {
                    status: hop.status.as_u16(),
                    url: hop.url.clone(),
                    timings: timings_info(&hop.timings, None),
                })
                .collect(),
            request: RequestPreview {
                url: head.final_url.clone(),
                ..preview
            },
            connection_reused: head.connection_reused,
            tls: head.tls.as_ref().map(tls_info),
            informational: head
                .informational
                .iter()
                .map(|i| InformationalResponse {
                    status: i.status.as_u16(),
                    headers: header_rows(&i.headers),
                    at_ms: ms(i.after_send),
                })
                .collect(),
            timeline: timeline(&head.timeline, secrets, &net.sensitive_headers),
            history_id: None,
            checks: None,
        })
    }

    /// Classifies the spooled body, converts text to UTF-8 and registers it in the body
    /// store.
    async fn store_body(
        &self,
        entry: SpoolEntry,
        head: &ResponseHead,
        method: &str,
    ) -> Result<ResponseBodyInfo, SendError> {
        let content_type = head
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let status = head.status.as_u16();
        let method = method.to_string();
        let wire_size = head.wire_len;
        let bodies = self.bodies.clone();

        let (info, meta) = tauri::async_runtime::spawn_blocking(move || -> std::io::Result<_> {
            let raw = entry.path("raw");
            let mut sniff = Vec::with_capacity(content::SNIFF_LEN);
            std::fs::File::open(&raw)?
                .take(content::SNIFF_LEN as u64)
                .read_to_end(&mut sniff)?;
            let kind = content::classify(content_type.as_deref(), &sniff, status, &method);

            let mut info = ResponseBodyInfo {
                body_id: None,
                kind: body_kind(kind),
                content_type: content_type.clone(),
                charset: None,
                charset_lossy: false,
                size: 0,
                wire_size,
                row_count: 0,
                pretty_available: false,
            };
            if kind == content::BodyKind::Empty {
                return Ok((info, None));
            }

            let path = if kind == content::BodyKind::Binary {
                raw
            } else {
                let charset = content::detect_charset(content_type.as_deref(), &sniff, kind);
                let text = entry.path("txt");
                let stats = content::transcode_to_utf8(
                    std::io::BufReader::new(std::fs::File::open(&raw)?),
                    std::io::BufWriter::new(entry.create("txt")?),
                    charset.encoding,
                )?;
                info.charset = Some(charset.name());
                info.charset_lossy = stats.lossy;
                text
            };
            let (id, body) = bodies.open(&path)?;
            info.body_id = Some(id);
            info.size = body.index().byte_len();
            info.row_count = body.index().row_count();
            info.pretty_available = kind.can_pretty_print() && info.size <= SMALL_BODY_LIMIT;
            let meta = BodyMeta {
                kind,
                stem: entry.stem().to_string(),
                media_type: content_type
                    .as_deref()
                    .and_then(|ct| ct.split(';').next())
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty()),
            };
            Ok((info, Some((id, meta))))
        })
        .await
        .map_err(|e| SendError::new(SendErrorKind::Internal, e.to_string()))?
        .map_err(io_error)?;

        if let Some((id, meta)) = meta {
            self.body_meta
                .lock()
                .expect("body meta lock poisoned")
                .insert(id, meta);
        }
        Ok(info)
    }

    /// Closes bodies of evicted spool entries, then deletes their files.
    fn evict(&self, stems: &[String]) {
        if stems.is_empty() {
            return;
        }
        let ids: Vec<BodyId> = self
            .body_meta
            .lock()
            .expect("body meta lock poisoned")
            .iter()
            .filter(|(_, m)| stems.contains(&m.stem))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close_body(id);
        }
        for stem in stems {
            self.spool.remove_entry(stem);
        }
    }
}

fn to_engine_request(
    wire: &WireRequest,
    net: Option<&NetOptions>,
    cancel: Option<CancellationToken>,
) -> Result<ironcall_http::Request, SendError> {
    let method = Method::from_bytes(wire.method.as_bytes())
        .map_err(|e| SendError::new(SendErrorKind::InvalidRequest, e.to_string()))?;
    let mut req = ironcall_http::Request::new(method, &wire.url).map_err(|e| engine_error(&e))?;
    let mut headers = HeaderMap::new();
    for (name, value) in &wire.headers {
        let invalid = |e: String| SendError::new(SendErrorKind::InvalidRequest, e);
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| invalid(e.to_string()))?;
        let value = HeaderValue::from_str(value).map_err(|e| invalid(e.to_string()))?;
        headers.append(name, value);
    }
    req.headers = headers;
    req.body = match &wire.body {
        WireBody::Bytes(bytes) => bytes.clone().into(),
        WireBody::File { path, content_type } => Body::File {
            path: path.clone(),
            content_type: Some(content_type.clone()),
        },
        WireBody::Multipart(parts) => Body::Multipart(
            parts
                .iter()
                .map(|p| match &p.content {
                    PartContent::Text(value) => Part::Text {
                        name: p.name.clone(),
                        value: value.clone(),
                        content_type: None,
                    },
                    PartContent::File {
                        path,
                        file_name,
                        content_type,
                    } => Part::File {
                        name: p.name.clone(),
                        path: path.clone(),
                        file_name: Some(file_name.clone()),
                        content_type: Some(content_type.clone()),
                    },
                })
                .collect(),
        ),
    };
    req.options.verify_tls = wire.verify_tls;
    req.options.http_version = match wire.http_version {
        HttpVersion::Auto => HttpVersionPref::Auto,
        HttpVersion::Http1 => HttpVersionPref::Http1,
        HttpVersion::Http2 => HttpVersionPref::Http2,
    };
    req.options.fresh_connection = wire.fresh_connection;
    req.options.timeout = wire.timeout_ms.map(Duration::from_millis);
    req.options.follow_redirects = wire.follow_redirects;
    req.options.max_redirects = wire.max_redirects;
    req.options.cancel = cancel;
    if let Some(net) = net {
        req.options.proxy = Some(net.proxy.clone());
        req.options.bypass_proxy = net.bypass_proxy;
        req.options.extra_roots = net.roots.clone();
        req.options.cookie_jar = Some(net.jar.clone());
    }
    Ok(req)
}

fn preview_of(
    request: &RequestFile,
    ctx: &WireContext,
    wire: &WireRequest,
    engine_req: &ironcall_http::Request,
) -> RequestPreview {
    let derived = wire::derived(ctx, request);
    // `wire.headers` is the user's rows followed by the effective derived ones.
    let user_count = wire.headers.len() - derived.iter().filter(|d| d.effective).count();
    let mut headers: Vec<PreviewHeader> = wire.headers[..user_count]
        .iter()
        .map(|(name, value)| PreviewHeader {
            name: name.clone(),
            value: value.clone(),
            source: HeaderSource::User,
            effective: true,
            origin: None,
        })
        .chain(derived.into_iter().map(|d| PreviewHeader {
            name: d.name,
            value: d.value,
            source: match d.kind {
                DerivedKind::Derived => HeaderSource::Derived,
                DerivedKind::Auth => HeaderSource::Auth,
                DerivedKind::Default => HeaderSource::Default,
            },
            effective: d.effective,
            origin: d.origin,
        }))
        .collect();
    headers.extend(
        ironcall_http::engine_headers(engine_req)
            .into_iter()
            .map(|(name, value)| PreviewHeader {
                name: name.to_string(),
                value: value.to_str().unwrap_or_default().to_string(),
                source: HeaderSource::Engine,
                effective: true,
                origin: None,
            }),
    );
    RequestPreview {
        method: wire.method.clone(),
        url: wire.url.clone(),
        headers,
        body_size: engine_req.body.len().unwrap_or(0),
    }
}

/// Replaces secret values in a preview's URL and header values with `••••`.
fn mask_preview(mut preview: RequestPreview, secrets: &[SecretString]) -> RequestPreview {
    if secrets.is_empty() {
        return preview;
    }
    preview.url = redact::redact_url_masking(&preview.url, secrets);
    for h in &mut preview.headers {
        h.value = redact::mask_values(&h.value, secrets);
    }
    preview
}

/// The send timeline for the UI, with sensitive header values and secrets masked.
fn timeline(
    events: &[ironcall_http::TimelineEvent],
    secrets: &[SecretString],
    sensitive_headers: &[String],
) -> Vec<TimelineEntry> {
    events
        .iter()
        .map(|e| {
            let mut text = redact::mask_values(&e.message, secrets);
            for (name, value) in &e.headers {
                let value = redact::header_value(name, value, sensitive_headers);
                text.push('\n');
                text.push_str(name);
                text.push_str(": ");
                text.push_str(&redact::mask_values(value, secrets));
            }
            TimelineEntry {
                at_ms: ms(e.at),
                kind: match e.kind {
                    ironcall_http::TimelineKind::Info => TimelineKind::Info,
                    ironcall_http::TimelineKind::Tls => TimelineKind::Tls,
                    ironcall_http::TimelineKind::Proxy => TimelineKind::Proxy,
                    ironcall_http::TimelineKind::RequestHeaders => TimelineKind::RequestHeaders,
                    ironcall_http::TimelineKind::RequestBody => TimelineKind::RequestBody,
                    ironcall_http::TimelineKind::ResponseHeaders => TimelineKind::ResponseHeaders,
                    ironcall_http::TimelineKind::ResponseBody => TimelineKind::ResponseBody,
                    ironcall_http::TimelineKind::Redirect => TimelineKind::Redirect,
                    ironcall_http::TimelineKind::Error => TimelineKind::Error,
                },
                text,
            }
        })
        .collect()
}

fn unix_rfc3339(secs: Option<i64>) -> Option<String> {
    let secs = u64::try_from(secs?).ok()?;
    Some(rfc3339_utc(std::time::UNIX_EPOCH + Duration::from_secs(secs)))
}

fn tls_info(t: &ironcall_http::TlsInfo) -> TlsInfo {
    TlsInfo {
        version: t.version.clone(),
        cipher: t.cipher.clone(),
        alpn: t.alpn.clone(),
        verified: t.verified,
        certificate: t.certificate.as_ref().map(|c| CertificateInfo {
            subject: c.subject.clone(),
            issuer: c.issuer.clone(),
            not_before: unix_rfc3339(c.not_before),
            not_after: unix_rfc3339(c.not_after),
            sans: c.sans.clone(),
            chain_length: c.chain_length,
        }),
    }
}

fn header_rows(headers: &HeaderMap) -> Vec<HeaderRow> {
    headers
        .iter()
        .map(|(name, value)| HeaderRow {
            name: name.to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn phase(p: Phase) -> TimingPhase {
    match p {
        Phase::Measured(d) => TimingPhase::Measured { ms: ms(d) },
        Phase::Reused => TimingPhase::Reused,
        Phase::NotApplicable => TimingPhase::NotApplicable,
    }
}

/// `total` overrides the hop's own total, so the final timings cover redirects too.
fn timings_info(t: &Timings, total: Option<Duration>) -> TimingsInfo {
    TimingsInfo {
        dns: phase(t.dns),
        connect: phase(t.connect),
        tls: phase(t.tls),
        ttfb_ms: ms(t.ttfb),
        download_ms: ms(t.download),
        total_ms: ms(total.unwrap_or(t.total)),
    }
}

fn body_kind(kind: content::BodyKind) -> BodyKind {
    match kind {
        content::BodyKind::Empty => BodyKind::Empty,
        content::BodyKind::Json => BodyKind::Json,
        content::BodyKind::Xml => BodyKind::Xml,
        content::BodyKind::Html => BodyKind::Html,
        content::BodyKind::Text => BodyKind::Text,
        content::BodyKind::Binary => BodyKind::Binary,
    }
}

fn build_error(err: BuildError) -> SendError {
    let kind = match &err {
        BuildError::Url(UrlError::UnresolvedVariable(_)) => SendErrorKind::Unsupported,
        // The URL itself is fine; a value in the params table is missing.
        BuildError::Url(UrlError::MissingPathParam(_)) => SendErrorKind::InvalidRequest,
        BuildError::Url(_) => SendErrorKind::InvalidUrl,
        BuildError::Unsupported(_) => SendErrorKind::Unsupported,
        _ => SendErrorKind::InvalidRequest,
    };
    SendError::new(kind, err.to_string())
}

fn engine_error(err: &ironcall_http::Error) -> SendError {
    let kind = match err.kind() {
        ErrorKind::InvalidRequest => SendErrorKind::InvalidRequest,
        ErrorKind::InvalidUrl => SendErrorKind::InvalidUrl,
        ErrorKind::Dns => SendErrorKind::Dns,
        ErrorKind::Connect => SendErrorKind::Connect,
        ErrorKind::Tls => SendErrorKind::Tls,
        ErrorKind::Timeout => SendErrorKind::Timeout,
        ErrorKind::Cancelled => SendErrorKind::Cancelled,
        ErrorKind::TooManyRedirects => SendErrorKind::TooManyRedirects,
        ErrorKind::Protocol => SendErrorKind::Protocol,
        ErrorKind::Io => SendErrorKind::Io,
        ErrorKind::Unsupported => SendErrorKind::Unsupported,
        ErrorKind::Internal => SendErrorKind::Internal,
    };
    let mut detail = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        detail.push_str("\ncaused by: ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    SendError {
        kind,
        message: err.message(),
        detail: Some(detail),
        timeline: Vec::new(),
        history_id: None,
    }
}

fn io_error(err: std::io::Error) -> SendError {
    SendError {
        kind: SendErrorKind::Io,
        message: format!("Could not store the response body: {err}"),
        detail: None,
        timeline: Vec::new(),
        history_id: None,
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Instant;

    use axum::Router;
    use axum::http::header;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use ironcall_core::model::KeyValue;

    use super::*;

    async fn serve() -> SocketAddr {
        let app = Router::new()
            .route(
                "/json",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        r#"{"a":1,"b":[true,null]}"#,
                    )
                }),
            )
            .route(
                "/latin1",
                get(|| async {
                    // "café" in ISO-8859-1.
                    (
                        [(header::CONTENT_TYPE, "text/plain; charset=iso-8859-1")],
                        vec![b'c', b'a', b'f', 0xE9],
                    )
                }),
            )
            .route("/empty", get(|| async { axum::http::StatusCode::NO_CONTENT }))
            .route(
                "/binary",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/octet-stream")],
                        vec![0u8, 1, 2, 255],
                    )
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    "late"
                }),
            )
            .route(
                "/big",
                get(|| async {
                    // About 20MB of pretty JSON, the Phase 1 exit-criterion size.
                    let row = "  {\"id\": 12345, \"name\": \"row\", \"tags\": [\"a\", \"b\"]},\n";
                    let mut body = String::from("[\n");
                    while body.len() < 20 * 1024 * 1024 {
                        body.push_str(row);
                    }
                    body.push_str("  {}\n]\n");
                    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn sender() -> (tempfile::TempDir, Sender) {
        let tmp = tempfile::tempdir().unwrap();
        let spool = Spool::open(tmp.path().join("spool"), 5).unwrap();
        let engine = Engine::new(ironcall_http::EngineConfig::default()).unwrap();
        (tmp, Sender::new(engine, spool, Arc::new(BodyStore::new())))
    }

    /// A mock Tauri app with `SendProgress` registered, as `run()` does.
    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        tauri_specta::Builder::<tauri::test::MockRuntime>::new()
            .events(tauri_specta::collect_events![SendProgress])
            .mount_events(&app);
        app
    }

    fn net() -> NetOptions {
        NetOptions {
            proxy: ironcall_http::ProxyConfig::none(),
            roots: None,
            jar: Arc::new(ironcall_http::CookieJar::new()),
            jar_key: "test".into(),
            bypass_proxy: false,
            sensitive_headers: Vec::new(),
        }
    }

    fn request(addr: SocketAddr, path: &str) -> RequestFile {
        let mut req = RequestFile::new("test");
        req.url = format!("http://{addr}{path}");
        req
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn json_response_is_stored_and_pretty_printable() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();

        let summary = sender
            .send(
                app.handle(),
                "s1".into(),
                request(addr, "/json"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(summary.status, 200);
        assert_eq!(summary.http_version, "HTTP/1.1");
        assert_eq!(summary.body.kind, BodyKind::Json);
        assert!(summary.body.pretty_available);
        let id = summary.body.body_id.unwrap();
        assert_eq!(sender.body_text(id, false).unwrap(), r#"{"a":1,"b":[true,null]}"#);
        assert!(sender.body_text(id, true).unwrap().contains("\n  \"a\": 1"));

        // The request as sent lists the engine's headers.
        let names: Vec<_> = summary
            .request
            .headers
            .iter()
            .map(|h| h.name.to_ascii_lowercase())
            .collect();
        assert!(names.contains(&"user-agent".to_string()), "{names:?}");

        let recent = sender.recent();
        assert_eq!(recent[0].status, Some(200));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_utf8_text_is_transcoded() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();
        let summary = sender
            .send(
                app.handle(),
                "s1".into(),
                request(addr, "/latin1"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(summary.body.charset.as_deref(), Some("windows-1252"));
        assert_eq!(
            sender.body_text(summary.body.body_id.unwrap(), false).unwrap(),
            "café"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancel_before_the_send_starts_is_kept() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();
        let reservation = sender.begin("early", None);
        sender.cancel("early");
        let err = sender
            .send(
                app.handle(),
                "early".into(),
                request(addr, "/empty"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, SendErrorKind::Cancelled);
        drop(reservation);

        // A parent token (a run) cancels sends registered under it.
        let run = CancellationToken::new();
        let _r = sender.begin("child", Some(&run));
        run.cancel();
        let err = sender
            .send(
                app.handle(),
                "child".into(),
                request(addr, "/empty"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, SendErrorKind::Cancelled);
        assert!(
            sender.reserved.lock().unwrap().is_empty(),
            "the send took its reservation"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_and_binary_bodies() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();

        let empty = sender
            .send(
                app.handle(),
                "e".into(),
                request(addr, "/empty"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(empty.status, 204);
        assert_eq!(empty.body.kind, BodyKind::Empty);
        assert!(empty.body.body_id.is_none());

        let binary = sender
            .send(
                app.handle(),
                "b".into(),
                request(addr, "/binary"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(binary.body.kind, BodyKind::Binary);
        assert_eq!(binary.body.size, 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_returns_cancelled_and_cleans_up() {
        let addr = serve().await;
        let (tmp, sender) = sender();
        let sender = Arc::new(sender);
        let app = mock_app();

        let handle = app.handle().clone();
        let task = {
            let sender = sender.clone();
            tokio::spawn(async move {
                sender
                    .send(
                        &handle,
                        "c1".into(),
                        request(addr, "/slow"),
                        &WireContext::default(),
                        net(),
                        Vec::new(),
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        sender.cancel("c1");
        let err = task.await.unwrap().unwrap_err();
        assert_eq!(err.kind, SendErrorKind::Cancelled);
        assert_eq!(sender.recent()[0].error, Some(SendErrorKind::Cancelled));
        assert_eq!(std::fs::read_dir(tmp.path().join("spool")).unwrap().count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn errors_are_classified() {
        let (_tmp, sender) = sender();
        let app = mock_app();

        let mut no_scheme = RequestFile::new("x");
        no_scheme.url = "example.com".into();
        let err = sender
            .send(
                app.handle(),
                "u".into(),
                no_scheme,
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, SendErrorKind::InvalidUrl);

        // Port 9 on localhost is almost never listening.
        let mut refused = RequestFile::new("x");
        refused.url = "http://127.0.0.1:9/".into();
        let err = sender
            .send(
                app.handle(),
                "r".into(),
                refused,
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, SendErrorKind::Connect);
        assert!(err.detail.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recent_sends_redact_sensitive_query_values() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();
        let mut req = request(addr, "/json");
        req.params = vec![KeyValue::new("api_key", "hunter2"), KeyValue::new("page", "2")];
        sender
            .send(
                app.handle(),
                "s".into(),
                req,
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        let url = &sender.recent()[0].url;
        assert!(!url.contains("hunter2"), "{url}");
        assert!(url.contains("page=2"), "{url}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn twenty_mb_response_first_page_within_budget() {
        let addr = serve().await;
        let (_tmp, sender) = sender();
        let app = mock_app();

        let summary = sender
            .send(
                app.handle(),
                "big".into(),
                request(addr, "/big"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        // Exit criterion: first page visible in under 1s. This measures the backend part
        // (indexing is done during send; this is the first page fetch after it).
        let start = Instant::now();
        let body = sender.bodies.get(summary.body.body_id.unwrap()).unwrap();
        let rows = body.rows(0, 200).unwrap();
        let first_page = start.elapsed();

        assert!(summary.body.size > 20 * 1024 * 1024);
        assert!(!summary.body.pretty_available, "over the small-body limit");
        assert_eq!(rows.len(), 200);
        assert!(
            Duration::from_secs_f64(summary.timings.total_ms / 1000.0) + first_page < Duration::from_secs(5),
            "send {}ms + first page {first_page:?}",
            summary.timings.total_ms
        );
        eprintln!(
            "20MB: send {:.0}ms (download {:.0}ms), first page {:?}",
            summary.timings.total_ms, summary.timings.download_ms, first_page
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closing_a_body_deletes_its_files() {
        let addr = serve().await;
        let (tmp, sender) = sender();
        let app = mock_app();
        let summary = sender
            .send(
                app.handle(),
                "s".into(),
                request(addr, "/json"),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await
            .unwrap();
        sender.close_body(summary.body.body_id.unwrap());
        assert_eq!(std::fs::read_dir(tmp.path().join("spool")).unwrap().count(), 0);
    }
}
