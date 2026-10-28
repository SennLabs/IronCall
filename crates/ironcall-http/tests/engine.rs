mod support;

use ironcall_http::{Engine, EngineConfig, Error, HttpVersionPref, Phase, Protocol, Request};
use support::TestServer;

fn engine_trusting(server: &TestServer) -> Engine {
    Engine::new(EngineConfig {
        extra_roots: vec![server.ca.clone()],
        ..EngineConfig::default()
    })
    .unwrap()
}

fn get(url: &str, version: HttpVersionPref) -> Request {
    let mut req = Request::get(url).unwrap();
    req.options.http_version = version;
    req
}

fn assert_measured(phase: Phase) {
    assert!(
        matches!(phase, Phase::Measured(_)),
        "expected measured, got {phase:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn http1_over_tls() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let resp = engine
        .send(get(&server.https_url("/"), HttpVersionPref::Http1))
        .await
        .unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "hello");
    assert_eq!(resp.protocol, Protocol::Http1_1);
    assert!(!resp.connection_reused);
    assert_measured(resp.timings.dns);
    assert_measured(resp.timings.connect);
    assert_measured(resp.timings.tls);
    assert!(resp.timings.total >= resp.timings.ttfb + resp.timings.download);
}

#[tokio::test(flavor = "multi_thread")]
async fn http2_negotiated_via_alpn() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let resp = engine
        .send(get(&server.https_url("/big"), HttpVersionPref::Auto))
        .await
        .unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.protocol, Protocol::Http2);
    assert_eq!(resp.body.len(), 1 << 20);
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_has_no_tls_phase() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let resp = engine
        .send(get(&server.http_url("/"), HttpVersionPref::Auto))
        .await
        .unwrap();

    assert_eq!(resp.protocol, Protocol::Http1_1);
    assert_eq!(resp.timings.tls, Phase::NotApplicable);
    assert_measured(resp.timings.connect);
}

async fn assert_second_request_reuses(version: HttpVersionPref, protocol: Protocol) {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let url = server.https_url("/");

    let first = engine.send(get(&url, version)).await.unwrap();
    let second = engine.send(get(&url, version)).await.unwrap();

    assert_eq!(second.protocol, protocol);
    assert!(!first.connection_reused);
    assert!(second.connection_reused);
    assert_eq!(first.connection_id, second.connection_id);
    assert_eq!(second.timings.dns, Phase::Reused);
    assert_eq!(second.timings.connect, Phase::Reused);
    assert_eq!(second.timings.tls, Phase::Reused);
    assert_eq!(server.https_connections(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn http1_connection_is_reused() {
    assert_second_request_reuses(HttpVersionPref::Http1, Protocol::Http1_1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http2_connection_is_reused() {
    assert_second_request_reuses(HttpVersionPref::Http2, Protocol::Http2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_connection_option_measures_cold() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let url = server.https_url("/");

    let first = engine.send(get(&url, HttpVersionPref::Http1)).await.unwrap();
    let mut req = get(&url, HttpVersionPref::Http1);
    req.options.fresh_connection = true;
    let second = engine.send(req).await.unwrap();

    assert!(!second.connection_reused);
    assert_ne!(first.connection_id, second.connection_id);
    assert_measured(second.timings.dns);
    assert_measured(second.timings.tls);
    assert_eq!(server.https_connections(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_off_connection_is_never_reused_for_verify_on() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let url = server.https_url("/");

    for version in [HttpVersionPref::Http1, HttpVersionPref::Http2] {
        let mut insecure = get(&url, version);
        insecure.options.verify_tls = false;
        let insecure = engine.send(insecure).await.unwrap();
        let secure = engine.send(get(&url, version)).await.unwrap();

        assert!(!secure.connection_reused, "{version:?}");
        assert_ne!(insecure.connection_id, secure.connection_id, "{version:?}");
    }
    assert_eq!(server.https_connections(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn untrusted_certificate_fails_unless_verification_is_off() {
    let server = TestServer::start().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let url = server.https_url("/");

    let err = engine.send(get(&url, HttpVersionPref::Auto)).await.unwrap_err();
    assert!(matches!(err, Error::Tls { .. }), "{err:?}");

    let mut insecure = get(&url, HttpVersionPref::Auto);
    insecure.options.verify_tls = false;
    assert_eq!(engine.send(insecure).await.unwrap().status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn http2_required_but_server_only_speaks_http1() {
    let server = TestServer::start_with_alpn(&[b"http/1.1"]).await;
    let engine = engine_trusting(&server);
    let url = server.https_url("/");

    let err = engine.send(get(&url, HttpVersionPref::Http2)).await.unwrap_err();
    assert!(matches!(err, Error::Http2NotNegotiated), "{err:?}");

    let auto = engine.send(get(&url, HttpVersionPref::Auto)).await.unwrap();
    assert_eq!(auto.protocol, Protocol::Http1_1);
}
