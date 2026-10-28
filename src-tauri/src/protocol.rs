//! Custom URI schemes for response bodies (PLAN.md §3.1, Phase 5 previews).
//!
//! - `ironcall`: byte ranges of open bodies, with the body's real media type so `<img>`
//!   and pdf.js can use it. Tauri scheme handlers return a whole response and cannot
//!   stream, so large bodies are read with `Range` requests answered by `206` slices.
//!   HTML (and anything script-capable) is served as `text/plain` here: this origin may
//!   be fetched by the app but must never render active content.
//! - `ironcall-preview`: HTML bodies only, for the sandboxed preview iframe. Responses
//!   carry `Content-Security-Policy: default-src 'none'; …`, so a previewed page can't
//!   load anything from the network (no tracking pixels, no IP leaks).
//!
//! URLs: `ironcall://localhost/body/{id}` on Linux, `http://ironcall.localhost/body/{id}`
//! on Windows (same pattern for `ironcall-preview`).

use ironcall_core::content::BodyKind;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::AppState;

/// CSP for previewed HTML: inline styles and data: images only.
const PREVIEW_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'";

/// Media types the `ironcall` scheme may serve as-is (they can't run script there).
fn safe_media_type(media_type: &str) -> bool {
    media_type.starts_with("image/") && media_type != "image/svg+xml"
        || media_type.starts_with("audio/")
        || media_type.starts_with("video/")
        || matches!(
            media_type,
            "application/pdf" | "application/json" | "application/octet-stream"
        )
}

/// Bodies up to this size may be fetched without a `Range` header.
const MAX_FULL_RESPONSE: u64 = 32 * 1024 * 1024;
/// Largest slice served for one `Range` request.
const MAX_RANGE: u64 = 16 * 1024 * 1024;

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    serve(ctx, request, responder, respond);
}

/// The `ironcall-preview` scheme (HTML previews).
pub fn handle_preview<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    serve(ctx, request, responder, respond_preview);
}

fn serve<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
    respond: Responder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let response = respond(&app.state::<AppState>(), &request).unwrap_or_else(|(status, msg)| {
            tracing::debug!(%status, uri = %request.uri(), "ironcall scheme error: {msg}");
            with_cors(Response::builder().status(status))
                .header(header::CONTENT_TYPE, "text/plain")
                .body(msg.into_bytes())
                .expect("valid error response")
        });
        responder.respond(response);
    });
}

fn with_cors(builder: tauri::http::response::Builder) -> tauri::http::response::Builder {
    builder
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Range")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Range, Content-Length, Accept-Ranges",
        )
}

type Failure = (StatusCode, String);
type Responder = fn(&AppState, &Request<Vec<u8>>) -> Result<Response<Vec<u8>>, Failure>;

fn body_id(request: &Request<Vec<u8>>) -> Result<ironcall_store::body::BodyId, Failure> {
    request
        .uri()
        .path()
        .strip_prefix("/body/")
        .and_then(|id| id.parse().ok())
        .ok_or((StatusCode::NOT_FOUND, "expected /body/{id}".into()))
}

/// The `Content-Type` to serve a body with on the `ironcall` scheme.
fn served_type(state: &AppState, id: ironcall_store::body::BodyId) -> String {
    let Some(info) = state.sender.serve_info(id) else {
        return "application/octet-stream".into();
    };
    match info.media_type {
        Some(t) if safe_media_type(&t) => t,
        // Text bodies are stored as UTF-8 (transcoded on receipt).
        _ if matches!(
            info.kind,
            BodyKind::Json | BodyKind::Xml | BodyKind::Html | BodyKind::Text
        ) =>
        {
            "text/plain; charset=utf-8".into()
        }
        _ => "application/octet-stream".into(),
    }
}

fn respond_preview(state: &AppState, request: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>, Failure> {
    let id = body_id(request)?;
    let info = state
        .sender
        .serve_info(id)
        .ok_or((StatusCode::NOT_FOUND, "unknown body".into()))?;
    if info.kind != BodyKind::Html {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "only HTML bodies can be previewed".into(),
        ));
    }
    let body = state
        .bodies
        .get(id)
        .ok_or((StatusCode::NOT_FOUND, "unknown body".into()))?;
    let len = body.index().byte_len();
    if len > MAX_FULL_RESPONSE {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "HTML too large to preview".into()));
    }
    let bytes = body
        .read_range(0, len)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_SECURITY_POLICY, PREVIEW_CSP)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes)
        .unwrap())
}

fn respond(state: &AppState, request: &Request<Vec<u8>>) -> Result<Response<Vec<u8>>, Failure> {
    if request.method() == "OPTIONS" {
        return Ok(with_cors(Response::builder().status(StatusCode::NO_CONTENT))
            .body(Vec::new())
            .unwrap());
    }
    let id = body_id(request)?;
    let content_type = served_type(state, id);
    let body = state
        .bodies
        .get(id)
        .ok_or((StatusCode::NOT_FOUND, "unknown body".into()))?;
    let len = body.index().byte_len();
    let io_err = |e: std::io::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());

    let range = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok());
    let Some(range) = range else {
        if len > MAX_FULL_RESPONSE {
            return Err((
                StatusCode::RANGE_NOT_SATISFIABLE,
                "body too large; use a Range request".into(),
            ));
        }
        let bytes = body.read_range(0, len).map_err(io_err)?;
        return Ok(with_cors(Response::builder().status(StatusCode::OK))
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_TYPE, &content_type)
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .body(bytes)
            .unwrap());
    };

    let (start, end) = parse_range(range, len).ok_or((
        StatusCode::RANGE_NOT_SATISFIABLE,
        format!("unsupported or invalid range: {range}"),
    ))?;
    let end = end.min(start + MAX_RANGE - 1);
    let bytes = body.read_range(start, end - start + 1).map_err(io_err)?;
    Ok(with_cors(Response::builder().status(StatusCode::PARTIAL_CONTENT))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, &content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
        .body(bytes)
        .unwrap())
}

/// Parses a single `bytes=start-end`, `bytes=start-` or `bytes=-suffix` range into an
/// inclusive `(start, end)` within `len`.
fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (s, "") => (s.parse().ok()?, len - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len - 1)),
    };
    (start <= end && start < len).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::{parse_range, safe_media_type};

    #[test]
    fn only_inert_media_types_are_served_as_is() {
        for ok in [
            "image/png",
            "image/jpeg",
            "application/pdf",
            "video/mp4",
            "application/json",
        ] {
            assert!(safe_media_type(ok), "{ok}");
        }
        // These can run script if rendered, so they're served as text/plain.
        for bad in [
            "text/html",
            "image/svg+xml",
            "application/xhtml+xml",
            "text/javascript",
            "application/xml",
        ] {
            assert!(!safe_media_type(bad), "{bad}");
        }
    }

    #[test]
    fn parses_supported_range_forms() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=990-2000", 1000), Some((990, 999)));
    }

    #[test]
    fn rejects_invalid_ranges() {
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1", 0), None);
    }
}
