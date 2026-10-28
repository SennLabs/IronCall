//! Local test servers: HTTPS (HTTP/1.1 + HTTP/2 via ALPN) and plain HTTP sharing one
//! axum app, with a throwaway CA so the engine can trust it through
//! `EngineConfig::extra_roots`. Plus raw TCP servers for misbehaviour tests.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_compression::tokio::write::{BrotliEncoder, DeflateEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use futures_util::stream;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The plain text every `/compressed/*` route encodes.
pub const COMPRESSED_TEXT: &str = "hello, compressed world! hello, compressed world!";

/// Tracks how many `/slow` requests are being handled at once.
#[derive(Default)]
pub struct Gauge {
    current: AtomicUsize,
    max: AtomicUsize,
}

impl Gauge {
    pub fn max(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }

    fn enter(&self) {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(now, Ordering::SeqCst);
    }

    fn leave(&self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct TestServer {
    pub https_addr: SocketAddr,
    pub http_addr: SocketAddr,
    pub ca: CertificateDer<'static>,
    /// The CA in PEM form, for tools like `curl --cacert`.
    pub ca_pem: String,
    pub gauge: Arc<Gauge>,
    https_accepted: Arc<AtomicUsize>,
    http_accepted: Arc<AtomicUsize>,
}

impl TestServer {
    pub fn https_url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.https_addr.port())
    }

    pub fn http_url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.http_addr.port())
    }

    /// Number of TCP connections the HTTPS listener has accepted.
    pub fn https_connections(&self) -> usize {
        self.https_accepted.load(Ordering::SeqCst)
    }

    pub fn http_connections(&self) -> usize {
        self.http_accepted.load(Ordering::SeqCst)
    }

    pub async fn start() -> Self {
        Self::start_with_alpn(&[b"h2", b"http/1.1"]).await
    }

    /// Starts a server whose TLS listener only offers the given ALPN protocols.
    pub async fn start_with_alpn(alpn: &[&[u8]]) -> Self {
        let (ca, ca_pem, leaf_chain, leaf_key) = make_certs();

        let mut tls =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(leaf_chain, leaf_key)
                .unwrap();
        tls.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let acceptor = TlsAcceptor::from(Arc::new(tls));

        let https = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = TestServer {
            https_addr: https.local_addr().unwrap(),
            http_addr: http.local_addr().unwrap(),
            ca,
            ca_pem,
            gauge: Arc::default(),
            https_accepted: Arc::default(),
            http_accepted: Arc::default(),
        };
        let app = app(AppState {
            gauge: server.gauge.clone(),
            compressed: Arc::new(compressed_bodies().await),
        });

        let accepted = server.https_accepted.clone();
        let https_app = app.clone();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = https.accept().await.unwrap();
                accepted.fetch_add(1, Ordering::SeqCst);
                let acceptor = acceptor.clone();
                let app = https_app.clone();
                tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(tcp).await {
                        serve(stream, app).await;
                    }
                });
            }
        });

        let accepted = server.http_accepted.clone();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = http.accept().await.unwrap();
                accepted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(tcp, app.clone()));
            }
        });

        server
    }
}

#[derive(Clone)]
struct AppState {
    gauge: Arc<Gauge>,
    compressed: Arc<Vec<(&'static str, Vec<u8>)>>,
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { "hello" }))
        .route("/big", get(|| async { vec![b'x'; 1 << 20] }))
        .route("/echo", any(echo))
        .route("/headers", any(echo_headers))
        .route("/status/{code}", any(status))
        .route("/compressed/{name}", get(compressed))
        .route("/redirect/{code}", any(redirect))
        .route("/chain/{n}", get(chain))
        .route("/slow", get(slow))
        .route("/slow-body", get(slow_body))
        .route("/stream/{mb}", get(stream_mb))
        .route("/login", axum::routing::post(login))
        .route("/me", get(me))
        .route("/cookie-echo", get(cookie_echo))
        .route("/multipart", axum::routing::post(multipart))
        .route("/sink", axum::routing::post(sink))
        .with_state(state)
}

/// `METHOD body` — so tests can see what method and body arrived.
async fn echo(method: Method, body: Bytes) -> String {
    format!("{method} {}", String::from_utf8_lossy(&body))
}

/// Request headers as `name: value` lines, sorted.
async fn echo_headers(headers: HeaderMap) -> String {
    let mut lines: Vec<_> = headers
        .iter()
        .map(|(n, v)| format!("{n}: {}", v.to_str().unwrap_or("?")))
        .collect();
    lines.sort();
    lines.join("\n")
}

async fn status(Path(code): Path<u16>) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

async fn compressed(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let Some((encoding, body)) = state.compressed.iter().find(|(n, _)| *n == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let encoding = if name.starts_with("deflate") {
        "deflate"
    } else {
        encoding
    };
    ([(header::CONTENT_ENCODING, encoding)], body.clone()).into_response()
}

async fn compressed_bodies() -> Vec<(&'static str, Vec<u8>)> {
    async fn encode<W: tokio::io::AsyncWrite + Unpin>(
        mut w: W,
        finish: impl FnOnce(W) -> Vec<u8>,
    ) -> Vec<u8> {
        w.write_all(COMPRESSED_TEXT.as_bytes()).await.unwrap();
        w.shutdown().await.unwrap();
        finish(w)
    }
    vec![
        (
            "gzip",
            encode(GzipEncoder::new(Vec::new()), |w| w.into_inner()).await,
        ),
        (
            "deflate-zlib",
            encode(ZlibEncoder::new(Vec::new()), |w| w.into_inner()).await,
        ),
        (
            "deflate-raw",
            encode(DeflateEncoder::new(Vec::new()), |w| w.into_inner()).await,
        ),
        (
            "br",
            encode(BrotliEncoder::new(Vec::new()), |w| w.into_inner()).await,
        ),
        (
            "zstd",
            encode(ZstdEncoder::new(Vec::new()), |w| w.into_inner()).await,
        ),
    ]
}

/// `/redirect/{code}?to=<location>` answers `code` with `Location: to`.
async fn redirect(
    Path(code): Path<u16>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mut resp = StatusCode::from_u16(code).unwrap().into_response();
    resp.headers_mut()
        .insert(header::LOCATION, HeaderValue::from_str(&q["to"]).unwrap());
    resp
}

/// `/chain/{n}` redirects to `/chain/{n-1}` until 0, which answers "done".
async fn chain(Path(n): Path<u32>) -> Response {
    if n == 0 {
        return "done".into_response();
    }
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/chain/{}", n - 1)).unwrap(),
    );
    resp
}

/// `/slow?ms=N` waits N ms (default 300) before answering, tracked by the gauge.
async fn slow(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> &'static str {
    let ms = q.get("ms").and_then(|v| v.parse().ok()).unwrap_or(300);
    state.gauge.enter();
    tokio::time::sleep(Duration::from_millis(ms)).await;
    state.gauge.leave();
    "slow"
}

/// Ten `chunk N\n` lines, 100 ms apart.
async fn slow_body() -> Body {
    Body::from_stream(stream::unfold(0u32, |i| async move {
        if i == 10 {
            return None;
        }
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Some((
            Ok::<_, std::io::Error>(Bytes::from(format!("chunk {i}\n"))),
            i + 1,
        ))
    }))
}

/// `mb` MiB of `x` in 64 KiB chunks, pausing 300 ms before the last chunk so tests can
/// tell the body is streamed rather than buffered.
async fn stream_mb(Path(mb): Path<usize>) -> Body {
    const CHUNK: usize = 64 * 1024;
    let chunks = mb * 1024 * 1024 / CHUNK;
    Body::from_stream(stream::unfold(0usize, move |i| async move {
        if i == chunks {
            return None;
        }
        if i == chunks - 1 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Some((Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; CHUNK])), i + 1))
    }))
}

/// Sets `session=abc` and redirects to `/me`, like a form login.
async fn login() -> Response {
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/me"),
            (header::SET_COOKIE, "session=abc; Path=/; HttpOnly"),
        ],
    )
        .into_response()
}

/// 200 `hello abc` with the session cookie, 401 without.
async fn me(headers: HeaderMap) -> Response {
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if cookie.split("; ").any(|c| c == "session=abc") {
        "hello abc".into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// The request's `Cookie` header, verbatim.
async fn cookie_echo(headers: HeaderMap) -> String {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// One line per part: `name|file name|content type|length|text (if short)`.
async fn multipart(mut form: axum::extract::Multipart) -> Result<String, StatusCode> {
    let mut lines = Vec::new();
    while let Some(field) = form.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().unwrap_or("").to_string();
        let content_type = field.content_type().unwrap_or("").to_string();
        let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
        let text = if data.len() <= 64 {
            String::from_utf8_lossy(&data).into_owned()
        } else {
            String::new()
        };
        lines.push(format!("{name}|{file_name}|{content_type}|{}|{text}", data.len()));
    }
    Ok(lines.join("\n"))
}

/// Reads the body as a stream without keeping it and answers with its length.
async fn sink(body: Body) -> String {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(c) => total += c.len() as u64,
            Err(_) => return "error".into(),
        }
    }
    total.to_string()
}

async fn serve<S>(stream: S, app: Router)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = TowerToHyperService::new(app);
    let _ = auto::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), service)
        .await;
}

/// Accepts TCP connections and never says anything, so a TLS handshake hangs.
pub async fn silent_listener() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            held.push(tcp);
        }
    });
    addr
}

/// A plain-HTTP server that answers the first request on each connection with a
/// keep-alive response, then closes the connection without answering the second, as a
/// server does when its idle timeout fires just as the client reuses the connection.
/// Returns the address and a counter of accepted connections.
pub async fn stale_after_first_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                if read_request(&mut tcp, &mut buf).await {
                    let _ = tcp
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: keep-alive\r\n\r\nok",
                        )
                        .await;
                }
                // Wait for the next request, then hang up without answering.
                let _ = read_request(&mut tcp, &mut buf).await;
            });
        }
    });
    (addr, accepted)
}

/// Reads until the end of a request head (`\r\n\r\n`). False on EOF.
async fn read_request(tcp: &mut tokio::net::TcpStream, buf: &mut [u8]) -> bool {
    let mut seen = Vec::new();
    loop {
        match tcp.read(buf).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    return true;
                }
            }
        }
    }
}

fn make_certs() -> (
    CertificateDer<'static>,
    String,
    Vec<CertificateDer<'static>>,
    PrivateKeyDer<'static>,
) {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "IronCall Test CA");
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();

    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &ca)
        .unwrap();

    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    (
        ca.der().clone(),
        ca.pem(),
        vec![leaf.der().clone(), ca.der().clone()],
        key,
    )
}

/// A minimal HTTP proxy for tests: `CONNECT` tunnels and absolute-form forwarding,
/// optionally requiring Basic auth. Records what it saw.
pub struct TestProxy {
    pub addr: SocketAddr,
    pub log: Arc<std::sync::Mutex<Vec<ProxyLog>>>,
}

#[derive(Clone, Debug)]
pub struct ProxyLog {
    pub method: String,
    pub target: String,
    pub auth: Option<String>,
}

impl TestProxy {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    pub fn entries(&self) -> Vec<ProxyLog> {
        self.log.lock().unwrap().clone()
    }

    /// `required_auth` is the exact `Proxy-Authorization` value to demand, if any.
    pub async fn start(required_auth: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log: Arc<std::sync::Mutex<Vec<ProxyLog>>> = Arc::default();
        let proxy = TestProxy {
            addr,
            log: log.clone(),
        };
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let log = log.clone();
                let required = required_auth.clone();
                tokio::spawn(async move {
                    let _ = handle_proxy_conn(tcp, log, required).await;
                });
            }
        });
        proxy
    }
}

async fn handle_proxy_conn(
    mut client: tokio::net::TcpStream,
    log: Arc<std::sync::Mutex<Vec<ProxyLog>>>,
    required: Option<String>,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let rest = buf[head_end..].to_vec();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    let header_lines: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    let auth = header_lines
        .iter()
        .find(|l| l.to_ascii_lowercase().starts_with("proxy-authorization:"))
        .map(|l| l.split_once(':').unwrap().1.trim().to_string());
    log.lock().unwrap().push(ProxyLog {
        method: method.clone(),
        target: target.clone(),
        auth: auth.clone(),
    });

    if let Some(required) = &required
        && auth.as_deref() != Some(required.as_str())
    {
        client
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"test\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await?;
        return Ok(());
    }

    if method == "CONNECT" {
        let mut upstream = tokio::net::TcpStream::connect(&target).await?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        return Ok(());
    }

    // Absolute-form: forward to the origin in origin-form, without proxy credentials.
    let url = url::Url::parse(&target).map_err(std::io::Error::other)?;
    let origin = format!(
        "{}:{}",
        url.host_str().unwrap_or(""),
        url.port_or_known_default().unwrap_or(80)
    );
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    let mut forwarded = format!("{method} {path} {version}\r\n");
    for line in header_lines {
        if !line.to_ascii_lowercase().starts_with("proxy-authorization:") {
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
    }
    forwarded.push_str("\r\n");
    let mut upstream = tokio::net::TcpStream::connect(&origin).await?;
    upstream.write_all(forwarded.as_bytes()).await?;
    upstream.write_all(&rest).await?;
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// A CA unrelated to the test servers, for CA-isolation tests.
pub fn unrelated_ca() -> CertificateDer<'static> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, "Unrelated CA");
    params
        .self_signed(&KeyPair::generate().unwrap())
        .unwrap()
        .der()
        .clone()
}
