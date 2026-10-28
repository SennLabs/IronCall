use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{
    ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, PROXY_AUTHORIZATION, USER_AGENT,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use hyper::body::Incoming;
use hyper::client::conn::{http1, http2};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use crate::body::{Body, ReqBody};
use crate::ca::RootSet;
use crate::connect;
use crate::cookies::CookieJar;
use crate::decode::{self, Plan};
use crate::error::Error;
use crate::pool::{Pool, PoolKey, Pooled, Sender};
use crate::proxy::{self, ProxyConfig, ProxyServer};
use crate::redirect;
use crate::timings::{Phase, Timings};
use crate::tls::TlsCache;
use crate::trace::{self, Failure, Informational, Recorder, TimelineEvent, TimelineKind, TlsInfo};

/// The `User-Agent` sent when the request doesn't set one.
pub const DEFAULT_USER_AGENT: &str = concat!("IronCall/", env!("CARGO_PKG_VERSION"));

/// Redirect responses smaller than this are drained so their connection can be reused;
/// larger ones are dropped along with their connection.
const MAX_REDIRECT_BODY_DRAIN: u64 = 64 * 1024;

/// Minimum time between `Uploading` and `Downloading` progress reports.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HttpVersionPref {
    /// Negotiate with ALPN, preferring HTTP/2.
    #[default]
    Auto,
    Http1,
    Http2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Http1_1,
    Http2,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http1_1 => "HTTP/1.1",
            Protocol::Http2 => "HTTP/2",
        }
    }
}

/// Where a request is in its lifecycle. Reported through progress callbacks and in
/// timeout errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Resolving DNS, connecting, TLS handshake (or waiting for a free connection).
    Connecting = 0,
    /// Sending a file or multipart body (in-memory bodies skip this stage).
    Uploading = 1,
    /// Request sent, waiting for the response headers.
    Waiting = 2,
    /// Reading the response body.
    Downloading = 3,
}

impl Stage {
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Stage::Connecting => "connecting",
            Stage::Uploading => "uploading the request body",
            Stage::Waiting => "waiting for the response",
            Stage::Downloading => "downloading the response",
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => Stage::Connecting,
            1 => Stage::Uploading,
            2 => Stage::Waiting,
            _ => Stage::Downloading,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// Decoded body bytes written to the sink so far.
    pub bytes: u64,
    /// Request body bytes handed to the connection so far (while `Uploading`).
    pub uploaded: u64,
    /// Total request body size, for file and multipart bodies.
    pub upload_total: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct RequestOptions {
    pub verify_tls: bool,
    pub http_version: HttpVersionPref,
    /// Open a new connection even if a pooled one is available, so DNS, connect and TLS
    /// are measured (PLAN.md §3.4). The new connection is pooled afterwards as usual.
    pub fresh_connection: bool,
    /// Limit for the whole send, including redirects.
    pub timeout: Option<Duration>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    /// Cancelling this token aborts the send with [`Error::Cancelled`].
    pub cancel: Option<CancellationToken>,
    /// Proxy for this request instead of the engine's [`EngineConfig::proxy`].
    pub proxy: Option<ProxyConfig>,
    /// Connect directly even if a proxy is configured.
    pub bypass_proxy: bool,
    /// CA certificates trusted for this request on top of the OS store and the engine's
    /// extra roots (e.g. a workspace's CAs).
    pub extra_roots: Option<RootSet>,
    /// Cookies are sent from and stored into this jar on every hop.
    pub cookie_jar: Option<Arc<CookieJar>>,
    /// Headers removed when a redirect goes to another origin, on top of `Authorization`,
    /// `Cookie` and `Proxy-Authorization` (e.g. `X-Api-Key`, user-configured ones).
    pub strip_on_cross_origin: Vec<String>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            verify_tls: true,
            http_version: HttpVersionPref::Auto,
            fresh_connection: false,
            timeout: None,
            follow_redirects: true,
            max_redirects: 10,
            cancel: None,
            proxy: None,
            bypass_proxy: false,
            extra_roots: None,
            cookie_jar: None,
            strip_on_cross_origin: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: Uri,
    pub headers: HeaderMap,
    pub body: Body,
    pub options: RequestOptions,
}

impl Request {
    pub fn new(method: Method, url: &str) -> Result<Self, Error> {
        let url = url.parse::<Uri>().map_err(|e| Error::InvalidUrl(e.to_string()))?;
        Ok(Self {
            method,
            url,
            headers: HeaderMap::new(),
            body: Body::default(),
            options: RequestOptions::default(),
        })
    }

    pub fn get(url: &str) -> Result<Self, Error> {
        Self::new(Method::GET, url)
    }
}

/// One redirect response that was followed.
#[derive(Clone, Debug)]
pub struct Hop {
    pub status: StatusCode,
    /// The URL that was requested and answered with this redirect.
    pub url: String,
    /// The resolved `Location` the engine went to next.
    pub location: String,
    pub timings: Timings,
    /// TLS details of the connection this hop used; `None` for plain HTTP.
    pub tls: Option<TlsInfo>,
}

/// Everything about the final response except its body, which went to the sink.
#[derive(Debug)]
pub struct ResponseHead {
    pub status: StatusCode,
    /// Reason phrase from the server, or the standard one for the status.
    pub reason: String,
    pub headers: HeaderMap,
    pub protocol: Protocol,
    /// Timings of the final hop only; see `redirects` for earlier hops and `total`.
    pub timings: Timings,
    pub connection_id: u64,
    pub connection_reused: bool,
    pub redirects: Vec<Hop>,
    /// URL of the final request (differs from the request URL after redirects).
    pub final_url: String,
    /// Decoded body size, as written to the sink.
    pub body_len: u64,
    /// Body size on the wire, before decompression.
    pub wire_len: u64,
    /// Content codings that were undone, in decoding order.
    pub decoded_encodings: Vec<String>,
    /// Set when `Content-Encoding` named a coding the engine doesn't support; the body
    /// was passed through undecoded.
    pub undecoded_encoding: Option<String>,
    /// Wall time for the whole send, including redirects.
    pub total: Duration,
    /// TLS details of the final connection; `None` for plain HTTP.
    pub tls: Option<TlsInfo>,
    /// 1xx responses received before the final response (HTTP/1.1 only; hyper doesn't
    /// expose them on HTTP/2). TTFB is measured to the final response headers.
    pub informational: Vec<Informational>,
    /// What happened during the send, in order (header values raw; mask before showing).
    pub timeline: Vec<TimelineEvent>,
}

/// A response with its body collected in memory. Dereferences to [`ResponseHead`].
#[derive(Debug)]
pub struct Response {
    pub head: ResponseHead,
    pub body: Bytes,
}

impl Deref for Response {
    type Target = ResponseHead;

    fn deref(&self) -> &ResponseHead {
        &self.head
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// CA certificates trusted in addition to the OS trust store.
    pub extra_roots: Vec<CertificateDer<'static>>,
    /// Sends allowed in flight at once across the engine (PLAN.md §3.4).
    pub max_in_flight: usize,
    /// Busy HTTP/1.1 connections allowed per origin.
    pub max_per_host_http1: usize,
    /// Pooled connections unused for this long are closed.
    pub idle_timeout: Duration,
    /// Default proxy (requests may override it). Direct by default.
    pub proxy: ProxyConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            extra_roots: Vec::new(),
            max_in_flight: 32,
            max_per_host_http1: 6,
            idle_timeout: Duration::from_secs(90),
            proxy: ProxyConfig::none(),
        }
    }
}

pub struct Engine {
    tls: TlsCache,
    pool: Pool,
    in_flight: Arc<Semaphore>,
    next_connection_id: AtomicU64,
    proxy: ProxyConfig,
}

struct Setup {
    dns: Phase,
    connect: Phase,
    tls: Phase,
    tls_info: Option<Arc<TlsInfo>>,
}

/// A connection checked out for one request. Dropping it closes the connection;
/// [`Lease::release`] returns it to the pool once the response body has been read.
struct Lease<'a> {
    pool: &'a Pool,
    key: PoolKey,
    conn: Option<Pooled>,
    _host_permit: Option<OwnedSemaphorePermit>,
}

impl Lease<'_> {
    fn release(mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.checkin(self.key.clone(), conn);
        }
    }
}

/// Response headers received on a leased connection; the body is still unread.
struct Exchange<'a> {
    response: http::Response<Incoming>,
    lease: Lease<'a>,
    protocol: Protocol,
    connection_id: u64,
    reused: bool,
    setup: Setup,
    hop_start: Instant,
    ttfb: Duration,
    informational: Arc<Mutex<Vec<Informational>>>,
}

/// Shared between a send and its timeout, so a timeout can say which stage it hit and
/// the timeline survives cancellation.
struct Tracker<'a> {
    stage: AtomicU8,
    progress: &'a (dyn Fn(Progress) + Send + Sync),
    timeline: Arc<Recorder>,
}

impl Tracker<'_> {
    fn enter(&self, stage: Stage, bytes: u64) {
        self.stage.store(stage as u8, Ordering::Relaxed);
        (self.progress)(Progress {
            stage,
            bytes,
            uploaded: 0,
            upload_total: None,
        });
    }

    fn upload(&self, uploaded: u64, total: u64) {
        self.stage.store(Stage::Uploading as u8, Ordering::Relaxed);
        (self.progress)(Progress {
            stage: Stage::Uploading,
            bytes: 0,
            uploaded,
            upload_total: Some(total),
        });
    }

    fn stage(&self) -> Stage {
        Stage::from_u8(self.stage.load(Ordering::Relaxed))
    }
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self, Error> {
        Ok(Self {
            tls: TlsCache::new(config.extra_roots)?,
            pool: Pool::new(config.idle_timeout, config.max_per_host_http1.max(1)),
            in_flight: Arc::new(Semaphore::new(config.max_in_flight.max(1))),
            next_connection_id: AtomicU64::new(1),
            proxy: config.proxy,
        })
    }

    /// The proxy a request goes through, or `None` for a direct connection.
    fn proxy_for(&self, req: &Request) -> Option<ProxyServer> {
        if req.options.bypass_proxy {
            return None;
        }
        req.options
            .proxy
            .as_ref()
            .unwrap_or(&self.proxy)
            .select(&req.url)
            .cloned()
    }

    /// Sends a request and collects the decoded body in memory.
    pub async fn send(&self, req: Request) -> Result<Response, Error> {
        let mut body = Vec::new();
        let head = self.send_to(req, &mut body, &|_| {}).await?;
        Ok(Response {
            head,
            body: Bytes::from(body),
        })
    }

    /// [`Engine::send_to`], dropping the timeline on failure.
    pub async fn send_to<W>(
        &self,
        req: Request,
        sink: &mut W,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<ResponseHead, Error>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        self.send_to_traced(req, sink, progress)
            .await
            .map_err(|f| f.error)
    }

    /// Sends a request, following redirects if enabled, and streams the decoded body of
    /// the final response into `sink`. The head carries the timeline; on failure the
    /// timeline up to the error comes with it in [`Failure`].
    ///
    /// On error (including cancellation and timeout) whatever body was already received
    /// stays in `sink`. `progress` is called on each stage change and periodically while
    /// downloading.
    pub async fn send_to_traced<W>(
        &self,
        req: Request,
        sink: &mut W,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<ResponseHead, Failure>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let tracker = Tracker {
            stage: AtomicU8::new(Stage::Connecting as u8),
            progress,
            timeline: Arc::new(Recorder::new()),
        };
        let cancel = req.options.cancel.clone();
        let timeout = req.options.timeout;

        let send = self.send_inner(req, sink, &tracker);
        let cancelled = async {
            match &cancel {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let timed_out = async {
            match timeout {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        // Losing branches drop the in-flight send, which closes its connection rather
        // than returning it to the pool.
        let result = tokio::select! {
            biased;
            () = cancelled => Err(Error::Cancelled),
            () = timed_out => Err(Error::Timeout {
                limit: timeout.unwrap_or_default(),
                stage: tracker.stage(),
            }),
            result = send => result,
        };
        match result {
            Ok(mut head) => {
                head.timeline = tracker.timeline.take();
                Ok(head)
            }
            Err(error) => {
                tracker.timeline.push(TimelineKind::Error, error.message());
                Err(Failure {
                    error,
                    timeline: tracker.timeline.take(),
                })
            }
        }
    }

    async fn send_inner<W>(
        &self,
        req: Request,
        sink: &mut W,
        tracker: &Tracker<'_>,
    ) -> Result<ResponseHead, Error>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let start = Instant::now();
        tracker.enter(Stage::Connecting, 0);
        let _in_flight = self
            .in_flight
            .clone()
            .acquire_owned()
            .await
            .expect("in-flight semaphore is never closed");

        let mut current = req;
        let mut redirects = Vec::new();
        loop {
            let exchange = self.exchange(&current, tracker).await?;
            let status = exchange.response.status();
            tracker.timeline.push_headers(
                TimelineKind::ResponseHeaders,
                format!(
                    "{} {} {}",
                    exchange.protocol.as_str(),
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )
                .trim_end()
                .to_string(),
                trace::header_pairs(exchange.response.headers()),
            );
            if let Some(jar) = &current.options.cookie_jar {
                jar.store_response(&current.url, exchange.response.headers());
            }

            if current.options.follow_redirects
                && let Some(location) = redirect::location(status, exchange.response.headers())
            {
                if redirects.len() as u32 >= current.options.max_redirects {
                    return Err(Error::TooManyRedirects(current.options.max_redirects));
                }
                let next = redirect::next_request(&current, status, location)?;
                tracker.timeline.push(
                    TimelineKind::Redirect,
                    if next.method == current.method {
                        format!("Following {} to {}", status.as_u16(), next.url)
                    } else {
                        format!(
                            "Following {} to {} (method changed to {})",
                            status.as_u16(),
                            next.url,
                            next.method
                        )
                    },
                );
                let headers_at = Instant::now();
                let Exchange {
                    response,
                    lease,
                    setup,
                    hop_start,
                    ttfb,
                    ..
                } = exchange;
                drain_and_release(response, lease).await;
                redirects.push(Hop {
                    status,
                    url: current.url.to_string(),
                    location: next.url.to_string(),
                    timings: Timings {
                        dns: setup.dns,
                        connect: setup.connect,
                        tls: setup.tls,
                        ttfb,
                        download: headers_at.elapsed(),
                        total: hop_start.elapsed(),
                    },
                    tls: setup.tls_info.as_deref().cloned(),
                });
                current = next;
                tracker.enter(Stage::Connecting, 0);
                continue;
            }

            return self
                .finish(exchange, current, redirects, sink, tracker, start)
                .await;
        }
    }

    /// Streams the final response body into the sink and assembles the head.
    async fn finish<W>(
        &self,
        exchange: Exchange<'_>,
        req: Request,
        redirects: Vec<Hop>,
        sink: &mut W,
        tracker: &Tracker<'_>,
        start: Instant,
    ) -> Result<ResponseHead, Error>
    where
        W: AsyncWrite + Unpin + Send + ?Sized,
    {
        let Exchange {
            response,
            lease,
            protocol,
            connection_id,
            reused,
            setup,
            hop_start,
            ttfb,
            informational,
        } = exchange;
        let (parts, body) = response.into_parts();
        let reason = parts
            .extensions
            .get::<hyper::ext::ReasonPhrase>()
            .and_then(|r| std::str::from_utf8(r.as_bytes()).ok())
            .map(str::to_string)
            .or_else(|| parts.status.canonical_reason().map(str::to_string))
            .unwrap_or_default();

        let headers_at = Instant::now();
        tracker.enter(Stage::Downloading, 0);

        let wire = Arc::new(AtomicU64::new(0));
        let reader = decode::wire_reader(body, wire.clone());
        let (codings, undecoded_encoding) = match decode::plan(&parts.headers) {
            Plan::Decode(codings) => (codings, None),
            Plan::Passthrough(token) => (Vec::new(), Some(token)),
        };
        let encoding_names = codings.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ");
        let mut decoded = decode::decoder(reader, &codings)
            .await
            .map_err(|e| read_error(e, &encoding_names))?;
        let body_len = copy_body(&mut decoded, sink, tracker, &encoding_names).await?;
        // The decoder may stop at the end of the compressed stream without polling the
        // body's end; drop it before deciding whether the connection can be reused.
        drop(decoded);
        lease.release();
        let download = headers_at.elapsed();
        let wire_len = wire.load(Ordering::Relaxed);
        let mut summary = format!("Received {body_len} bytes");
        if !codings.is_empty() {
            summary.push_str(&format!(" ({wire_len} on the wire, decoded {encoding_names})"));
        } else if let Some(token) = &undecoded_encoding {
            summary.push_str(&format!(" (unsupported Content-Encoding `{token}`, not decoded)"));
        }
        summary.push_str(&format!(" in {}", trace::ms(download)));
        tracker.timeline.push(TimelineKind::ResponseBody, summary);

        Ok(ResponseHead {
            status: parts.status,
            reason,
            headers: parts.headers,
            protocol,
            timings: Timings {
                dns: setup.dns,
                connect: setup.connect,
                tls: setup.tls,
                ttfb,
                download,
                total: hop_start.elapsed(),
            },
            connection_id,
            connection_reused: reused,
            redirects,
            final_url: req.url.to_string(),
            body_len,
            wire_len,
            decoded_encodings: codings.iter().map(|c| c.name().to_string()).collect(),
            undecoded_encoding,
            total: start.elapsed(),
            tls: setup.tls_info.as_deref().cloned(),
            informational: std::mem::take(&mut *informational.lock().expect("informational lock poisoned")),
            timeline: Vec::new(),
        })
    }

    /// Sends one request on a pooled or new connection and waits for the response
    /// headers. A pooled HTTP/1.1 connection the server already closed is retried once
    /// on a new connection.
    async fn exchange(&self, req: &Request, tracker: &Tracker<'_>) -> Result<Exchange<'_>, Error> {
        let proxy = self.proxy_for(req);
        let key = pool_key(req, proxy.clone())?;
        // Checks files before connecting, so a missing file fails fast.
        let body_len = req.body.len()?;
        let upload_total = req.body.is_streamed().then_some(body_len);
        let mut force_fresh = req.options.fresh_connection;
        loop {
            let hop_start = Instant::now();
            tracker.enter(Stage::Connecting, 0);
            let mut host_permit = Some(self.pool.host_permit(&key).await);

            let pooled = if force_fresh {
                None
            } else {
                self.pool.checkout(&key)
            };
            let (conn, setup, reused) = match pooled {
                Some(conn) => {
                    tracker.timeline.push(
                        TimelineKind::Info,
                        format!(
                            "Reusing connection #{} to {}:{} ({})",
                            conn.id,
                            key.host,
                            key.port,
                            conn.sender.protocol().as_str()
                        ),
                    );
                    let tls_info = conn.tls.clone();
                    (
                        conn,
                        Setup {
                            dns: Phase::Reused,
                            connect: Phase::Reused,
                            tls: Phase::Reused,
                            tls_info,
                        },
                        true,
                    )
                }
                None => {
                    let (conn, setup) = self.connect(&key, req, tracker).await?;
                    (conn, setup, false)
                }
            };
            let protocol = conn.sender.protocol();
            if protocol == Protocol::Http2 {
                // HTTP/2 multiplexes; the per-host cap only applies to HTTP/1.1.
                host_permit = None;
            }
            let connection_id = conn.id;
            let mut lease = Lease {
                pool: &self.pool,
                key: key.clone(),
                conn: Some(conn),
                _host_permit: host_permit,
            };
            let sent = Arc::new(AtomicU64::new(0));
            let via = if key.https { None } else { key.proxy.as_ref() };
            let mut http_req = build_request(req, protocol, via, key.proxy.is_some(), sent.clone())?;
            tracker.timeline.push_headers(
                TimelineKind::RequestHeaders,
                format!("{} {} {}", http_req.method(), http_req.uri(), protocol.as_str()),
                trace::header_pairs(http_req.headers()),
            );
            if let Some(summary) = body_summary(&req.body, body_len) {
                tracker.timeline.push(TimelineKind::RequestBody, summary);
            }
            let informational: Arc<Mutex<Vec<Informational>>> = Arc::default();
            let sent_at = Instant::now();
            {
                let info = informational.clone();
                let timeline = tracker.timeline.clone();
                hyper::ext::on_informational(&mut http_req, move |res| {
                    timeline.push_headers(
                        TimelineKind::ResponseHeaders,
                        format!(
                            "HTTP/1.1 {} {}",
                            res.status().as_u16(),
                            res.status().canonical_reason().unwrap_or("")
                        )
                        .trim_end()
                        .to_string(),
                        trace::header_pairs(res.headers()),
                    );
                    info.lock()
                        .expect("informational lock poisoned")
                        .push(Informational {
                            status: res.status(),
                            headers: res.headers().clone(),
                            after_send: sent_at.elapsed(),
                        });
                });
            }

            match upload_total {
                Some(total) => tracker.upload(0, total),
                None => tracker.enter(Stage::Waiting, 0),
            }
            let mut uploading = upload_total;
            let result = {
                let sender = &mut lease.conn.as_mut().expect("lease holds a connection").sender;
                let send = sender.send(http_req);
                tokio::pin!(send);
                let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
                tick.tick().await;
                loop {
                    tokio::select! {
                        result = &mut send => break result,
                        _ = tick.tick(), if uploading.is_some() => {
                            let total = uploading.unwrap_or_default();
                            let n = sent.load(Ordering::Relaxed);
                            tracker.upload(n, total);
                            if n >= total {
                                tracker.enter(Stage::Waiting, 0);
                                uploading = None;
                            }
                        }
                    }
                }
            };
            if let Some(total) = uploading {
                // The response arrived before the next tick: report the final count.
                tracker.upload(sent.load(Ordering::Relaxed).min(total), total);
                tracker.enter(Stage::Waiting, 0);
            }
            match result {
                Ok(response) => {
                    return Ok(Exchange {
                        response,
                        lease,
                        protocol,
                        connection_id,
                        reused,
                        setup,
                        hop_start,
                        ttfb: sent_at.elapsed(),
                        informational,
                    });
                }
                Err(e) => {
                    let err = Error::Http(e);
                    // `lease` is dropped here, closing the broken connection.
                    if reused
                        && protocol == Protocol::Http1_1
                        && !force_fresh
                        && err.is_safe_to_retry(&req.method)
                    {
                        tracing::debug!(error = %err, "pooled connection was stale; retrying on a new one");
                        tracker.timeline.push(
                            TimelineKind::Info,
                            format!("Pooled connection #{connection_id} was closed by the server; retrying on a new connection"),
                        );
                        force_fresh = true;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    /// Opens a connection for `key`: directly, through an HTTP proxy (plain HTTP), or
    /// through a `CONNECT` tunnel (HTTPS via proxy). DNS and connect are measured to the
    /// proxy when there is one; the `CONNECT` exchange counts as connect time.
    async fn connect(
        &self,
        key: &PoolKey,
        req: &Request,
        tracker: &Tracker<'_>,
    ) -> Result<(Pooled, Setup), Error> {
        let timeline = &tracker.timeline;
        let (dial_host, dial_port) = match &key.proxy {
            Some(p) => (p.host.as_str(), p.port),
            None => (key.host.as_str(), key.port),
        };
        timeline.push(
            TimelineKind::Info,
            match &key.proxy {
                Some(p) => format!("Resolving proxy {}…", p.display()),
                None => format!("Resolving {dial_host}…"),
            },
        );
        let dns_start = Instant::now();
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((dial_host, dial_port))
            .await
            .map_err(|source| Error::Dns {
                host: dial_host.to_string(),
                source,
            })?
            .collect();
        let dns = dns_start.elapsed();
        if addrs.is_empty() {
            return Err(Error::NoAddresses(dial_host.to_string()));
        }
        let mut ips: Vec<String> = addrs.iter().map(|a| a.ip().to_string()).collect();
        ips.dedup();
        timeline.push(
            TimelineKind::Info,
            format!("Resolved {dial_host} to {} in {}", ips.join(", "), trace::ms(dns)),
        );
        timeline.push(
            TimelineKind::Info,
            match &key.proxy {
                Some(p) => format!("Connecting to proxy {} port {dial_port}…", p.host),
                None => format!("Connecting to {dial_host} port {dial_port}…"),
            },
        );

        let connect_start = Instant::now();
        let mut tcp = connect::connect_happy(&connect::interleave(addrs), connect::ATTEMPT_DELAY)
            .await
            .map_err(|source| match &key.proxy {
                Some(p) => Error::ProxyConnect {
                    proxy: p.display(),
                    source,
                },
                None => Error::Connect {
                    host: key.host.clone(),
                    source,
                },
            })?;
        let peer = tcp
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| format!("{dial_host}:{dial_port}"));
        timeline.push(
            TimelineKind::Info,
            format!("Connected to {peer} in {}", trace::ms(connect_start.elapsed())),
        );
        if let Some(proxy) = &key.proxy
            && key.https
        {
            let target = proxy::authority(&key.host, key.port);
            let auth = proxy
                .auth
                .as_ref()
                .map(|a| a.header())
                .or_else(|| req.headers.get(PROXY_AUTHORIZATION).cloned());
            let mut headers = vec![("host".to_string(), target.clone())];
            if let Some(auth) = &auth {
                headers.push((
                    "proxy-authorization".to_string(),
                    String::from_utf8_lossy(auth.as_bytes()).into_owned(),
                ));
            }
            timeline.push_headers(TimelineKind::Proxy, format!("CONNECT {target} HTTP/1.1"), headers);
            let reply = proxy::connect_tunnel(
                &mut tcp,
                proxy,
                &key.host,
                key.port,
                req.headers.get(PROXY_AUTHORIZATION),
            )
            .await;
            match &reply {
                Ok(line) => timeline.push(TimelineKind::Proxy, format!("Proxy replied: {line}")),
                Err(Error::Proxy { status, .. }) if *status != 0 => {
                    timeline.push(TimelineKind::Proxy, format!("Proxy replied with status {status}"))
                }
                Err(_) => {}
            }
            reply?;
        }
        let connect = connect_start.elapsed();

        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        if !key.https {
            let sender = handshake(tcp, false).await?;
            let setup = Setup {
                dns: Phase::Measured(dns),
                connect: Phase::Measured(connect),
                tls: Phase::NotApplicable,
                tls_info: None,
            };
            return Ok((
                Pooled {
                    id,
                    sender,
                    last_used: now,
                    tls: None,
                },
                setup,
            ));
        }

        let server_name =
            ServerName::try_from(key.host.clone()).map_err(|e| Error::InvalidUrl(e.to_string()))?;
        let connector = TlsConnector::from(self.tls.get(
            key.verify_tls,
            key.version,
            req.options.extra_roots.as_ref(),
        )?);
        let tls_start = Instant::now();
        let stream = connector.connect(server_name, tcp).await.map_err(|source| {
            if key.version == HttpVersionPref::Http2 && is_no_alpn_alert(&source) {
                Error::Http2NotNegotiated
            } else {
                Error::Tls {
                    host: key.host.clone(),
                    source,
                }
            }
        })?;
        let tls = tls_start.elapsed();
        let tls_info = Arc::new(TlsInfo::from_connection(stream.get_ref().1, key.verify_tls));
        timeline.push(
            TimelineKind::Tls,
            format!(
                "TLS handshake complete in {}: {}",
                trace::ms(tls),
                tls_info.summary()
            ),
        );
        if let Some(cert) = &tls_info.certificate {
            let expiry = cert
                .not_after
                .map(|t| format!(", expires {}", unix_to_date(t)))
                .unwrap_or_default();
            timeline.push(
                TimelineKind::Tls,
                format!(
                    "Server certificate: {} (issuer {}){expiry}",
                    cert.subject, cert.issuer
                ),
            );
        }

        let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
        if key.version == HttpVersionPref::Http2 && !h2 {
            return Err(Error::Http2NotNegotiated);
        }
        let sender = handshake(stream, h2).await?;
        if let Sender::H2(s) = &sender {
            // HTTP/2 connections are shared, so they enter the pool as soon as they exist.
            self.pool.checkin(
                key.clone(),
                Pooled {
                    id,
                    sender: Sender::H2(s.clone()),
                    last_used: now,
                    tls: Some(tls_info.clone()),
                },
            );
        }
        let setup = Setup {
            dns: Phase::Measured(dns),
            connect: Phase::Measured(connect),
            tls: Phase::Measured(tls),
            tls_info: Some(tls_info.clone()),
        };
        Ok((
            Pooled {
                id,
                sender,
                last_used: now,
                tls: Some(tls_info),
            },
            setup,
        ))
    }
}

/// Headers the engine adds at send time when the request doesn't set them: `Host`
/// (HTTP/1.1 form), `User-Agent`, `Accept-Encoding`, `Content-Length`, `Content-Type`
/// for file and multipart bodies, and `Cookie` from the cookie jar. Used both to build
/// the request and to show the UI exactly what will be added. (`Proxy-Authorization`
/// is added too when going through an authenticated proxy, but never shown.)
///
/// A multipart body always gets the engine's `Content-Type` unless the request's own
/// one already names a boundary, since without the right boundary the body can't be
/// parsed.
pub fn engine_headers(req: &Request) -> Vec<(HeaderName, HeaderValue)> {
    let mut out = Vec::new();
    if !req.headers.contains_key(HOST)
        && let Some(host) = host_header(&req.url)
    {
        out.push((HOST, host));
    }
    if !req.headers.contains_key(USER_AGENT) {
        out.push((USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT)));
    }
    if !req.headers.contains_key(ACCEPT_ENCODING) {
        out.push((ACCEPT_ENCODING, HeaderValue::from_static(decode::ACCEPT_ENCODING)));
    }
    let body_expected = matches!(req.method, Method::POST | Method::PUT | Method::PATCH);
    if !req.headers.contains_key(CONTENT_LENGTH)
        && (!req.body.is_empty() || body_expected)
        && let Ok(len) = req.body.len()
    {
        out.push((CONTENT_LENGTH, HeaderValue::from(len)));
    }
    if let Some(content_type) = req.body.default_content_type() {
        let user = req.headers.get(CONTENT_TYPE);
        let keep_user = match &req.body {
            Body::Multipart(_) => user.is_some_and(|v| v.to_str().is_ok_and(|v| v.contains("boundary="))),
            _ => user.is_some(),
        };
        if !keep_user {
            out.push((CONTENT_TYPE, content_type));
        }
    }
    if !req.headers.contains_key(COOKIE)
        && let Some(jar) = &req.options.cookie_jar
        && let Some(cookie) = jar.header_for(&req.url)
    {
        out.push((COOKIE, cookie));
    }
    out
}

/// `host[:port]`, omitting the port when it is the scheme's default.
fn host_header(url: &Uri) -> Option<HeaderValue> {
    let host = url.host()?;
    let https = url.scheme_str() == Some("https");
    let value = match url.port_u16() {
        Some(port) if port != if https { 443 } else { 80 } => format!("{host}:{port}"),
        _ => host.to_string(),
    };
    HeaderValue::from_str(&value).ok()
}

fn pool_key(req: &Request, proxy: Option<ProxyServer>) -> Result<PoolKey, Error> {
    let https = match req.url.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        Some(other) => return Err(Error::UnsupportedScheme(other.to_string())),
        None => return Err(Error::InvalidUrl("URL has no scheme".into())),
    };
    let host = req
        .url
        .host()
        .ok_or_else(|| Error::InvalidUrl("URL has no host".into()))?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = req.url.port_u16().unwrap_or(if https { 443 } else { 80 });
    let verify_tls = !https || req.options.verify_tls;
    Ok(PoolKey {
        https,
        host,
        port,
        // Plain HTTP connections don't depend on TLS settings; keep them in one bucket.
        verify_tls,
        version: if https {
            req.options.http_version
        } else {
            HttpVersionPref::Http1
        },
        proxy,
        roots: req
            .options
            .extra_roots
            .as_ref()
            .filter(|r| https && verify_tls && !r.is_empty())
            .map(RootSet::id),
    })
}

/// Builds the hyper request for one attempt. `http_proxy` is set when a plain-HTTP
/// request goes through a proxy (absolute-form target, `Proxy-Authorization` from the
/// proxy settings); `tunnelled` when an HTTPS request goes through a `CONNECT` tunnel,
/// in which case a user-set `Proxy-Authorization` was sent to the proxy and must not
/// reach the origin.
fn build_request(
    req: &Request,
    protocol: Protocol,
    http_proxy: Option<&ProxyServer>,
    proxied: bool,
    sent: Arc<AtomicU64>,
) -> Result<http::Request<ReqBody>, Error> {
    // HTTP/1.1 uses origin-form ("/path?q") plus a Host header; HTTP/2 needs the
    // absolute URI so hyper can fill in :scheme and :authority. Through a plain-HTTP
    // proxy the absolute form goes on the request line.
    let uri = match protocol {
        Protocol::Http1_1 if http_proxy.is_none() => req
            .url
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .parse::<Uri>()
            .map_err(http::Error::from)?,
        _ => req.url.clone(),
    };
    let mut builder = http::Request::builder().method(req.method.clone()).uri(uri);
    let headers = builder.headers_mut().expect("builder has no errors yet");
    headers.extend(req.headers.clone());
    for (name, value) in engine_headers(req) {
        if name == HOST && protocol == Protocol::Http2 {
            continue;
        }
        headers.insert(name, value);
    }
    match http_proxy {
        Some(proxy) => {
            if let Some(auth) = &proxy.auth {
                headers.insert(PROXY_AUTHORIZATION, auth.header());
            }
        }
        None if proxied => {
            headers.remove(PROXY_AUTHORIZATION);
        }
        None => {}
    }
    Ok(builder.body(req.body.to_hyper(sent)?)?)
}

/// One-line request body summary for the timeline; `None` for no body.
fn body_summary(body: &Body, len: u64) -> Option<String> {
    match body {
        Body::Bytes(b) if b.is_empty() => None,
        Body::Bytes(_) => Some(format!("Request body: {len} bytes")),
        Body::File { path, .. } => Some(format!("Request body: file {} ({len} bytes)", path.display())),
        Body::Multipart(parts) => Some(format!(
            "Request body: multipart/form-data, {} part{} ({len} bytes)",
            parts.len(),
            if parts.len() == 1 { "" } else { "s" }
        )),
    }
}

/// `YYYY-MM-DD` (UTC) for a Unix timestamp, for the timeline.
fn unix_to_date(secs: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(secs)
        .map(|t| format!("{:04}-{:02}-{:02}", t.year(), u8::from(t.month()), t.day()))
        .unwrap_or_else(|_| secs.to_string())
}

/// Reads a redirect body so its HTTP/1.1 connection can be reused, or drops the
/// connection if the body is large or fails.
async fn drain_and_release(response: http::Response<Incoming>, lease: Lease<'_>) {
    let mut reader =
        decode::wire_reader(response.into_body(), Arc::default()).take(MAX_REDIRECT_BODY_DRAIN + 1);
    match tokio::io::copy(&mut reader, &mut tokio::io::sink()).await {
        Ok(n) if n <= MAX_REDIRECT_BODY_DRAIN => {
            drop(reader);
            lease.release();
        }
        _ => drop(lease),
    }
}

async fn copy_body<W>(
    reader: &mut (dyn AsyncRead + Send + Unpin),
    sink: &mut W,
    tracker: &Tracker<'_>,
    encodings: &str,
) -> Result<u64, Error>
where
    W: AsyncWrite + Unpin + Send + ?Sized,
{
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    let mut last_report = Instant::now();
    loop {
        let n = reader
            .read(&mut buf)
            .await
            .map_err(|e| read_error(e, encodings))?;
        if n == 0 {
            break;
        }
        sink.write_all(&buf[..n]).await.map_err(Error::Io)?;
        total += n as u64;
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            last_report = Instant::now();
            tracker.enter(Stage::Downloading, total);
        }
    }
    sink.flush().await.map_err(Error::Io)?;
    tracker.enter(Stage::Downloading, total);
    Ok(total)
}

/// Read errors are either transport errors (a `hyper::Error` wrapped by `wire_reader`)
/// or invalid compressed data.
fn read_error(err: std::io::Error, encodings: &str) -> Error {
    if err.get_ref().is_some_and(|inner| inner.is::<hyper::Error>()) {
        let inner = err.into_inner().expect("checked above");
        return Error::Http(*inner.downcast::<hyper::Error>().expect("checked above"));
    }
    if encodings.is_empty() {
        Error::Io(err)
    } else {
        Error::Decompress {
            encoding: encodings.to_string(),
            source: err,
        }
    }
}

/// Servers that support ALPN but none of our protocols reply with this alert (RFC 7301).
fn is_no_alpn_alert(err: &std::io::Error) -> bool {
    matches!(
        err.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()),
        Some(rustls::Error::AlertReceived(
            rustls::AlertDescription::NoApplicationProtocol
        ))
    )
}

async fn handshake<T>(io: T, h2: bool) -> Result<Sender, Error>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(io);
    if h2 {
        let (sender, conn) = http2::handshake(TokioExecutor::new(), io).await?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "HTTP/2 connection closed with error");
            }
        });
        Ok(Sender::H2(sender))
    } else {
        let (sender, conn) = http1::handshake(io).await?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "HTTP/1.1 connection closed with error");
            }
        });
        Ok(Sender::H1(sender))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_headers_fill_in_missing_defaults() {
        let mut req = Request::new(Method::POST, "https://api.test:8443/x").unwrap();
        req.body = Bytes::from_static(b"{}").into();
        let names: Vec<_> = engine_headers(&req)
            .into_iter()
            .map(|(n, v)| (n.to_string(), v))
            .collect();
        assert_eq!(
            names,
            vec![
                ("host".to_string(), HeaderValue::from_static("api.test:8443")),
                (
                    "user-agent".to_string(),
                    HeaderValue::from_static(DEFAULT_USER_AGENT)
                ),
                (
                    "accept-encoding".to_string(),
                    HeaderValue::from_static("gzip, deflate, br, zstd")
                ),
                ("content-length".to_string(), HeaderValue::from(2)),
            ]
        );
    }

    #[test]
    fn engine_headers_respect_user_headers_and_default_ports() {
        let mut req = Request::get("https://api.test/x").unwrap();
        req.headers.insert(USER_AGENT, HeaderValue::from_static("mine"));
        req.headers
            .insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let added = engine_headers(&req);
        assert_eq!(added, vec![(HOST, HeaderValue::from_static("api.test"))]);
    }

    #[test]
    fn empty_post_gets_content_length_zero_but_get_does_not() {
        let post = Request::new(Method::POST, "http://a.test/").unwrap();
        assert!(
            engine_headers(&post)
                .iter()
                .any(|(n, v)| n == CONTENT_LENGTH && v == "0")
        );
        let get = Request::get("http://a.test/").unwrap();
        assert!(!engine_headers(&get).iter().any(|(n, _)| n == CONTENT_LENGTH));
    }
}
