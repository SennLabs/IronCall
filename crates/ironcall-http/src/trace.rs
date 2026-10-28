//! What happened during a send (PLAN.md Phase 5): the timeline (like `curl -v`), TLS
//! details and informational (1xx) responses.
//!
//! Header values in the timeline are recorded **raw**, including `Authorization`,
//! `Cookie` and `Proxy-Authorization`. Callers must mask sensitive values before showing
//! or storing them.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use http::{HeaderMap, StatusCode};
use rustls::client::ClientConnection;
use rustls::pki_types::CertificateDer;

use crate::error::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineKind {
    /// Resolving, connecting, reusing a connection, retries.
    Info,
    /// TLS handshake result.
    Tls,
    /// Proxy `CONNECT` request and response.
    Proxy,
    /// Request line and headers as sent.
    RequestHeaders,
    /// Request body summary.
    RequestBody,
    /// Status line and headers received (including 1xx).
    ResponseHeaders,
    /// Response body summary.
    ResponseBody,
    Redirect,
    Error,
}

/// One timeline entry. `headers` is set for request/response/CONNECT head entries,
/// with values exactly as sent or received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEvent {
    /// Time since the send started.
    pub at: Duration,
    pub kind: TimelineKind,
    pub message: String,
    pub headers: Vec<(String, String)>,
}

/// The server's leaf certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    /// Unix seconds.
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    /// DNS names and IP addresses from subjectAltName.
    pub sans: Vec<String>,
    /// Certificates the server sent (leaf plus intermediates).
    pub chain_length: u32,
}

/// TLS details of a connection, captured when it was set up (so reused connections
/// report them too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsInfo {
    /// e.g. `TLSv1.3`.
    pub version: String,
    /// e.g. `TLS13_AES_128_GCM_SHA256`.
    pub cipher: String,
    pub alpn: Option<String>,
    /// False if certificate verification was turned off.
    pub verified: bool,
    pub certificate: Option<CertInfo>,
}

impl TlsInfo {
    pub(crate) fn from_connection(conn: &ClientConnection, verified: bool) -> Self {
        let version = match conn.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3".to_string(),
            Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2".to_string(),
            Some(other) => format!("{other:?}"),
            None => "unknown".to_string(),
        };
        let cipher = conn
            .negotiated_cipher_suite()
            .map(|s| format!("{:?}", s.suite()))
            .unwrap_or_else(|| "unknown".to_string());
        let alpn = conn
            .alpn_protocol()
            .map(|p| String::from_utf8_lossy(p).into_owned());
        let certificate = conn.peer_certificates().and_then(cert_info);
        Self {
            version,
            cipher,
            alpn,
            verified,
            certificate,
        }
    }

    /// One-line summary for the timeline.
    pub(crate) fn summary(&self) -> String {
        let mut s = format!("{} {}", self.version, self.cipher);
        if let Some(alpn) = &self.alpn {
            s.push_str(&format!(", ALPN {alpn}"));
        }
        if !self.verified {
            s.push_str(", certificate NOT verified");
        }
        s
    }
}

fn cert_info(chain: &[CertificateDer<'_>]) -> Option<CertInfo> {
    let (_, cert) = x509_parser::parse_x509_certificate(chain.first()?).ok()?;
    let mut sans = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for name in &ext.value.general_names {
            match name {
                x509_parser::extensions::GeneralName::DNSName(dns) => sans.push(dns.to_string()),
                x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                    if let Ok(v4) = <[u8; 4]>::try_from(*bytes) {
                        sans.push(Ipv4Addr::from(v4).to_string());
                    } else if let Ok(v6) = <[u8; 16]>::try_from(*bytes) {
                        sans.push(Ipv6Addr::from(v6).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    let validity = cert.validity();
    Some(CertInfo {
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        not_before: Some(validity.not_before.timestamp()),
        not_after: Some(validity.not_after.timestamp()),
        sans,
        chain_length: chain.len() as u32,
    })
}

/// An informational (1xx) response received before the final response.
#[derive(Clone, Debug)]
pub struct Informational {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// Time since the request was sent on the connection.
    pub after_send: Duration,
}

/// Collects timeline events for one send. Shared with hyper's informational callback,
/// which runs on the connection task.
#[derive(Debug)]
pub(crate) struct Recorder {
    start: Instant,
    events: Mutex<Vec<TimelineEvent>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            events: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, kind: TimelineKind, message: impl Into<String>) {
        self.push_headers(kind, message, Vec::new());
    }

    pub fn push_headers(
        &self,
        kind: TimelineKind,
        message: impl Into<String>,
        headers: Vec<(String, String)>,
    ) {
        let event = TimelineEvent {
            at: self.start.elapsed(),
            kind,
            message: message.into(),
            headers,
        };
        self.events.lock().expect("timeline lock poisoned").push(event);
    }

    pub fn take(&self) -> Vec<TimelineEvent> {
        std::mem::take(&mut *self.events.lock().expect("timeline lock poisoned"))
    }
}

/// Header map as `(name, value)` pairs in order; non-UTF-8 values are shown lossily.
pub(crate) fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(n, v)| (n.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect()
}

pub(crate) fn ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

/// A failed send with what happened before the failure.
#[derive(Debug)]
pub struct Failure {
    pub error: Error,
    pub timeline: Vec<TimelineEvent>,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
