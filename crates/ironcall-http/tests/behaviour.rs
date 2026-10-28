//! Phase 1 engine behaviour: methods, bodies, decompression, redirects, cancellation,
//! timeouts, streaming, stale connections and concurrency caps.

mod support;

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HeaderValue};
use http::{Method, StatusCode};
use ironcall_http::{
    CancellationToken, Engine, EngineConfig, Error, ErrorKind, HttpVersionPref, Phase, Progress, Request,
    Stage,
};
use support::{COMPRESSED_TEXT, TestServer};
use tokio::io::AsyncWrite;

fn engine_trusting(server: &TestServer) -> Engine {
    engine_with(server, EngineConfig::default())
}

fn engine_with(server: &TestServer, config: EngineConfig) -> Engine {
    Engine::new(EngineConfig {
        extra_roots: vec![server.ca.clone()],
        ..config
    })
    .unwrap()
}

fn request(method: Method, url: &str) -> Request {
    Request::new(method, url).unwrap()
}

fn text(body: &Bytes) -> &str {
    std::str::from_utf8(body).unwrap()
}

// ---------------------------------------------------------------------------------
// Methods and bodies

#[tokio::test(flavor = "multi_thread")]
async fn common_methods_reach_the_server_with_their_body() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    for (method, version) in [
        (Method::GET, HttpVersionPref::Http1),
        (Method::POST, HttpVersionPref::Http1),
        (Method::PUT, HttpVersionPref::Http2),
        (Method::PATCH, HttpVersionPref::Http1),
        (Method::DELETE, HttpVersionPref::Http2),
        (Method::OPTIONS, HttpVersionPref::Http1),
        (Method::from_bytes(b"PROPFIND").unwrap(), HttpVersionPref::Http1),
    ] {
        let mut req = request(method.clone(), &server.https_url("/echo"));
        req.options.http_version = version;
        if method != Method::GET {
            req.body = Bytes::from_static(b"payload").into();
        }
        let resp = engine.send(req).await.unwrap();
        assert_eq!(resp.status, 200, "{method}");
        let expected_body = if method == Method::GET { "" } else { "payload" };
        assert_eq!(
            text(&resp.body),
            format!("{method} {expected_body}"),
            "{method} {version:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn head_and_204_have_empty_bodies() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let head = engine
        .send(request(Method::HEAD, &server.http_url("/")))
        .await
        .unwrap();
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.body_len, 0);

    for version in [HttpVersionPref::Http1, HttpVersionPref::Http2] {
        let mut req = request(Method::GET, &server.https_url("/status/204"));
        req.options.http_version = version;
        let resp = engine.send(req).await.unwrap();
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(resp.body.is_empty());
        assert_eq!(resp.reason, "No Content");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_adds_default_headers_but_keeps_user_ones() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let resp = engine
        .send(request(Method::GET, &server.http_url("/headers")))
        .await
        .unwrap();
    let headers = text(&resp.body);
    assert!(
        headers.contains(&format!("user-agent: {}", ironcall_http::DEFAULT_USER_AGENT)),
        "{headers}"
    );
    assert!(
        headers.contains("accept-encoding: gzip, deflate, br, zstd"),
        "{headers}"
    );
    assert!(
        headers.contains(&format!("host: localhost:{}", server.http_addr.port())),
        "{headers}"
    );

    let mut req = request(Method::GET, &server.http_url("/headers"));
    req.headers
        .insert("user-agent", HeaderValue::from_static("custom/1"));
    let resp = engine.send(req).await.unwrap();
    let headers = text(&resp.body);
    assert!(headers.contains("user-agent: custom/1"), "{headers}");
    assert_eq!(headers.matches("user-agent").count(), 1, "{headers}");
}

// ---------------------------------------------------------------------------------
// Decompression

#[tokio::test(flavor = "multi_thread")]
async fn every_supported_encoding_is_decoded() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    for (route, coding) in [
        ("gzip", "gzip"),
        ("deflate-zlib", "deflate"),
        ("deflate-raw", "deflate"),
        ("br", "br"),
        ("zstd", "zstd"),
    ] {
        let resp = engine
            .send(request(
                Method::GET,
                &server.https_url(&format!("/compressed/{route}")),
            ))
            .await
            .unwrap();
        assert_eq!(text(&resp.body), COMPRESSED_TEXT, "{route}");
        assert_eq!(resp.decoded_encodings, vec![coding.to_string()], "{route}");
        assert_eq!(resp.body_len, COMPRESSED_TEXT.len() as u64, "{route}");
        assert_ne!(resp.wire_len, resp.body_len, "{route}");
        assert!(resp.undecoded_encoding.is_none());
    }
}

// ---------------------------------------------------------------------------------
// Redirects

#[tokio::test(flavor = "multi_thread")]
async fn redirect_statuses_follow_method_rules() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    for (code, expected) in [
        (301, "GET "),
        (302, "GET "),
        (303, "GET "),
        (307, "POST data"),
        (308, "POST data"),
    ] {
        let mut req = request(
            Method::POST,
            &server.https_url(&format!("/redirect/{code}?to=/echo")),
        );
        req.body = Bytes::from_static(b"data").into();
        req.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        let resp = engine.send(req).await.unwrap();
        assert_eq!(resp.status, 200, "{code}");
        assert_eq!(text(&resp.body), expected, "{code}");
        assert_eq!(resp.redirects.len(), 1, "{code}");
        assert_eq!(resp.redirects[0].status.as_u16(), code);
        assert!(
            resp.redirects[0]
                .url
                .ends_with(&format!("/redirect/{code}?to=/echo"))
        );
        assert!(resp.final_url.ends_with("/echo"), "{}", resp.final_url);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_redirect_chain_is_recorded_hop_by_hop() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let resp = engine
        .send(request(Method::GET, &server.http_url("/chain/3")))
        .await
        .unwrap();
    assert_eq!(text(&resp.body), "done");
    let urls: Vec<_> = resp
        .redirects
        .iter()
        .map(|h| h.url.rsplit('/').next().unwrap().to_string())
        .collect();
    assert_eq!(urls, ["3", "2", "1"]);
    assert!(resp.redirects.iter().all(|h| h.status == StatusCode::FOUND));
    // Same origin: the first hop's connection is reused for the rest.
    assert!(matches!(resp.redirects[0].timings.connect, Phase::Measured(_)));
    assert_eq!(resp.redirects[1].timings.connect, Phase::Reused);
    assert!(resp.connection_reused);
    assert_eq!(server.http_connections(), 1);
    assert!(resp.total >= resp.timings.total);
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_redirects_is_an_error() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let mut req = request(Method::GET, &server.http_url("/chain/5"));
    req.options.max_redirects = 3;
    let err = engine.send(req).await.unwrap_err();
    assert!(matches!(err, Error::TooManyRedirects(3)), "{err:?}");
    assert_eq!(err.kind(), ErrorKind::TooManyRedirects);
}

#[tokio::test(flavor = "multi_thread")]
async fn redirects_not_followed_when_disabled() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let mut req = request(Method::GET, &server.http_url("/chain/1"));
    req.options.follow_redirects = false;
    let resp = engine.send(req).await.unwrap();
    assert_eq!(resp.status, StatusCode::FOUND);
    assert!(resp.redirects.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn credentials_are_stripped_on_cross_origin_redirect_only() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    let with_credentials = |url: String| {
        let mut req = request(Method::GET, &url);
        req.headers
            .insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        req.headers.insert(COOKIE, HeaderValue::from_static("session=1"));
        req
    };

    let same = engine
        .send(with_credentials(server.https_url("/redirect/307?to=/headers")))
        .await
        .unwrap();
    assert!(text(&same.body).contains("authorization: Bearer secret"));
    assert!(text(&same.body).contains("cookie: session=1"));

    // https → http on another port is a different origin.
    let target = server.http_url("/headers");
    let cross = engine
        .send(with_credentials(
            server.https_url(&format!("/redirect/307?to={target}")),
        ))
        .await
        .unwrap();
    assert!(
        !text(&cross.body).contains("authorization"),
        "{}",
        text(&cross.body)
    );
    assert!(!text(&cross.body).contains("cookie"), "{}", text(&cross.body));
}

// ---------------------------------------------------------------------------------
// Cancellation and timeouts

fn cancel_after(ms: u64) -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        t.cancel();
    });
    token
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_connect() {
    let addr = support::silent_listener().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let mut req = request(Method::GET, &format!("https://localhost:{}/", addr.port()));
    req.options.cancel = Some(cancel_after(100));

    let start = Instant::now();
    let err = engine.send(req).await.unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_while_waiting_for_headers() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let mut req = request(Method::GET, &server.http_url("/slow?ms=5000"));
    req.options.cancel = Some(cancel_after(150));

    let start = Instant::now();
    let err = engine.send(req).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_download_keeps_partial_body_and_drops_connection() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let mut req = request(Method::GET, &server.http_url("/slow-body"));
    req.options.cancel = Some(cancel_after(350));

    let mut sink = Vec::new();
    let err = engine.send_to(req, &mut sink, &|_| {}).await.unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    let partial = String::from_utf8(sink).unwrap();
    assert!(partial.starts_with("chunk 0\n"), "{partial:?}");
    assert!(!partial.contains("chunk 9"), "{partial:?}");

    // The interrupted connection must not be reused.
    let next = engine
        .send(request(Method::GET, &server.http_url("/")))
        .await
        .unwrap();
    assert!(!next.connection_reused);
    assert_eq!(server.http_connections(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_reports_the_stage_it_hit() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);
    let silent = support::silent_listener().await;

    for (url, stage) in [
        (format!("https://localhost:{}/", silent.port()), Stage::Connecting),
        (server.http_url("/slow?ms=5000"), Stage::Waiting),
        (server.http_url("/slow-body"), Stage::Downloading),
    ] {
        let mut req = request(Method::GET, &url);
        req.options.timeout = Some(Duration::from_millis(300));
        let err = engine.send(req).await.unwrap_err();
        match err {
            Error::Timeout { limit, stage: got } => {
                assert_eq!(limit, Duration::from_millis(300));
                assert_eq!(got, stage, "{url}");
            }
            other => panic!("{url}: expected timeout, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------
// Streaming and progress

/// Counts bytes without storing them and remembers when the first write happened.
#[derive(Default)]
struct CountingSink {
    bytes: u64,
    writes: usize,
    first_write: Option<Instant>,
}

impl AsyncWrite for CountingSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.first_write.get_or_insert_with(Instant::now);
        self.bytes += buf.len() as u64;
        self.writes += 1;
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn large_body_is_streamed_to_the_sink_with_progress() {
    let server = TestServer::start().await;
    let engine = engine_trusting(&server);

    for version in [HttpVersionPref::Http1, HttpVersionPref::Http2] {
        let mut req = request(Method::GET, &server.https_url("/stream/20"));
        req.options.http_version = version;
        let stages = Arc::new(Mutex::new(Vec::<Stage>::new()));
        let max_bytes = Arc::new(AtomicU64::new(0));
        let (s, m) = (stages.clone(), max_bytes.clone());
        let progress = move |p: Progress| {
            let mut stages = s.lock().unwrap();
            if stages.last() != Some(&p.stage) {
                stages.push(p.stage);
            }
            m.fetch_max(p.bytes, Ordering::SeqCst);
        };

        let mut sink = CountingSink::default();
        let head = engine.send_to(req, &mut sink, &progress).await.unwrap();
        let done = Instant::now();

        assert_eq!(head.body_len, 20 << 20, "{version:?}");
        assert_eq!(sink.bytes, 20 << 20);
        assert!(sink.writes > 10, "{} writes", sink.writes);
        // The server pauses 300 ms before the last chunk; data arriving before that pause
        // proves the body was streamed rather than buffered.
        let streamed_for = done - sink.first_write.unwrap();
        assert!(
            streamed_for >= Duration::from_millis(250),
            "{version:?}: {streamed_for:?}"
        );
        assert_eq!(
            *stages.lock().unwrap(),
            [Stage::Connecting, Stage::Waiting, Stage::Downloading],
            "{version:?}"
        );
        assert_eq!(max_bytes.load(Ordering::SeqCst), 20 << 20);
    }
}

// ---------------------------------------------------------------------------------
// Connection management

#[tokio::test(flavor = "multi_thread")]
async fn stale_pooled_connection_is_retried_once_on_a_new_one() {
    let (addr, accepted) = support::stale_after_first_server().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let url = format!("http://127.0.0.1:{}/", addr.port());

    let first = engine.send(request(Method::GET, &url)).await.unwrap();
    assert_eq!(text(&first.body), "ok");
    // The second request goes out on the pooled connection, which the server closes
    // without answering; the engine retries on a fresh connection.
    let second = engine.send(request(Method::GET, &url)).await.unwrap();
    assert_eq!(text(&second.body), "ok");
    assert!(!second.connection_reused);
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn post_is_not_retried_after_the_server_saw_it() {
    // The server reads the second request and then hangs up, so it may have acted on it.
    // Retrying a POST could duplicate a side effect, so the error must surface instead.
    let (addr, accepted) = support::stale_after_first_server().await;
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let url = format!("http://127.0.0.1:{}/", addr.port());

    engine.send(request(Method::GET, &url)).await.unwrap();
    let err = engine.send(request(Method::POST, &url)).await.unwrap_err();
    assert!(matches!(err, ironcall_http::Error::Http(_)), "{err:?}");
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn per_host_http1_cap_limits_concurrency() {
    let server = TestServer::start().await;
    let engine = Arc::new(engine_with(
        &server,
        EngineConfig {
            max_per_host_http1: 2,
            ..EngineConfig::default()
        },
    ));

    let url = server.http_url("/slow?ms=200");
    let tasks: Vec<_> = (0..6)
        .map(|_| {
            let (engine, url) = (engine.clone(), url.clone());
            tokio::spawn(async move { engine.send(request(Method::GET, &url)).await.unwrap() })
        })
        .collect();
    for t in tasks {
        assert_eq!(t.await.unwrap().status, 200);
    }
    assert_eq!(server.gauge.max(), 2);
    assert_eq!(server.http_connections(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn global_in_flight_cap_applies_across_hosts_and_protocols() {
    let server = TestServer::start().await;
    let engine = Arc::new(engine_with(
        &server,
        EngineConfig {
            max_in_flight: 1,
            ..EngineConfig::default()
        },
    ));

    let urls = [
        server.http_url("/slow?ms=150"),
        server.https_url("/slow?ms=150"),
        server.http_url("/slow?ms=150"),
    ];
    let tasks: Vec<_> = urls
        .into_iter()
        .map(|url| {
            let engine = engine.clone();
            tokio::spawn(async move { engine.send(request(Method::GET, &url)).await.unwrap() })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(server.gauge.max(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_connections_expire() {
    let server = TestServer::start().await;
    let engine = engine_with(
        &server,
        EngineConfig {
            idle_timeout: Duration::from_millis(100),
            ..EngineConfig::default()
        },
    );
    let url = server.http_url("/");

    engine.send(request(Method::GET, &url)).await.unwrap();
    assert!(
        engine
            .send(request(Method::GET, &url))
            .await
            .unwrap()
            .connection_reused
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !engine
            .send(request(Method::GET, &url))
            .await
            .unwrap()
            .connection_reused
    );
    assert_eq!(server.http_connections(), 2);
}

// ---------------------------------------------------------------------------------
// Errors

#[tokio::test(flavor = "multi_thread")]
async fn error_kinds_and_messages_are_user_facing() {
    let server = TestServer::start().await;
    let untrusting = Engine::new(EngineConfig::default()).unwrap();

    let tls = untrusting
        .send(request(Method::GET, &server.https_url("/")))
        .await
        .unwrap_err();
    assert_eq!(tls.kind(), ErrorKind::Tls);
    assert!(
        tls.message().contains("not trusted (unknown issuer)"),
        "{}",
        tls.message()
    );

    // Bind then drop to get a port nothing listens on.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let refused = untrusting
        .send(request(
            Method::GET,
            &format!("http://127.0.0.1:{}/", closed.port()),
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Connect);
    assert!(
        refused.message().contains("Connection refused"),
        "{}",
        refused.message()
    );

    let dns = untrusting
        .send(request(Method::GET, "http://does-not-exist.invalid/"))
        .await
        .unwrap_err();
    assert_eq!(dns.kind(), ErrorKind::Dns);

    let scheme = untrusting
        .send(request(Method::GET, "ftp://example.com/"))
        .await
        .unwrap_err();
    assert_eq!(scheme.kind(), ErrorKind::Unsupported);
}

/// Tauri runs commands on a multi-threaded runtime, so `send_to` futures must be `Send`
/// for any `Send` sink and progress callback.
#[allow(dead_code)]
fn send_to_future_is_send(
    engine: &Engine,
    sink: &mut tokio::fs::File,
    progress: &(dyn Fn(Progress) + Send + Sync),
) {
    fn assert_send<T: Send>(_: T) {}
    assert_send(engine.send_to(Request::get("http://a.test/").unwrap(), sink, progress));
}
