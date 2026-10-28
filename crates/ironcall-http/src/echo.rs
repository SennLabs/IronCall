//! The built-in local echo server (PLAN.md Phase 8): a tiny httpbin-style API on
//! 127.0.0.1 that the sample workspace calls, so first-run examples work offline.
//!
//! Routes (any method unless noted):
//! - `/status/{code}`: that status, with the echo body
//! - `/delay/{ms}`: waits (at most 10 s), then echoes
//! - `POST /token`: a fresh `access_token`
//! - `/bearer`: 200 with `{"authenticated": true}` if there is a Bearer token, else 401
//! - `/json`: a fixed sample document, for JSONPath
//! - anything else: echoes the request as JSON (method, path, query, headers, body)
//!
//! Sensitive request headers are masked in echoes, so a token sent to the echo server
//! doesn't come back in a response body (and from there into history).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// Request bodies larger than this are rejected with 413.
const MAX_BODY: usize = 1024 * 1024;
const MAX_DELAY_MS: u64 = 10_000;
/// Header or query names whose values are masked in echoes: a token sent to the echo
/// server shouldn't come back in a response body (and from there into history).
const MASKED_PARTS: &[&str] = &[
    "authorization",
    "cookie",
    "token",
    "secret",
    "password",
    "passwd",
    "api-key",
    "api_key",
    "apikey",
    "session",
    "signature",
    "credential",
];
const MASKED_NAMES: &[&str] = &["key", "sig", "auth", "code", "sid", "jwt", "pwd", "pass"];
/// Connections served at once; more wait (the echo server is a local demo, not a load target).
const MAX_CONNECTIONS: usize = 64;

fn masked(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MASKED_NAMES.contains(&lower.as_str()) || MASKED_PARTS.iter().any(|p| lower.contains(p))
}

/// A running echo server. It stops when the async runtime shuts down.
#[derive(Clone, Copy, Debug)]
pub struct EchoServer {
    pub addr: SocketAddr,
}

impl EchoServer {
    /// `http://127.0.0.1:<port>`, with no trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

/// Binds to a free loopback port and serves in the background.
pub async fn start() -> std::io::Result<EchoServer> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(_) => {
                    // E.g. out of file descriptors: back off instead of spinning.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let Ok(permit) = slots.clone().acquire_owned().await else {
                break;
            };
            tokio::spawn(async move {
                let _permit = permit;
                let service = service_fn(move |req| handle(req, addr));
                let conn = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
                if let Err(e) = conn.await {
                    tracing::debug!(error = %e, "echo connection ended");
                }
            });
        }
    });
    tracing::info!(%addr, "local echo server started");
    Ok(EchoServer { addr })
}

async fn handle(req: Request<Incoming>, addr: SocketAddr) -> Result<Response<Full<Bytes>>, Infallible> {
    // A web page can point a hostname it controls at 127.0.0.1 (DNS rebinding) and read
    // the echoes; such requests carry that hostname, so only our own address is served.
    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let port = addr.port();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Ok(reply(
            StatusCode::MISDIRECTED_REQUEST,
            json!({"error": "unexpected Host"}),
        ));
    }
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers: BTreeMap<String, String> = req
        .headers()
        .iter()
        .map(|(n, v)| {
            let name = n.as_str().to_string();
            let value = if masked(&name) {
                "••••".to_string()
            } else {
                String::from_utf8_lossy(v.as_bytes()).into_owned()
            };
            (name, value)
        })
        .collect();
    let bearer = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.len() > 7 && v[..7].eq_ignore_ascii_case("bearer "));
    let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return Ok(reply(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error": "body too large"}),
            ));
        }
    };

    let echo = || {
        let text = String::from_utf8(body.to_vec()).ok();
        json!({
            "method": method,
            "path": path,
            "query": mask_query(parse_query(&query)),
            "headers": headers,
            "json": text.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()),
            "body": text,
        })
    };

    let mut segments = path.trim_start_matches('/').split('/');
    let response = match (segments.next().unwrap_or(""), segments.next()) {
        // 1xx are interim responses, not something a handler can return as final.
        ("status", Some(code)) => match code.parse::<u16>().ok().filter(|c| (200..=999).contains(c)) {
            Some(c) => reply(StatusCode::from_u16(c).unwrap_or(StatusCode::OK), echo()),
            None => reply(
                StatusCode::BAD_REQUEST,
                json!({"error": "status must be 200–999"}),
            ),
        },
        ("delay", Some(ms)) => {
            let ms = ms.parse::<u64>().unwrap_or(0).min(MAX_DELAY_MS);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            reply(StatusCode::OK, echo())
        }
        ("token", None) if method == "POST" => reply(
            StatusCode::OK,
            json!({
                "access_token": format!("echo-{:016x}", rand_u64()),
                "token_type": "Bearer",
                "expires_in": 3600,
            }),
        ),
        ("bearer", None) if bearer => reply(StatusCode::OK, json!({"authenticated": true})),
        ("bearer", None) => reply(StatusCode::UNAUTHORIZED, json!({"authenticated": false})),
        ("json", None) => reply(
            StatusCode::OK,
            json!({
                "store": {
                    "name": "IronCall Books",
                    "books": [
                        {"id": 1, "title": "The Rust Programming Language", "price": 39.95, "tags": ["rust"]},
                        {"id": 2, "title": "HTTP: The Definitive Guide", "price": 54.5, "tags": ["http", "web"]},
                    ],
                    "open": true,
                },
            }),
        ),
        _ => reply(StatusCode::OK, echo()),
    };
    Ok(response)
}

fn reply(status: StatusCode, body: Value) -> Response<Full<Bytes>> {
    let mut text = serde_json::to_string_pretty(&body).unwrap_or_default();
    text.push('\n');
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(text)))
        .expect("valid response")
}

/// Masks the values of sensitive-looking query parameters.
fn mask_query(mut query: Value) -> Value {
    if let Value::Object(map) = &mut query {
        for (name, value) in map.iter_mut() {
            if masked(name) {
                *value = Value::from("••••");
            }
        }
    }
    query
}

/// `a=1&b=2&b=3` → `{"a": "1", "b": ["2", "3"]}` (percent-decoded).
fn parse_query(query: &str) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match out.get_mut(k.as_ref()) {
            Some(Value::Array(list)) => list.push(Value::from(v.into_owned())),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, Value::from(v.into_owned())]);
            }
            None => {
                out.insert(k.into_owned(), Value::from(v.into_owned()));
            }
        }
    }
    Value::Object(out)
}

/// Not cryptographic; the echo token is a demo value.
fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, EngineConfig, Request as EngineRequest};

    async fn send(
        engine: &Engine,
        method: &str,
        url: String,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, Value) {
        let mut req = EngineRequest::new(method.parse().unwrap(), &url).unwrap();
        for (n, v) in headers {
            req.headers.append(
                http::HeaderName::from_bytes(n.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        if !body.is_empty() {
            req.body = crate::Body::from(body.to_string());
        }
        let resp = engine.send(req).await.unwrap();
        (resp.status.as_u16(), serde_json::from_slice(&resp.body).unwrap())
    }

    #[tokio::test]
    async fn echoes_and_serves_the_demo_routes() {
        let server = start().await.unwrap();
        let base = server.base_url();
        let engine = Engine::new(EngineConfig::default()).unwrap();

        let (status, v) = send(
            &engine,
            "POST",
            format!("{base}/anything/x?a=1&b=2&b=3&q=h%C3%A9&api_key=qk-1"),
            &[
                ("content-type", "application/json"),
                ("authorization", "Bearer secret-1"),
                ("x-auth-token", "tok-2"),
                ("x-demo", "yes"),
            ],
            r#"{"name":"Ada"}"#,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(v["method"], "POST");
        assert_eq!(v["path"], "/anything/x");
        assert_eq!(
            v["query"],
            json!({"a": "1", "b": ["2", "3"], "q": "hé", "api_key": "••••"})
        );
        assert_eq!(v["headers"]["x-auth-token"], "••••");
        assert_eq!(v["json"]["name"], "Ada");
        assert_eq!(v["headers"]["x-demo"], "yes");
        assert_eq!(
            v["headers"]["authorization"], "••••",
            "sensitive headers are masked"
        );
        assert!(!v.to_string().contains("secret-1"));
        assert_eq!(
            send(&engine, "GET", format!("{base}/status/101"), &[], "")
                .await
                .0,
            400
        );
        let rebound = send(
            &engine,
            "GET",
            format!("{base}/json"),
            &[("host", "evil.example")],
            "",
        )
        .await;
        assert_eq!(rebound.0, 421, "a foreign Host is refused (DNS rebinding)");

        assert_eq!(
            send(&engine, "GET", format!("{base}/status/418"), &[], "")
                .await
                .0,
            418
        );
        assert_eq!(
            send(&engine, "GET", format!("{base}/status/abc"), &[], "")
                .await
                .0,
            400
        );
        let (status, token) = send(&engine, "POST", format!("{base}/token"), &[], "").await;
        assert_eq!(status, 200);
        let token = token["access_token"].as_str().unwrap().to_string();
        assert!(token.starts_with("echo-"));
        let auth = format!("Bearer {token}");
        assert_eq!(
            send(
                &engine,
                "GET",
                format!("{base}/bearer"),
                &[("authorization", &auth)],
                ""
            )
            .await
            .0,
            200
        );
        assert_eq!(
            send(&engine, "GET", format!("{base}/bearer"), &[], "").await.0,
            401
        );
        let (_, doc) = send(&engine, "GET", format!("{base}/json"), &[], "").await;
        assert_eq!(doc["store"]["books"][1]["price"], 54.5);
        let t = std::time::Instant::now();
        send(&engine, "GET", format!("{base}/delay/50"), &[], "").await;
        assert!(t.elapsed() >= Duration::from_millis(50));
    }
}
