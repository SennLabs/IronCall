//! Sending requests (PLAN.md Phase 1): the IPC contract between the UI and the engine.
//!
//! The UI edits a [`RequestFile`] (the same shape that is saved to disk in Phase 2),
//! picks a `sendId`, and calls [`send_request`]. Progress arrives as [`SendProgress`]
//! events; [`cancel_send`] aborts. Response bodies are spooled to disk and registered
//! in the body store; the UI reads them through `get_body_text` (small bodies) or the
//! paged body commands in `body.rs` (any size).

use ironcall_core::assertions::AssertionResult;
use ironcall_core::checks::{self, Checked};
use ironcall_core::extract::ExtractedView;
use ironcall_core::model::RequestFile;
use ironcall_core::response::{BodyText, MAX_CHECK_BODY, ResponseData};
use ironcall_store::body::BodyId;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use crate::AppState;

/// Bodies up to this size can be fetched whole with `get_body_text` and shown in
/// CodeMirror; larger ones use the paged viewer (ADR 0003).
pub const SMALL_BODY_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HeaderRow {
    pub name: String,
    pub value: String,
}

/// Where a header in the request preview comes from.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HeaderSource {
    /// Typed by the user in the headers table.
    User,
    /// Derived from the request (e.g. Content-Type from the body type).
    Derived,
    /// Added by the engine at send time (Host, Content-Length, User-Agent, Accept-Encoding,
    /// Cookie from the jar).
    Engine,
    /// From the effective auth (Authorization or the API key header).
    Auth,
    /// A default header from the collection or a folder.
    Default,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewHeader {
    pub name: String,
    pub value: String,
    pub source: HeaderSource,
    /// False when a user header with the same name overrides this one.
    pub effective: bool,
    /// Where an auth or default header comes from, e.g. "Folder › Users".
    pub origin: Option<String>,
}

/// The request exactly as it will be sent (before variables, which arrive in Phase 3).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestPreview {
    pub method: String,
    /// Final URL after path-param substitution and encoding (PLAN.md §5).
    pub url: String,
    pub headers: Vec<PreviewHeader>,
    #[specta(type = Number)]
    pub body_size: u64,
}

/// Duration of a connection-setup phase, in milliseconds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TimingPhase {
    Measured { ms: f64 },
    Reused,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimingsInfo {
    pub dns: TimingPhase,
    pub connect: TimingPhase,
    pub tls: TimingPhase,
    pub ttfb_ms: f64,
    pub download_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RedirectHop {
    pub status: u16,
    pub url: String,
    pub timings: TimingsInfo,
}

/// How the UI should present a response body.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BodyKind {
    /// No body (e.g. 204, 304, HEAD).
    Empty,
    Json,
    Xml,
    Html,
    Text,
    Binary,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseBodyInfo {
    /// Body store ID of the decoded body, for `get_body_text` and the paged viewer.
    /// `None` for empty bodies.
    pub body_id: Option<BodyId>,
    pub kind: BodyKind,
    pub content_type: Option<String>,
    /// Charset used to decode text bodies to UTF-8 (e.g. "utf-8", "windows-1252").
    pub charset: Option<String>,
    /// True if decoding hit invalid bytes and used replacement characters.
    pub charset_lossy: bool,
    /// Decoded size in bytes (after decompression and charset conversion).
    #[specta(type = Number)]
    pub size: u64,
    /// Size on the wire (before decompression).
    #[specta(type = Number)]
    pub wire_size: u64,
    #[specta(type = Number)]
    pub row_count: usize,
    /// True if the kind can be pretty-printed and the body is at most `SMALL_BODY_LIMIT`.
    pub pretty_available: bool,
}

/// The server's leaf certificate (PLAN.md Phase 5 TLS summary).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    /// RFC 3339.
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    /// DNS names and IP addresses from the subjectAltName extension.
    pub sans: Vec<String>,
    /// Number of certificates the server sent (leaf plus intermediates).
    pub chain_length: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TlsInfo {
    /// e.g. "TLSv1.3".
    pub version: String,
    /// e.g. "TLS13_AES_128_GCM_SHA256".
    pub cipher: String,
    /// Negotiated ALPN protocol, e.g. "h2".
    pub alpn: Option<String>,
    pub certificate: Option<CertificateInfo>,
    /// False if verification was turned off for this request.
    pub verified: bool,
}

/// An informational (1xx) response received before the final one, e.g. 103 Early Hints.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InformationalResponse {
    pub status: u16,
    pub headers: Vec<HeaderRow>,
    /// Milliseconds after the request was sent.
    pub at_ms: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TimelineKind {
    /// General progress (resolving, connected, reusing connection).
    Info,
    /// TLS handshake details.
    Tls,
    /// Proxy CONNECT and its response.
    Proxy,
    /// Request line and headers as sent (sensitive values masked).
    RequestHeaders,
    /// Request body summary.
    RequestBody,
    /// Status line and headers received.
    ResponseHeaders,
    /// Response body summary.
    ResponseBody,
    Redirect,
    Error,
}

/// One line of the per-send console, like `curl -v` (PLAN.md Phase 5).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// Milliseconds since the send started.
    pub at_ms: f64,
    pub kind: TimelineKind,
    /// One or more lines of text (headers are one per line).
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSummary {
    pub send_id: String,
    pub status: u16,
    pub status_text: String,
    /// "HTTP/1.1" or "HTTP/2".
    pub http_version: String,
    pub headers: Vec<HeaderRow>,
    pub body: ResponseBodyInfo,
    pub timings: TimingsInfo,
    /// Hops before the final response, in order. Empty if there were no redirects.
    pub redirects: Vec<RedirectHop>,
    /// The final request as sent, including engine-added headers.
    pub request: RequestPreview,
    pub connection_reused: bool,
    /// TLS details of the final connection; `None` for plain HTTP.
    pub tls: Option<TlsInfo>,
    /// 1xx responses before the final one. TTFB is measured to the final headers.
    pub informational: Vec<InformationalResponse>,
    pub timeline: Vec<TimelineEntry>,
    /// ID of the history entry recorded for this send.
    pub history_id: Option<String>,
    /// Assertion and extraction results; `None` if the request has neither.
    pub checks: Option<ResponseChecks>,
}

/// Results of a request's assertions and extractions (PLAN.md Phase 7).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseChecks {
    pub assertions: Vec<AssertionResult>,
    /// Sensitive values are masked.
    pub extracted: Vec<ExtractedView>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SendErrorKind {
    InvalidRequest,
    InvalidUrl,
    Dns,
    Connect,
    Tls,
    Timeout,
    Cancelled,
    TooManyRedirects,
    Protocol,
    Io,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SendError {
    pub kind: SendErrorKind,
    /// One-line, user-facing message.
    pub message: String,
    /// Underlying cause chain, for an expandable "details" section.
    pub detail: Option<String>,
    /// What happened before the failure (empty for errors before sending).
    pub timeline: Vec<TimelineEntry>,
    pub history_id: Option<String>,
}

impl SendError {
    pub(crate) fn new(kind: SendErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
            timeline: Vec::new(),
            history_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SendStage {
    Connecting,
    /// Sending the request body (file and multipart bodies are streamed from disk).
    Uploading,
    Waiting,
    Downloading,
}

/// Emitted while a send is in flight.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub send_id: String,
    pub stage: SendStage,
    /// Decoded body bytes received so far.
    #[specta(type = Number)]
    pub bytes: u64,
    /// Request body bytes sent so far, and the total if known.
    #[specta(type = Number)]
    pub uploaded: u64,
    #[specta(type = Option<Number>)]
    pub upload_total: Option<u64>,
}

/// A redacted summary of a recent send (PLAN.md Phase 1 "recent sends" log).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentSend {
    pub send_id: String,
    /// RFC 3339 timestamp.
    pub at: String,
    pub method: String,
    /// URL with sensitive query values masked.
    pub url: String,
    pub status: Option<u16>,
    pub error: Option<SendErrorKind>,
    pub total_ms: Option<f64>,
}

/// Returns a new, empty scratch request with a fresh ID.
#[tauri::command]
#[specta::specta]
pub fn new_scratch_request() -> RequestFile {
    RequestFile::new("Untitled request")
}

/// Resolves a request (variables included) into exactly what would be sent, without
/// sending it. Secret values are masked as `••••` unless `revealSecrets`.
#[tauri::command]
#[specta::specta]
pub fn preview_request(
    state: tauri::State<'_, AppState>,
    request: RequestFile,
    reveal_secrets: bool,
) -> Result<RequestPreview, SendError> {
    let prepared = crate::network::prepare(&state, &request);
    // Network options only matter for the engine's headers (the jar's Cookie); a broken
    // proxy setting shouldn't stop the preview.
    let net = crate::network::net_options(&state, &prepared).ok();
    state.sender.preview(
        &prepared.resolved.request,
        &prepared.ctx,
        net.as_ref(),
        &prepared.resolved.secrets,
        reveal_secrets,
    )
}

/// Sends a request. `send_id` is chosen by the UI so it can cancel before this returns.
#[tauri::command]
#[specta::specta]
pub async fn send_request(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    send_id: String,
    request: RequestFile,
) -> Result<ResponseSummary, SendError> {
    let (result, checked) = send_in(
        &state,
        &app,
        send_id,
        &request,
        &crate::vars::Scope::Session,
        None,
    )
    .await;
    if let Some(checked) = checked {
        state.runs.apply_to_session(&state, &checked.extracted);
    }
    result
}

/// Sends `request` with variables from `scope`, records it in history, then runs its
/// assertions and extractions. The caller applies the extracted values to its scope.
/// Cancelled or failed sends are never checked.
pub(crate) async fn send_in<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    send_id: String,
    request: &RequestFile,
    scope: &crate::vars::Scope,
    cancel_parent: Option<&ironcall_http::CancellationToken>,
) -> (Result<ResponseSummary, SendError>, Option<Checked>) {
    // Cancellable from here on, even while secrets are looked up (a keychain prompt).
    let _reservation = state.sender.begin(&send_id, cancel_parent);
    let result = send_and_record(state, app, send_id, request, scope).await;
    match result {
        Ok((mut summary, resolver)) => {
            if request.assert.is_empty() && request.extract.is_empty() {
                return (Ok(summary), None);
            }
            let response = response_data(state, request, &summary);
            let checked =
                checks::check_response(request, &resolver, &response, &state.network.sensitive_headers());
            summary.checks = Some(ResponseChecks {
                assertions: checked.assertions.clone(),
                extracted: checked.extracted.iter().map(|e| e.view()).collect(),
            });
            (Ok(summary), Some(checked))
        }
        Err(e) => (Err(e), None),
    }
}

/// What assertions and extraction see of a response. The body is read only if a check
/// needs it, and only up to [`MAX_CHECK_BODY`].
fn response_data(state: &AppState, request: &RequestFile, s: &ResponseSummary) -> ResponseData {
    let body = if !checks::needs_body(request) {
        BodyText::Unavailable("the body wasn't loaded".into())
    } else if s.body.kind == BodyKind::Binary {
        BodyText::Unavailable("the body is binary".into())
    } else if s.body.size > MAX_CHECK_BODY {
        BodyText::Unavailable(format!(
            "the body is {} MB; assertions and extraction read at most {} MB",
            s.body.size / (1024 * 1024),
            MAX_CHECK_BODY / (1024 * 1024)
        ))
    } else {
        match s.body.body_id {
            None => BodyText::Text(String::new()),
            Some(id) => state
                .bodies
                .get(id)
                .and_then(|b| b.read_range(0, s.body.size).ok())
                .map(|b| BodyText::Text(String::from_utf8_lossy(&b).into_owned()))
                .unwrap_or_else(|| BodyText::Unavailable("the body is no longer available".into())),
        }
    };
    let headers = s
        .headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect();
    ResponseData::new(s.status, headers, s.timings.total_ms, s.body.size, body)
}

async fn send_and_record<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    send_id: String,
    request: &RequestFile,
    scope: &crate::vars::Scope,
) -> Result<(ResponseSummary, ironcall_core::vars::Resolver), SendError> {
    let started = std::time::SystemTime::now();
    let prepared = crate::network::prepare_in(state, request, true, scope);
    if prepared.locked && !prepared.resolved.unresolved.is_empty() {
        return Err(SendError::new(
            SendErrorKind::InvalidRequest,
            "This request uses secrets from the encrypted secrets file, which is locked. Unlock it in Secrets settings.",
        ));
    }
    // Uploads must be inside the workspace or picked this session (see network.rs).
    for file in crate::network::check_body_files(state, &prepared) {
        if !file.exists {
            return Err(SendError::new(
                SendErrorKind::InvalidRequest,
                format!("The file to upload doesn't exist: {}", file.path),
            ));
        }
        if !file.allowed {
            return Err(SendError::new(
                SendErrorKind::InvalidRequest,
                format!(
                    "{} is outside the workspace. Select it again in the body editor to allow uploading it this session.",
                    file.path
                ),
            ));
        }
    }
    let net = crate::network::net_options(state, &prepared)
        .map_err(|e| SendError::new(SendErrorKind::InvalidRequest, e))?;
    let jar_key = net.jar_key.clone();
    let result = state
        .sender
        .send(
            app,
            send_id,
            prepared.resolved.request,
            &prepared.ctx,
            net,
            prepared.resolved.secrets,
        )
        .await;
    // Responses (including failed redirect chains) may have set cookies.
    state.network.save_jar(&jar_key);
    let mut result = result;
    let history_id = crate::history::record(
        state,
        crate::history::Finished {
            at: started,
            authored: request,
            outcome: &result,
        },
    )
    .await;
    match &mut result {
        Ok(summary) => summary.history_id = history_id,
        Err(err) => err.history_id = history_id,
    }
    result.map(|summary| (summary, prepared.resolver))
}

/// Cancels an in-flight send. Does nothing if it already finished.
#[tauri::command]
#[specta::specta]
pub fn cancel_send(state: tauri::State<'_, AppState>, send_id: String) {
    state.sender.cancel(&send_id);
}

/// Returns a small body's full text (at most `SMALL_BODY_LIMIT` bytes), optionally
/// pretty-printed.
#[tauri::command]
#[specta::specta]
pub async fn get_body_text(
    state: tauri::State<'_, AppState>,
    body_id: BodyId,
    pretty: bool,
) -> Result<String, String> {
    let sender = state.sender.clone();
    tauri::async_runtime::spawn_blocking(move || sender.body_text(body_id, pretty))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DerivedBody {
    pub body_id: BodyId,
    #[specta(type = Number)]
    pub size: u64,
    #[specta(type = Number)]
    pub row_count: usize,
}

/// Pretty-prints a JSON or XML body of any size into a new body (streamed in Rust, on a
/// background thread). Used by the paged viewer for bodies over `SMALL_BODY_LIMIT`.
/// Close the returned body when done, like any other.
#[tauri::command]
#[specta::specta]
pub async fn body_pretty(state: tauri::State<'_, AppState>, body_id: BodyId) -> Result<DerivedBody, String> {
    let sender = state.sender.clone();
    let (body_id, size, row_count) =
        tauri::async_runtime::spawn_blocking(move || sender.pretty_body(body_id))
            .await
            .map_err(|e| e.to_string())??;
    Ok(DerivedBody {
        body_id,
        size,
        row_count,
    })
}

/// Opens a native "Save as" dialog and writes the response body there (the decoded,
/// decompressed bytes as received; text is not charset-converted). Returns the path,
/// or `None` if cancelled.
#[tauri::command]
#[specta::specta]
pub async fn body_save_as(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    body_id: BodyId,
    suggested_name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let source = state.sender.raw_path(body_id).ok_or("unknown body")?;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(suggested_name)
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(target) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    let shown = target.display().to_string();
    tauri::async_runtime::spawn_blocking(move || std::fs::copy(&source, &target))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not save to {shown}: {e}"))?;
    Ok(Some(shown))
}

/// The last 50 sends, newest first, with sensitive values redacted.
#[tauri::command]
#[specta::specta]
pub fn recent_sends(state: tauri::State<'_, AppState>) -> Vec<RecentSend> {
    state.sender.recent()
}
