//! Request history (PLAN.md Phase 5): the IPC contract.
//!
//! Every send is recorded in a local SQLite database in the app data dir (never in the
//! workspace). An entry keeps:
//! - the request **as authored** (with `{{variables}}`, so it can be reopened and sent
//!   again); secret values never appear because secrets are only ever variables, and
//!   sensitive literal values (denylisted headers, literal auth credentials) are masked;
//! - the request **as sent** (resolved), with secrets and sensitive headers masked;
//! - response metadata (status, headers with sensitive values masked, timings, TLS);
//! - the response body up to the configured cap, unless the workspace turned body
//!   storage off.

use std::sync::Arc;
use std::time::SystemTime;

use ironcall_core::ItemId;
use ironcall_core::model::RequestFile;
use ironcall_core::recent::rfc3339_utc;
use ironcall_core::{content, redact};
use ironcall_store::history::{self as store, EntryInput, History, StoredBody};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use crate::AppState;
use crate::send::{
    BodyKind, HeaderRow, RequestPreview, ResponseBodyInfo, ResponseSummary, SMALL_BODY_LIMIT, SendError,
    SendErrorKind, TimingsInfo,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StatusClass {
    /// 1xx–3xx (the final status).
    Success,
    Redirect,
    ClientError,
    ServerError,
    /// No response (DNS, connect, TLS, timeout, cancelled…).
    Failed,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    /// Full-text search over method, URL, request/response headers and names
    /// (not bodies). Empty matches everything.
    pub text: String,
    /// Only this workspace's sends; `None` for all, including scratch sends made with no
    /// workspace open.
    pub workspace: Option<ItemId>,
    pub methods: Vec<String>,
    pub status: Vec<StatusClass>,
    /// Paging: entries strictly older than this cursor (from a previous page).
    pub before: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistorySummary {
    /// ULID; sortable by time.
    pub id: String,
    /// RFC 3339.
    pub at: String,
    pub workspace: Option<ItemId>,
    pub workspace_name: Option<String>,
    /// The request's ID (workspace or scratch), to jump back to it.
    pub request_id: Option<ItemId>,
    pub request_name: String,
    pub method: String,
    /// As sent, with sensitive query values masked.
    pub url: String,
    pub status: Option<u16>,
    pub error: Option<SendErrorKind>,
    pub total_ms: Option<f64>,
    #[specta(type = Option<Number>)]
    pub size: Option<u64>,
    /// Whether a response body was stored.
    pub has_body: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub entries: Vec<HistorySummary>,
    /// Pass as `before` to get the next page; `None` at the end.
    pub next: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryResponse {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    /// Sensitive values (Set-Cookie, …) masked.
    pub headers: Vec<HeaderRow>,
    pub timings: TimingsInfo,
    pub body_kind: BodyKind,
    pub content_type: Option<String>,
    #[specta(type = Number)]
    pub size: u64,
    /// True if the body was larger than the cap and only the first part was stored.
    pub body_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub summary: HistorySummary,
    /// The request as authored (masked as described above). Open it in a scratch tab to
    /// send it again.
    pub request: RequestFile,
    /// The request as sent (resolved, masked).
    pub sent: Option<RequestPreview>,
    pub response: Option<HistoryResponse>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistorySettings {
    /// Keep at most this many entries (oldest removed first).
    pub max_entries: u32,
    pub max_age_days: u32,
    /// Total size cap for stored bodies, in MB.
    pub max_total_mb: u32,
    /// Per-body cap, in KB; larger bodies are stored truncated.
    pub max_body_kb: u32,
}

impl Default for HistorySettings {
    /// PLAN.md §12: 1MB per body, 500MB total, 90 days.
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_age_days: 90,
            max_total_mb: 500,
            max_body_kb: 1024,
        }
    }
}

type Result<T> = std::result::Result<T, String>;

// ---------------------------------------------------------------------------------
// Recording

/// What the app knows about a send when it finishes.
pub(crate) struct Finished<'a> {
    pub at: SystemTime,
    /// The request as authored (before variables), from the tab.
    pub authored: &'a RequestFile,
    pub outcome: &'a std::result::Result<ResponseSummary, SendError>,
}

fn kind_name(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Empty => "empty",
        BodyKind::Json => "json",
        BodyKind::Xml => "xml",
        BodyKind::Html => "html",
        BodyKind::Text => "text",
        BodyKind::Binary => "binary",
    }
}

fn kind_from_name(name: &str) -> BodyKind {
    match name {
        "json" => BodyKind::Json,
        "xml" => BodyKind::Xml,
        "html" => BodyKind::Html,
        "text" => BodyKind::Text,
        "empty" => BodyKind::Empty,
        _ => BodyKind::Binary,
    }
}

fn core_kind(kind: BodyKind) -> content::BodyKind {
    match kind {
        BodyKind::Empty => content::BodyKind::Empty,
        BodyKind::Json => content::BodyKind::Json,
        BodyKind::Xml => content::BodyKind::Xml,
        BodyKind::Html => content::BodyKind::Html,
        BodyKind::Text => content::BodyKind::Text,
        BodyKind::Binary => content::BodyKind::Binary,
    }
}

fn masked_rows(rows: &[HeaderRow], sensitive: &[String]) -> Vec<HeaderRow> {
    rows.iter()
        .map(|h| HeaderRow {
            name: h.name.clone(),
            value: redact::header_value(&h.name, &h.value, sensitive).to_string(),
        })
        .collect()
}

fn headers_text(rows: impl IntoIterator<Item = (String, String)>) -> String {
    rows.into_iter().map(|(n, v)| format!("{n}: {v}\n")).collect()
}

/// Records a finished send in history and returns the entry ID. Never fails the send:
/// problems are logged.
pub(crate) async fn record(state: &AppState, finished: Finished<'_>) -> Option<String> {
    let settings = state.network.history_settings();
    let (workspace_id, workspace_name, store_bodies) = match state.workspaces.current() {
        Ok(ws) => {
            let ws = ws.lock().expect("workspace lock poisoned");
            (
                Some(ws.id()),
                Some(ws.name().to_string()),
                ws.load_local().stores_history_bodies(),
            )
        }
        Err(_) => (None, None, true),
    };
    let sensitive = state.network.sensitive_headers();
    let authored = redact::redact_request_file(finished.authored, &sensitive);
    let mut input = EntryInput {
        at: finished.at,
        workspace_id,
        workspace_name,
        request_id: Some(finished.authored.id),
        request_name: finished.authored.name.clone(),
        method: finished.authored.method.to_ascii_uppercase(),
        request_authored: serde_json::to_string(&authored).ok()?,
        ..EntryInput::default()
    };

    match finished.outcome {
        Ok(summary) => {
            // `summary.request` is already masked for secrets; mask sensitive headers too.
            let mut sent = summary.request.clone();
            for h in &mut sent.headers {
                h.value = redact::header_value(&h.name, &h.value, &sensitive).to_string();
            }
            let response = HistoryResponse {
                status: summary.status,
                status_text: summary.status_text.clone(),
                http_version: summary.http_version.clone(),
                headers: masked_rows(&summary.headers, &sensitive),
                timings: summary.timings,
                body_kind: summary.body.kind,
                content_type: summary.body.content_type.clone(),
                size: summary.body.size,
                body_truncated: false,
            };
            input.method = sent.method.clone();
            input.url = redact::redact_url(&sent.url);
            input.status = Some(summary.status);
            input.total_ms = Some(summary.timings.total_ms);
            input.size = Some(summary.body.size);
            input.request_headers_text =
                headers_text(sent.headers.iter().map(|h| (h.name.clone(), h.value.clone())));
            input.response_headers_text =
                headers_text(response.headers.iter().map(|h| (h.name.clone(), h.value.clone())));

            let cap = u64::from(settings.max_body_kb) * 1024;
            let body = if store_bodies {
                summary
                    .body
                    .body_id
                    .and_then(|id| state.sender.body_prefix(id, cap))
            } else {
                None
            };
            let truncated = body.as_ref().is_some_and(|(_, t)| *t);
            input.body = body.map(|(bytes, truncated)| StoredBody {
                bytes,
                truncated,
                kind: kind_name(summary.body.kind).into(),
                content_type: summary.body.content_type.clone(),
            });
            input.request_sent = serde_json::to_string(&sent).ok();
            input.response_meta = serde_json::to_string(&HistoryResponse {
                body_truncated: truncated,
                ..response
            })
            .ok();
        }
        Err(err) => {
            input.error_kind = serde_json::to_value(err.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
            input.error_message = Some(err.message.clone());
            input.url = redact::redact_url(&finished.authored.url);
        }
    }

    let history = state.history.clone();
    let retention = store::Retention {
        max_entries: settings.max_entries,
        max_age_days: settings.max_age_days,
        max_total_body_bytes: u64::from(settings.max_total_mb) * 1024 * 1024,
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        let id = history.insert(&input)?;
        history.apply_retention(retention)?;
        Ok::<_, store::HistoryError>(id)
    })
    .await;
    match result {
        Ok(Ok(id)) => Some(id),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not record history");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "history task failed");
            None
        }
    }
}

// ---------------------------------------------------------------------------------
// Commands

fn status_class(c: StatusClass) -> store::StatusClass {
    match c {
        StatusClass::Success => store::StatusClass::Success,
        StatusClass::Redirect => store::StatusClass::Redirect,
        StatusClass::ClientError => store::StatusClass::ClientError,
        StatusClass::ServerError => store::StatusClass::ServerError,
        StatusClass::Failed => store::StatusClass::Failed,
    }
}

fn summary(row: store::SummaryRow) -> HistorySummary {
    HistorySummary {
        id: row.id,
        at: rfc3339_utc(row.at),
        workspace: row.workspace_id,
        workspace_name: row.workspace_name,
        request_id: row.request_id,
        request_name: row.request_name,
        method: row.method,
        url: row.url,
        status: row.status,
        error: row
            .error_kind
            .and_then(|k| serde_json::from_value(serde_json::Value::String(k)).ok()),
        total_ms: row.total_ms,
        size: row.size,
        has_body: row.has_body,
    }
}

async fn blocking<T: Send + 'static>(
    history: Arc<History>,
    f: impl FnOnce(&History) -> std::result::Result<T, store::HistoryError> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || f(&history))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn history_list(state: tauri::State<'_, AppState>, query: HistoryQuery) -> Result<HistoryPage> {
    let q = store::Query {
        text: query.text,
        workspace: query.workspace,
        methods: query.methods,
        status: query.status.into_iter().map(status_class).collect(),
        before: query.before,
        limit: query.limit.clamp(1, 500),
    };
    let (rows, next) = blocking(state.history.clone(), move |h| h.list(&q)).await?;
    Ok(HistoryPage {
        entries: rows.into_iter().map(summary).collect(),
        next,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn history_get(state: tauri::State<'_, AppState>, id: String) -> Result<HistoryEntry> {
    let row = blocking(state.history.clone(), move |h| h.get(&id))
        .await?
        .ok_or("no such history entry")?;
    Ok(HistoryEntry {
        request: serde_json::from_str(&row.request_authored).map_err(|e| e.to_string())?,
        sent: row.request_sent.and_then(|s| serde_json::from_str(&s).ok()),
        response: row.response_meta.and_then(|s| serde_json::from_str(&s).ok()),
        error_message: row.error_message,
        summary: summary(row.summary),
    })
}

/// Loads a stored response body into the body store, for viewing with the usual body
/// commands. `None` if the entry has no stored body.
#[tauri::command]
#[specta::specta]
pub async fn history_open_body(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<ResponseBodyInfo>> {
    let Some(stored) = blocking(state.history.clone(), move |h| h.body(&id)).await? else {
        return Ok(None);
    };
    let kind = kind_from_name(&stored.kind);
    let media_type = stored
        .content_type
        .as_deref()
        .and_then(|c| c.split(';').next())
        .map(|t| t.trim().to_ascii_lowercase());
    let (body_id, size, row_count) = state
        .sender
        .open_stored_body(&stored.bytes, core_kind(kind), media_type)
        .map_err(|e| e.to_string())?;
    Ok(Some(ResponseBodyInfo {
        body_id: Some(body_id),
        kind,
        content_type: stored.content_type,
        charset: Some("utf-8".into()),
        charset_lossy: false,
        size,
        wire_size: size,
        row_count,
        pretty_available: core_kind(kind).can_pretty_print() && size <= SMALL_BODY_LIMIT,
    }))
}

#[tauri::command]
#[specta::specta]
pub async fn history_delete(state: tauri::State<'_, AppState>, id: String) -> Result<()> {
    blocking(state.history.clone(), move |h| h.delete(&id).map(|_| ())).await
}

/// Deletes every entry, or only one workspace's (`Some(id)`).
#[tauri::command]
#[specta::specta]
pub async fn history_clear(state: tauri::State<'_, AppState>, workspace: Option<ItemId>) -> Result<()> {
    blocking(state.history.clone(), move |h| h.clear(workspace).map(|_| ())).await
}

#[tauri::command]
#[specta::specta]
pub async fn history_settings_get(state: tauri::State<'_, AppState>) -> Result<HistorySettings> {
    Ok(state.network.history_settings())
}

/// Saves the settings and applies retention immediately.
#[tauri::command]
#[specta::specta]
pub async fn history_settings_set(
    state: tauri::State<'_, AppState>,
    settings: HistorySettings,
) -> Result<()> {
    state.network.save_history_settings(&settings)?;
    let retention = store::Retention {
        max_entries: settings.max_entries,
        max_age_days: settings.max_age_days,
        max_total_body_bytes: u64::from(settings.max_total_mb) * 1024 * 1024,
    };
    blocking(state.history.clone(), move |h| {
        h.apply_retention(retention).map(|_| ())
    })
    .await
}

/// Whether response bodies from the open workspace are stored in history (per machine,
/// in `.ironcall/local.yaml`; default true).
#[tauri::command]
#[specta::specta]
pub async fn workspace_history_bodies_get(state: tauri::State<'_, AppState>) -> Result<bool> {
    let ws = state.workspaces.current().map_err(|e| e.message)?;
    let ws = ws.lock().expect("workspace lock poisoned");
    Ok(ws.load_local().stores_history_bodies())
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_history_bodies_set(
    state: tauri::State<'_, AppState>,
    store_bodies: bool,
) -> Result<()> {
    let ws = state.workspaces.current().map_err(|e| e.message)?;
    let ws = ws.lock().expect("workspace lock poisoned");
    let mut local = ws.load_local();
    local.history_bodies = Some(store_bodies);
    ws.save_local(&local).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::Router;
    use axum::http::header;
    use axum::routing::get;
    use ironcall_core::model::KeyValue;
    use ironcall_core::wire::WireContext;

    use super::*;
    use crate::network::NetOptions;
    use crate::test_support::fixture;

    async fn serve() -> SocketAddr {
        let app = Router::new().route(
            "/users",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::SET_COOKIE, "session=server-secret-cookie; Path=/"),
                    ],
                    r#"{"users":[{"id":1,"name":"Ada"},{"id":2,"name":"Grace"}]}"#,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
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

    #[tokio::test(flavor = "multi_thread")]
    async fn sends_are_recorded_redacted_and_reopenable() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;

        // As authored: a literal credential (bad practice, but it happens) and a variable.
        let mut authored = RequestFile::new("List users");
        authored.url = format!("http://{addr}/users");
        authored.headers = vec![
            KeyValue::new("Authorization", "Bearer literal-secret-123"),
            KeyValue::new("X-Token", "{{tok}}"),
        ];
        let mut sent_req = authored.clone();
        sent_req.headers[1].value = "resolved-token".into();

        let outcome = state
            .sender
            .send(
                f.app.handle(),
                "h1".into(),
                sent_req,
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await;
        let summary = outcome.as_ref().unwrap();
        assert!(!summary.timeline.is_empty());
        let timeline: String = summary.timeline.iter().map(|t| t.text.as_str()).collect();
        assert!(
            !timeline.contains("literal-secret-123"),
            "timeline masks Authorization"
        );

        let id = record(
            state,
            Finished {
                at: SystemTime::now(),
                authored: &authored,
                outcome: &outcome,
            },
        )
        .await
        .expect("recorded");

        let row = state.history.get(&id).unwrap().unwrap();
        let everything = format!(
            "{} {} {}",
            row.request_authored,
            row.request_sent.clone().unwrap_or_default(),
            row.response_meta.clone().unwrap_or_default()
        );
        assert!(
            !everything.contains("literal-secret-123"),
            "literal credential masked"
        );
        assert!(!everything.contains("server-secret-cookie"), "Set-Cookie masked");
        assert!(
            row.request_authored.contains("{{tok}}"),
            "variables kept so it can be resent"
        );

        // Searchable, and the body was stored and can be reopened.
        let (rows, _) = state
            .history
            .list(&store::Query {
                text: "users".into(),
                limit: 10,
                ..store::Query::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].has_body);
        let stored = state.history.body(&id).unwrap().unwrap();
        let (body_id, _, _) = state
            .sender
            .open_stored_body(
                &stored.bytes,
                content::BodyKind::Json,
                Some("application/json".into()),
            )
            .unwrap();
        assert!(state.sender.body_text(body_id, false).unwrap().contains("Grace"));

        // Large-body pretty-printing produces a new, multi-line body.
        let (pretty_id, _, rows) = state.sender.pretty_body(summary.body.body_id.unwrap()).unwrap();
        assert!(rows > 5, "pretty output has {rows} rows");
        assert!(
            state
                .sender
                .body_text(pretty_id, false)
                .unwrap()
                .contains("\n  \"users\": [")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_can_turn_off_body_storage() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;
        {
            let ws = state.workspaces.current().unwrap();
            let ws = ws.lock().unwrap();
            let mut local = ws.load_local();
            local.history_bodies = Some(false);
            ws.save_local(&local).unwrap();
        }
        let mut req = RequestFile::new("x");
        req.url = format!("http://{addr}/users");
        let outcome = state
            .sender
            .send(
                f.app.handle(),
                "h2".into(),
                req.clone(),
                &WireContext::default(),
                net(),
                Vec::new(),
            )
            .await;
        let id = record(
            state,
            Finished {
                at: SystemTime::now(),
                authored: &req,
                outcome: &outcome,
            },
        )
        .await
        .unwrap();
        assert!(state.history.body(&id).unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn user_configured_sensitive_headers_are_masked() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;
        let mut g = state.network.general_settings();
        g.sensitive_headers = vec!["X-Session".into()];
        state.network.save_general_settings(&g).unwrap();

        let mut req = RequestFile::new("Custom header");
        req.url = format!("http://{addr}/users");
        req.headers = vec![KeyValue::new("X-Session", "custom-secret-42")];
        let outcome = state
            .sender
            .send(
                f.app.handle(),
                "h9".into(),
                req.clone(),
                &WireContext::default(),
                NetOptions {
                    sensitive_headers: state.network.sensitive_headers(),
                    ..net()
                },
                Vec::new(),
            )
            .await;
        let timeline: String = outcome
            .as_ref()
            .unwrap()
            .timeline
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        assert!(!timeline.contains("custom-secret-42"), "timeline masks it");
        let id = record(
            state,
            Finished {
                at: SystemTime::now(),
                authored: &req,
                outcome: &outcome,
            },
        )
        .await
        .unwrap();
        let row = format!("{:?}", state.history.get(&id).unwrap().unwrap());
        assert!(!row.contains("custom-secret-42"), "history masks it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_keys_under_custom_names_are_masked() {
        let addr = serve().await;
        let f = fixture();
        let state = &f.state;
        let mut req = RequestFile::new("Custom API key");
        req.url = format!("http://{addr}/users");
        for (location, name) in [
            (ironcall_core::model::ApiKeyLocation::Header, "X-Goog-Api-Key"),
            (ironcall_core::model::ApiKeyLocation::Query, "subscription-key"),
        ] {
            req.auth = ironcall_core::model::Auth::ApiKey {
                key: name.into(),
                value: "gk-literal-123456".into(),
                location,
                extra: Default::default(),
            };
            let (result, _) = crate::send::send_in(
                state,
                f.app.handle(),
                format!("k-{name}"),
                &req,
                &crate::vars::Scope::Session,
                None,
            )
            .await;
            let summary = result.unwrap();
            let timeline: String = summary.timeline.iter().map(|t| t.text.as_str()).collect();
            assert!(!timeline.contains("gk-literal-123456"), "{name}: timeline");
            let row = format!(
                "{:?}",
                state
                    .history
                    .get(summary.history_id.as_deref().unwrap())
                    .unwrap()
                    .unwrap()
            );
            assert!(!row.contains("gk-literal-123456"), "{name}: history");
        }
    }
}
