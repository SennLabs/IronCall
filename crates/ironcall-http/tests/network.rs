//! Phase 4 engine behaviour: HTTP proxies and CONNECT tunnels, NO_PROXY, custom CAs and
//! pool isolation, cookies, and file and multipart bodies.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::Engine as _;
use bytes::Bytes;
use http::header::{COOKIE, HeaderValue};
use http::{Method, StatusCode};
use ironcall_http::{
    Body, CookieJar, Engine, EngineConfig, Error, ErrorKind, NoProxy, Part, Progress, Protocol, ProxyAuth,
    ProxyConfig, Request, RootSet, Stage, load_ca_file,
};
use support::{TestProxy, TestServer, unrelated_ca};

fn engine_trusting(server: &TestServer) -> Engine {
    Engine::new(EngineConfig {
        extra_roots: vec![server.ca.clone()],
        ..EngineConfig::default()
    })
    .unwrap()
}

fn text(body: &Bytes) -> &str {
    std::str::from_utf8(body).unwrap()
}

fn basic(user: &str, pass: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
    )
}

fn via(proxy: &TestProxy, auth: Option<ProxyAuth>) -> ProxyConfig {
    ProxyConfig::manual(&proxy.url(), auth, NoProxy::default()).unwrap()
}

// ---------------------------------------------------------------------------------
// Proxies

#[tokio::test(flavor = "multi_thread")]
async fn http_goes_through_proxy_in_absolute_form() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(None).await;
    let engine = engine_trusting(&server);

    let mut req = Request::get(&server.http_url("/headers")).unwrap();
    req.options.proxy = Some(via(&proxy, None));
    let resp = engine.send(req).await.unwrap();

    assert_eq!(resp.status, 200);
    let log = proxy.entries();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].method, "GET");
    assert_eq!(log[0].target, server.http_url("/headers"));
}

#[tokio::test(flavor = "multi_thread")]
async fn https_tunnels_with_connect_and_negotiates_h2_inside() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(Some(basic("u", "p"))).await;
    let engine = engine_trusting(&server);

    let mut req = Request::get(&server.https_url("/headers")).unwrap();
    req.options.proxy = Some(via(&proxy, Some(ProxyAuth::new("u", "p"))));
    let resp = engine.send(req).await.unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.protocol, Protocol::Http2);
    let log = proxy.entries();
    assert_eq!(log[0].method, "CONNECT");
    assert_eq!(log[0].target, format!("localhost:{}", server.https_addr.port()));
    assert_eq!(log[0].auth.as_deref(), Some(basic("u", "p").as_str()));
    // The credentials went only to the proxy, never inside the tunnel.
    assert!(
        !text(&resp.body).contains("proxy-authorization"),
        "{}",
        text(&resp.body)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_auth_is_sent_on_http_requests() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(Some(basic("u", "p"))).await;
    let engine = engine_trusting(&server);

    let mut req = Request::get(&server.http_url("/")).unwrap();
    req.options.proxy = Some(via(&proxy, Some(ProxyAuth::new("u", "p"))));
    assert_eq!(engine.send(req).await.unwrap().status, 200);
    assert_eq!(proxy.entries()[0].auth.as_deref(), Some(basic("u", "p").as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_proxy_auth_gives_407() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(Some(basic("u", "p"))).await;
    let engine = engine_trusting(&server);

    // HTTPS: the CONNECT is refused, which is a connection error.
    let mut req = Request::get(&server.https_url("/")).unwrap();
    req.options.proxy = Some(via(&proxy, None));
    let err = engine.send(req).await.unwrap_err();
    assert!(matches!(err, Error::Proxy { status: 407, .. }), "{err:?}");
    assert_eq!(err.kind(), ErrorKind::Connect);
    assert!(err.message().contains("407"), "{}", err.message());

    // Plain HTTP: the proxy's 407 is an ordinary response.
    let mut req = Request::get(&server.http_url("/")).unwrap();
    req.options.proxy = Some(via(&proxy, None));
    assert_eq!(engine.send(req).await.unwrap().status, 407);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_proxy_and_bypass_connect_directly_without_proxy_credentials() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(None).await;
    let engine = engine_trusting(&server);
    let auth = Some(ProxyAuth::new("u", "secret"));

    let mut req = Request::get(&server.https_url("/headers")).unwrap();
    req.options.proxy =
        Some(ProxyConfig::manual(&proxy.url(), auth.clone(), NoProxy::parse("localhost")).unwrap());
    let resp = engine.send(req).await.unwrap();
    assert!(!text(&resp.body).contains("proxy-authorization"));

    let mut req = Request::get(&server.http_url("/headers")).unwrap();
    req.options.proxy = Some(via(&proxy, auth));
    req.options.bypass_proxy = true;
    let resp = engine.send(req).await.unwrap();
    assert!(!text(&resp.body).contains("proxy-authorization"));

    assert!(proxy.entries().is_empty(), "{:?}", proxy.entries());
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_default_proxy_applies_and_requests_can_override() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(None).await;
    let engine = Engine::new(EngineConfig {
        extra_roots: vec![server.ca.clone()],
        proxy: via(&proxy, None),
        ..EngineConfig::default()
    })
    .unwrap();

    engine
        .send(Request::get(&server.http_url("/")).unwrap())
        .await
        .unwrap();
    assert_eq!(proxy.entries().len(), 1);

    let mut direct = Request::get(&server.http_url("/")).unwrap();
    direct.options.proxy = Some(ProxyConfig::none());
    engine.send(direct).await.unwrap();
    assert_eq!(proxy.entries().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn pool_keeps_proxied_and_direct_connections_apart() {
    let server = TestServer::start().await;
    let proxy = TestProxy::start(None).await;
    let engine = engine_trusting(&server);

    let direct = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap();
    let mut req = Request::get(&server.https_url("/")).unwrap();
    req.options.proxy = Some(via(&proxy, None));
    let proxied = engine.send(req).await.unwrap();

    assert!(!proxied.connection_reused);
    assert_ne!(direct.connection_id, proxied.connection_id);
    assert_eq!(proxy.entries().len(), 1);
}

// ---------------------------------------------------------------------------------
// Custom CAs

#[tokio::test(flavor = "multi_thread")]
async fn custom_ca_from_file_is_needed_unless_verification_is_off() {
    let server = TestServer::start().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ca_file = dir.path().join("ca.pem");
    std::fs::write(&ca_file, &server.ca_pem).unwrap();

    let err = engine
        .send(Request::get(&server.https_url("/")).unwrap())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Tls, "{err:?}");

    let mut req = Request::get(&server.https_url("/")).unwrap();
    req.options.extra_roots = Some(RootSet::new(load_ca_file(&ca_file).unwrap()));
    assert_eq!(engine.send(req).await.unwrap().status, 200);

    let mut req = Request::get(&server.https_url("/")).unwrap();
    req.options.verify_tls = false;
    assert_eq!(engine.send(req).await.unwrap().status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_verified_with_one_ca_set_are_not_reused_for_another() {
    let server = TestServer::start().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();

    let mut trusted = Request::get(&server.https_url("/")).unwrap();
    trusted.options.extra_roots = Some(RootSet::new(vec![server.ca.clone()]));
    assert_eq!(engine.send(trusted).await.unwrap().status, 200);

    // Same origin, a CA set that doesn't trust the server: must not ride on the pooled
    // connection verified with the other set.
    let mut other = Request::get(&server.https_url("/")).unwrap();
    other.options.extra_roots = Some(RootSet::new(vec![unrelated_ca()]));
    let err = engine.send(other).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Tls, "{err:?}");
}

// ---------------------------------------------------------------------------------
// Cookies

#[tokio::test(flavor = "multi_thread")]
async fn cookie_login_flow_across_redirect_and_later_requests() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let jar = Arc::new(CookieJar::new());

    let mut login = Request::new(Method::POST, &server.https_url("/login")).unwrap();
    login.options.cookie_jar = Some(jar.clone());
    let resp = engine.send(login).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.redirects.len(), 1);
    assert_eq!(text(&resp.body), "hello abc");

    let mut me = Request::get(&server.https_url("/me")).unwrap();
    me.options.cookie_jar = Some(jar.clone());
    assert_eq!(engine.send(me).await.unwrap().status, 200);

    let anonymous = Request::get(&server.https_url("/me")).unwrap();
    assert_eq!(
        engine.send(anonymous).await.unwrap().status,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(jar.list().len(), 1);
    let restored = CookieJar::from_json(&jar.to_json()).unwrap();
    assert_eq!(restored.list(), jar.list());
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cookie_header_overrides_the_jar() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let jar = Arc::new(CookieJar::new());
    let mut login = Request::new(Method::POST, &server.https_url("/login")).unwrap();
    login.options.cookie_jar = Some(jar.clone());
    engine.send(login).await.unwrap();

    let mut req = Request::get(&server.https_url("/cookie-echo")).unwrap();
    req.options.cookie_jar = Some(jar.clone());
    let preview = ironcall_http::engine_headers(&req);
    assert!(preview.iter().any(|(n, v)| n == COOKIE && v == "session=abc"));
    req.headers
        .insert(COOKIE, HeaderValue::from_static("session=mine"));
    assert_eq!(text(&engine.send(req).await.unwrap().body), "session=mine");
}

// ---------------------------------------------------------------------------------
// File and multipart bodies

#[tokio::test(flavor = "multi_thread")]
async fn multipart_text_and_file_parts_are_parsed_by_the_server() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.png");
    std::fs::write(&file, b"PNGDATA").unwrap();

    let mut req = Request::new(Method::POST, &server.https_url("/multipart")).unwrap();
    req.body = Body::Multipart(vec![
        Part::Text {
            name: "title".into(),
            value: "Holiday".into(),
            content_type: None,
        },
        Part::File {
            name: "image".into(),
            path: file,
            file_name: None,
            content_type: Some("image/png".into()),
        },
    ]);
    let resp = engine.send(req).await.unwrap();
    assert_eq!(resp.status, 200, "{}", text(&resp.body));
    assert_eq!(
        text(&resp.body),
        "title|||7|Holiday\nimage|photo.png|image/png|7|PNGDATA"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_file_body_and_307_replay() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("body.txt");
    std::fs::write(&file, b"from disk").unwrap();
    let body = Body::File {
        path: file,
        content_type: Some("text/plain".into()),
    };

    let mut req = Request::new(Method::PUT, &server.https_url("/echo")).unwrap();
    req.body = body.clone();
    assert_eq!(text(&engine.send(req).await.unwrap().body), "PUT from disk");

    let mut req = Request::new(Method::POST, &server.https_url("/redirect/307?to=/echo")).unwrap();
    req.body = body;
    let resp = engine.send(req).await.unwrap();
    assert_eq!(resp.redirects.len(), 1);
    assert_eq!(text(&resp.body), "POST from disk");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_body_file_fails_before_connecting() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let mut req = Request::new(Method::POST, &server.http_url("/echo")).unwrap();
    req.body = Body::File {
        path: "/no/such/file.bin".into(),
        content_type: None,
    };
    let err = engine.send(req).await.unwrap_err();
    assert!(matches!(err, Error::BodyFile { .. }), "{err:?}");
    assert_eq!(err.kind(), ErrorKind::InvalidRequest);
    assert_eq!(server.http_connections(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_progress_is_reported() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("up.bin");
    let size = 8 * 1024 * 1024u64;
    std::fs::File::create(&file).unwrap().set_len(size).unwrap();

    let events: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let seen = events.clone();
    let progress = move |p: Progress| seen.lock().unwrap().push(p);
    let mut req = Request::new(Method::POST, &server.http_url("/sink")).unwrap();
    req.body = Body::File {
        path: file,
        content_type: None,
    };
    let mut sink = Vec::new();
    engine.send_to(req, &mut sink, &progress).await.unwrap();
    assert_eq!(String::from_utf8(sink).unwrap(), size.to_string());

    let events = events.lock().unwrap();
    let uploads: Vec<_> = events.iter().filter(|p| p.stage == Stage::Uploading).collect();
    assert!(!uploads.is_empty());
    assert!(uploads.iter().all(|p| p.upload_total == Some(size)));
    assert_eq!(uploads.last().unwrap().uploaded, size);
    let stages: Vec<Stage> = events.iter().map(|p| p.stage).collect();
    let first_wait = stages.iter().position(|s| *s == Stage::Waiting).unwrap();
    assert!(stages[..first_wait].contains(&Stage::Uploading));
}

/// Resident set size of this process, from `/proc/self/statm` (Linux).
fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

#[tokio::test(flavor = "multi_thread")]
async fn large_multipart_upload_streams_with_flat_memory() {
    let Some(before) = rss_bytes() else {
        eprintln!("no /proc/self/statm; skipping");
        return;
    };
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.bin");
    let size = 500 * 1024 * 1024u64;
    // Sparse: takes no disk space and reads back as zeros.
    std::fs::File::create(&file).unwrap().set_len(size).unwrap();

    let mut req = Request::new(Method::POST, &server.http_url("/sink")).unwrap();
    req.body = Body::Multipart(vec![Part::File {
        name: "blob".into(),
        path: file,
        file_name: None,
        content_type: None,
    }]);
    let expected_len = req.body.len().unwrap();
    let start = Instant::now();
    let resp = engine.send(req).await.unwrap();
    let elapsed = start.elapsed();
    let after = rss_bytes().unwrap();

    assert_eq!(text(&resp.body), expected_len.to_string());
    let growth = after.saturating_sub(before);
    eprintln!(
        "500MB multipart upload: {:.1}s, RSS {} MB -> {} MB (+{} MB)",
        elapsed.as_secs_f64(),
        before >> 20,
        after >> 20,
        growth >> 20
    );
    assert!(growth < 50 * 1024 * 1024, "RSS grew by {} MB", growth >> 20);
}
