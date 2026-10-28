//! Timeline, TLS details and informational responses (PLAN.md Phase 5).

mod support;

use std::time::Duration;

use http::Method;
use ironcall_http::{Engine, EngineConfig, NoProxy, ProxyConfig, Request, TimelineEvent, TimelineKind};
use support::{TestProxy, TestServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn engine_for(server: &TestServer) -> Engine {
    Engine::new(EngineConfig {
        extra_roots: vec![server.ca.clone()],
        ..EngineConfig::default()
    })
    .unwrap()
}

fn kinds(timeline: &[TimelineEvent]) -> Vec<TimelineKind> {
    timeline.iter().map(|e| e.kind).collect()
}

/// `true` if `wanted` appears in `kinds` in order (not necessarily adjacent).
fn in_order(kinds: &[TimelineKind], wanted: &[TimelineKind]) -> bool {
    let mut it = kinds.iter();
    wanted.iter().all(|w| it.any(|k| k == w))
}

async fn send_traced(
    engine: &Engine,
    req: Request,
) -> Result<ironcall_http::ResponseHead, ironcall_http::Failure> {
    let mut sink = Vec::new();
    engine.send_to_traced(req, &mut sink, &|_| {}).await
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_info_and_certificate_for_http2_and_reused_connections() {
    let server = TestServer::start().await;
    let engine = engine_for(&server);

    let first = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    let tls = first.tls.clone().expect("TLS info on an HTTPS response");
    assert_eq!(tls.version, "TLSv1.3");
    assert!(tls.cipher.starts_with("TLS13_"), "{}", tls.cipher);
    assert_eq!(tls.alpn.as_deref(), Some("h2"));
    assert!(tls.verified);
    let cert = tls.certificate.expect("leaf certificate");
    assert!(cert.issuer.contains("IronCall Test CA"), "{}", cert.issuer);
    assert!(cert.sans.contains(&"localhost".to_string()), "{:?}", cert.sans);
    assert!(cert.sans.contains(&"127.0.0.1".to_string()), "{:?}", cert.sans);
    assert_eq!(cert.chain_length, 2);
    assert!(cert.not_before.unwrap() < cert.not_after.unwrap());

    // The second request reuses the connection and still reports its TLS details.
    let second = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    assert!(second.connection_reused);
    assert_eq!(
        second.tls.as_ref().map(|t| &t.alpn),
        Some(&Some("h2".to_string()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_info_for_http1_and_none_for_plain_http() {
    let server = TestServer::start_with_alpn(&[b"http/1.1"]).await;
    let engine = engine_for(&server);
    let res = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    let tls = res.tls.clone().unwrap();
    assert_eq!(tls.alpn.as_deref(), Some("http/1.1"));
    let again = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    assert!(again.connection_reused);
    assert_eq!(again.tls, Some(tls));

    let plain = engine
        .send(Request::get(&server.http_url("/")).unwrap())
        .await
        .unwrap();
    assert!(plain.tls.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_off_is_reported() {
    let server = TestServer::start().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let mut req = Request::get(&server.https_url("/")).unwrap();
    req.options.verify_tls = false;
    let res = engine.send(req).await.unwrap();
    assert!(!res.tls.as_ref().unwrap().verified);
    assert!(res.timeline.iter().any(|e| e.message.contains("NOT verified")));
}

/// A raw HTTP/1.1 server that sends `103 Early Hints` before the final response.
async fn early_hints_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = tcp.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    seen.extend_from_slice(&buf[..n]);
                }
                tcp.write_all(b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\n")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(30)).await;
                tcp.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            });
        }
    });
    format!("http://127.0.0.1:{}/", addr.port())
}

#[tokio::test(flavor = "multi_thread")]
async fn informational_103_is_captured_and_ttfb_is_to_the_final_response() {
    let url = early_hints_server().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let res = send_traced(&engine, Request::get(&url).unwrap()).await.unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.informational.len(), 1);
    let hint = &res.informational[0];
    assert_eq!(hint.status, 103);
    assert_eq!(hint.headers["link"], "</style.css>; rel=preload");
    assert!(
        res.timings.ttfb >= hint.after_send + Duration::from_millis(20),
        "TTFB counts to the final headers"
    );

    let heads: Vec<&str> = res
        .timeline
        .iter()
        .filter(|e| e.kind == TimelineKind::ResponseHeaders)
        .map(|e| e.message.as_str())
        .collect();
    assert_eq!(heads, ["HTTP/1.1 103 Early Hints", "HTTP/1.1 200 OK"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_of_a_plain_send() {
    let server = TestServer::start().await;
    let engine = engine_for(&server);
    let mut req = Request::new(Method::POST, &server.https_url("/echo")).unwrap();
    req.body = "hello".into();
    let res = send_traced(&engine, req).await.unwrap();
    use TimelineKind::*;
    let k = kinds(&res.timeline);
    assert!(
        in_order(
            &k,
            &[
                Info,
                Info,
                Info,
                Info,
                Tls,
                RequestHeaders,
                RequestBody,
                ResponseHeaders,
                ResponseBody
            ]
        ),
        "{k:?}"
    );
    assert!(
        res.timeline
            .iter()
            .any(|e| e.message.starts_with("Resolved localhost to"))
    );
    assert!(
        res.timeline
            .iter()
            .any(|e| e.message.starts_with("Connected to 127.0.0.1:"))
    );
    let request = res.timeline.iter().find(|e| e.kind == RequestHeaders).unwrap();
    assert!(
        request.message.starts_with("POST https://localhost:"),
        "{}",
        request.message
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(n, v)| n == "user-agent" && v.starts_with("IronCall/"))
    );
    assert!(res.timeline.iter().any(|e| e.message == "Request body: 5 bytes"));
    assert!(
        res.timeline.windows(2).all(|w| w[0].at <= w[1].at),
        "timestamps are ordered"
    );

    // A second send on the same connection says so instead of resolving again.
    let again = send_traced(&engine, Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    assert!(
        again.timeline[0].message.starts_with("Reusing connection #"),
        "{:?}",
        again.timeline[0]
    );
    assert!(!again.timeline.iter().any(|e| e.message.starts_with("Resolving")));
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_of_a_redirect() {
    let server = TestServer::start().await;
    let engine = engine_for(&server);
    let url = server.http_url(&format!("/redirect/303?to={}", server.http_url("/")));
    let mut req = Request::new(Method::POST, &url).unwrap();
    req.body = "x".into();
    let res = send_traced(&engine, req).await.unwrap();
    use TimelineKind::*;
    let k = kinds(&res.timeline);
    assert!(
        in_order(
            &k,
            &[
                RequestHeaders,
                ResponseHeaders,
                Redirect,
                RequestHeaders,
                ResponseHeaders,
                ResponseBody
            ]
        ),
        "{k:?}"
    );
    let redirect = res.timeline.iter().find(|e| e.kind == Redirect).unwrap();
    assert!(
        redirect.message.contains("Following 303 to http://localhost:"),
        "{}",
        redirect.message
    );
    assert!(
        redirect.message.contains("method changed to GET"),
        "{}",
        redirect.message
    );
    assert!(res.redirects[0].tls.is_none(), "plain HTTP hop");
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_of_a_proxy_connect_keeps_raw_headers_for_the_app_to_mask() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(None).await;
    let engine = engine_for(&server);
    let mut req = Request::get(&server.https_url("/")).unwrap();
    let auth = ironcall_http::ProxyAuth::new("user", "pass");
    req.options.proxy = Some(ProxyConfig::manual(&proxy.url(), Some(auth), NoProxy::default()).unwrap());
    let res = send_traced(&engine, req).await.unwrap();
    use TimelineKind::*;
    let k = kinds(&res.timeline);
    assert!(
        in_order(&k, &[Info, Proxy, Proxy, Tls, RequestHeaders, ResponseHeaders]),
        "{k:?}"
    );
    let connect = res.timeline.iter().find(|e| e.kind == Proxy).unwrap();
    assert!(
        connect.message.starts_with("CONNECT localhost:"),
        "{}",
        connect.message
    );
    assert!(
        connect
            .headers
            .iter()
            .any(|(n, v)| n == "proxy-authorization" && v.starts_with("Basic "))
    );
    assert!(
        res.timeline
            .iter()
            .any(|e| e.kind == Proxy && e.message.starts_with("Proxy replied: HTTP/1.1 200"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn timeline_is_returned_with_failures() {
    // Connection refused: resolve and connect attempt, then the error.
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let failure = send_traced(&engine, Request::get("http://127.0.0.1:9/").unwrap())
        .await
        .unwrap_err();
    assert_eq!(failure.error.kind(), ironcall_http::ErrorKind::Connect);
    let k = kinds(&failure.timeline);
    assert_eq!(k.first(), Some(&TimelineKind::Info));
    assert_eq!(k.last(), Some(&TimelineKind::Error));
    assert!(
        failure
            .timeline
            .iter()
            .any(|e| e.message.starts_with("Connecting to 127.0.0.1 port 9"))
    );

    // TLS with an untrusted CA: connected, then a TLS error with the reason.
    let server = TestServer::start().await;
    let failure = send_traced(&engine, Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap_err();
    assert_eq!(failure.error.kind(), ironcall_http::ErrorKind::Tls);
    assert!(
        failure
            .timeline
            .iter()
            .any(|e| e.message.starts_with("Connected to"))
    );
    let last = failure.timeline.last().unwrap();
    assert_eq!(last.kind, TimelineKind::Error);
    assert!(last.message.contains("unknown issuer"), "{}", last.message);

    // send_to (without trace) keeps the plain error API.
    let mut sink = Vec::new();
    let err = engine
        .send_to(Request::get("http://127.0.0.1:9/").unwrap(), &mut sink, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(err.kind(), ironcall_http::ErrorKind::Connect);
}
