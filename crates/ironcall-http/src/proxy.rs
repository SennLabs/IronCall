//! HTTP proxies (PLAN.md Phase 4 "minimal proxy"): environment variables or a manual
//! URL, a `NO_PROXY` list, Basic proxy auth, and `CONNECT` tunnels for HTTPS.
//!
//! Only plain-HTTP proxies are supported. SOCKS and TLS-to-the-proxy (`https://`
//! proxy URLs) come after V1.
//!
//! Timings through a proxy: DNS and TCP connect are measured to the proxy; for HTTPS
//! the `CONNECT` exchange counts as part of connect, and TLS is the handshake with the
//! origin through the tunnel.

use std::fmt;
use std::net::IpAddr;

use base64::Engine as _;
use http::{HeaderValue, Uri};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::Error;

/// Basic credentials for a proxy. The password never appears in `Debug` output.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyAuth {
    pub username: String,
    password: String,
}

impl ProxyAuth {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// The `Proxy-Authorization` header value.
    pub(crate) fn header(&self) -> HeaderValue {
        let token =
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.username, self.password));
        let mut value =
            HeaderValue::from_str(&format!("Basic {token}")).expect("base64 is a valid header value");
        value.set_sensitive(true);
        value
    }
}

impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyAuth")
            .field("username", &self.username)
            .field("password", &"••••")
            .finish()
    }
}

/// One HTTP proxy server.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProxyServer {
    pub host: String,
    pub port: u16,
    pub auth: Option<ProxyAuth>,
}

impl ProxyServer {
    /// Parses `http://[user:pass@]host[:port]` (scheme optional, port defaults to 80).
    /// Credentials in the URL are used unless `auth` is given.
    pub fn parse(url: &str, auth: Option<ProxyAuth>) -> Result<Self, Error> {
        let url = url.trim();
        let with_scheme = if url.contains("://") {
            url.to_string()
        } else {
            format!("http://{url}")
        };
        let parsed =
            url::Url::parse(&with_scheme).map_err(|e| Error::InvalidProxy(format!("`{url}`: {e}")))?;
        match parsed.scheme() {
            "http" => {}
            s if s.starts_with("socks") => {
                return Err(Error::ProxyUnsupported(format!(
                    "SOCKS proxies ({s}://) are not supported yet; they are planned after V1"
                )));
            }
            "https" => {
                return Err(Error::ProxyUnsupported(
                    "proxies reached over TLS (https:// proxy URLs) are not supported yet; use http://"
                        .into(),
                ));
            }
            s => return Err(Error::InvalidProxy(format!("unsupported proxy scheme `{s}`"))),
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::InvalidProxy(format!("`{url}` has no host")))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let decode = |s: &str| {
            percent_decode(s)
                .ok_or_else(|| Error::InvalidProxy("proxy credentials are not valid UTF-8".into()))
        };
        let auth = match auth {
            Some(a) => Some(a),
            None if !parsed.username().is_empty() => Some(ProxyAuth::new(
                decode(parsed.username())?,
                decode(parsed.password().unwrap_or(""))?,
            )),
            None => None,
        };
        Ok(Self {
            host,
            port: parsed.port().unwrap_or(80),
            auth,
        })
    }

    /// `host:port` for messages (IPv6 in brackets), without credentials.
    pub fn display(&self) -> String {
        authority(&self.host, self.port)
    }
}

fn percent_decode(s: &str) -> Option<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .ok()
        .map(|c| c.into_owned())
}

pub(crate) fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Which proxy to use for which scheme, and which hosts bypass it.
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub http: Option<ProxyServer>,
    pub https: Option<ProxyServer>,
    pub no_proxy: NoProxy,
}

impl ProxyConfig {
    /// Connect directly.
    pub fn none() -> Self {
        Self::default()
    }

    /// One proxy for both HTTP and HTTPS.
    pub fn manual(url: &str, auth: Option<ProxyAuth>, no_proxy: NoProxy) -> Result<Self, Error> {
        let server = ProxyServer::parse(url, auth)?;
        Ok(Self {
            http: Some(server.clone()),
            https: Some(server),
            no_proxy,
        })
    }

    /// Reads `http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY`, `all_proxy`/
    /// `ALL_PROXY` and `no_proxy`/`NO_PROXY` (lowercase wins, as in curl).
    pub fn from_env() -> Result<Self, Error> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// [`ProxyConfig::from_env`] with a custom variable lookup (for tests).
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let var = |names: &[&str]| names.iter().find_map(|n| get(n).filter(|v| !v.trim().is_empty()));
        let all = var(&["all_proxy", "ALL_PROXY"]);
        let parse = |v: Option<String>| v.map(|v| ProxyServer::parse(&v, None)).transpose();
        Ok(Self {
            http: parse(var(&["http_proxy", "HTTP_PROXY"]).or_else(|| all.clone()))?,
            https: parse(var(&["https_proxy", "HTTPS_PROXY"]).or(all))?,
            no_proxy: NoProxy::parse(&var(&["no_proxy", "NO_PROXY"]).unwrap_or_default()),
        })
    }

    pub fn is_none(&self) -> bool {
        self.http.is_none() && self.https.is_none()
    }

    /// The proxy for a request to `url`, or `None` to connect directly.
    pub fn select(&self, url: &Uri) -> Option<&ProxyServer> {
        let https = url.scheme_str() == Some("https");
        let server = if https {
            self.https.as_ref()
        } else {
            self.http.as_ref()
        }?;
        let host = url.host()?.trim_start_matches('[').trim_end_matches(']');
        let port = url.port_u16().unwrap_or(if https { 443 } else { 80 });
        (!self.no_proxy.matches(host, port)).then_some(server)
    }
}

/// A `NO_PROXY` list: `*`, host names (matching subdomains too, with or without a
/// leading dot), `host:port`, IP addresses and CIDR ranges, separated by commas or
/// whitespace. Loopback is not bypassed implicitly; list `localhost` and `127.0.0.1`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoProxy {
    all: bool,
    entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Domain { name: String, port: Option<u16> },
    Ip(IpAddr, Option<u16>),
    Cidr(IpAddr, u8),
}

impl NoProxy {
    pub fn parse(list: &str) -> Self {
        let mut out = Self::default();
        out.add(list);
        out
    }

    /// Adds more entries (e.g. the user's list on top of the environment's).
    pub fn add(&mut self, list: &str) {
        for raw in list
            .split([',', ' ', '\t', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if raw == "*" {
                self.all = true;
                continue;
            }
            if let Some(entry) = parse_entry(raw) {
                self.entries.push(entry);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.all && self.entries.is_empty()
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        if self.all {
            return true;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let ip = host.parse::<IpAddr>().ok();
        self.entries.iter().any(|e| match e {
            Entry::Domain { name, port: p } => {
                p.is_none_or(|p| p == port) && (host == *name || host.ends_with(&format!(".{name}")))
            }
            Entry::Ip(addr, p) => ip == Some(*addr) && p.is_none_or(|p| p == port),
            Entry::Cidr(net, bits) => ip.is_some_and(|ip| in_cidr(ip, *net, *bits)),
        })
    }
}

fn parse_entry(raw: &str) -> Option<Entry> {
    let raw = raw.to_ascii_lowercase();
    if let Some((net, bits)) = raw.split_once('/') {
        let net = net
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .ok()?;
        let bits = bits.parse::<u8>().ok()?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        return (bits <= max).then_some(Entry::Cidr(net, bits));
    }
    if let Ok(ip) = raw
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return Some(Entry::Ip(ip, None));
    }
    // `[::1]:8080`
    if let Some(rest) = raw.strip_prefix('[')
        && let Some((ip, port)) = rest.split_once("]:")
    {
        return Some(Entry::Ip(ip.parse().ok()?, Some(port.parse().ok()?)));
    }
    let (name, port) = match raw.rsplit_once(':') {
        Some((name, port)) => (name, Some(port.parse::<u16>().ok()?)),
        None => (raw.as_str(), None),
    };
    if let Ok(ip) = name.parse::<IpAddr>() {
        return Some(Entry::Ip(ip, port));
    }
    let name = name
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .trim_end_matches('.');
    (!name.is_empty()).then(|| Entry::Domain {
        name: name.to_string(),
        port,
    })
}

fn in_cidr(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

/// Asks the proxy to open a tunnel to `host:port` (RFC 9110 §9.3.6).
pub(crate) async fn connect_tunnel(
    tcp: &mut TcpStream,
    proxy: &ProxyServer,
    host: &str,
    port: u16,
    auth_override: Option<&HeaderValue>,
) -> Result<String, Error> {
    let target = authority(host, port);
    let mut head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    let auth = proxy
        .auth
        .as_ref()
        .map(ProxyAuth::header)
        .or_else(|| auth_override.cloned());
    if let Some(auth) = auth {
        head.push_str("Proxy-Authorization: ");
        head.push_str(auth.to_str().unwrap_or_default());
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let io = |source| Error::ProxyConnect {
        proxy: proxy.display(),
        source,
    };
    tcp.write_all(head.as_bytes()).await.map_err(io)?;

    // Read the response head byte-chunk-wise; a proxy sends nothing after it until the
    // client speaks, so nothing meant for TLS is consumed.
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > 64 * 1024 {
            return Err(Error::Proxy {
                status: 0,
                message: "The proxy sent an oversized response to CONNECT".into(),
            });
        }
        let n = tcp.read(&mut chunk).await.map_err(io)?;
        if n == 0 {
            return Err(io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the proxy closed the connection during CONNECT",
            )));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let line =
        String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == b'\r').unwrap_or(0)]).into_owned();
    let mut words = line.splitn(3, ' ');
    let _version = words.next();
    let status: u16 = words.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let reason = words.next().unwrap_or("").trim().to_string();
    if (200..300).contains(&status) {
        return Ok(line);
    }
    Err(Error::Proxy {
        status,
        message: if status == 0 {
            format!("The proxy sent an invalid response to CONNECT: `{line}`")
        } else {
            format!("Proxy refused the connection ({status} {reason})")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_matching() {
        let np = NoProxy::parse(
            "example.com, .corp.local  10.0.0.0/8,192.168.1.5 localhost:8080 [::1] internal:9000",
        );
        let cases = [
            ("example.com", 443, true),
            ("api.example.com", 443, true),
            ("notexample.com", 443, false),
            ("x.corp.local", 80, true),
            ("corp.local", 80, true),
            ("10.1.2.3", 80, true),
            ("11.1.2.3", 80, false),
            ("192.168.1.5", 1234, true),
            ("192.168.1.6", 1234, false),
            ("localhost", 8080, true),
            ("localhost", 3000, false),
            ("::1", 80, true),
            ("internal", 9000, true),
            ("internal", 9001, false),
            ("EXAMPLE.COM.", 443, true),
        ];
        for (host, port, expected) in cases {
            assert_eq!(np.matches(host, port), expected, "{host}:{port}");
        }
        assert!(NoProxy::parse("*").matches("anything", 1));
        assert!(!NoProxy::parse("").matches("anything", 1));
        assert!(NoProxy::parse("fd00::/8").matches("fd12::1", 80));
    }

    #[test]
    fn parses_proxy_urls() {
        let p = ProxyServer::parse("http://user:p%40ss@proxy.corp:3128", None).unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.corp", 3128));
        assert_eq!(p.auth, Some(ProxyAuth::new("user", "p@ss")));
        let bare = ProxyServer::parse("proxy.corp", None).unwrap();
        assert_eq!(bare.port, 80);
        assert!(matches!(
            ProxyServer::parse("socks5://proxy:1080", None),
            Err(Error::ProxyUnsupported(_))
        ));
        assert!(matches!(
            ProxyServer::parse("https://proxy:443", None),
            Err(Error::ProxyUnsupported(_))
        ));
        assert!(!format!("{:?}", p.auth).contains("p@ss"));
    }

    #[test]
    fn env_vars_select_per_scheme_and_lowercase_wins() {
        let vars = |name: &str| {
            match name {
                "http_proxy" => Some("http://lower:1"),
                "HTTP_PROXY" => Some("http://upper:2"),
                "ALL_PROXY" => Some("http://all:3"),
                "NO_PROXY" => Some("skip.me"),
                _ => None,
            }
            .map(str::to_string)
        };
        let cfg = ProxyConfig::from_vars(vars).unwrap();
        assert_eq!(cfg.http.as_ref().unwrap().host, "lower");
        assert_eq!(cfg.https.as_ref().unwrap().host, "all");
        let uri = |s: &str| s.parse::<Uri>().unwrap();
        assert!(cfg.select(&uri("http://x.test/")).is_some());
        assert!(cfg.select(&uri("https://skip.me/")).is_none());
        assert!(ProxyConfig::from_vars(|_| None).unwrap().is_none());
    }
}
