use std::time::Duration;

use rustls::CertificateError;

use crate::engine::Stage;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported URL scheme `{0}` (only http and https)")]
    UnsupportedScheme(String),
    #[error("DNS lookup for {host} failed: {source}")]
    Dns {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error("DNS lookup for {0} returned no addresses")]
    NoAddresses(String),
    #[error("could not connect to {host}: {source}")]
    Connect {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error("TLS handshake with {host} failed: {source}")]
    Tls {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error("server negotiated HTTP/1.1 but HTTP/2 was required")]
    Http2NotNegotiated,
    #[error("TLS configuration error: {0}")]
    TlsConfig(#[from] rustls::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] hyper::Error),
    #[error("invalid request: {0}")]
    InvalidRequest(#[from] http::Error),
    #[error("request timed out after {limit:?} while {}", stage.describe())]
    Timeout { limit: Duration, stage: Stage },
    #[error("request was cancelled")]
    Cancelled,
    #[error("stopped after {0} redirects")]
    TooManyRedirects(u32),
    #[error("invalid redirect location `{location}`: {reason}")]
    InvalidRedirect { location: String, reason: String },
    #[error("could not decode `{encoding}` response body: {source}")]
    Decompress {
        encoding: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not store response body: {0}")]
    Io(#[source] std::io::Error),
    #[error("could not read body file {}: {source}", path.display())]
    BodyFile {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not connect to proxy {proxy}: {source}")]
    ProxyConnect {
        proxy: String,
        #[source]
        source: std::io::Error,
    },
    #[error("proxy error ({status}): {message}")]
    Proxy { status: u16, message: String },
    #[error("invalid proxy setting: {0}")]
    InvalidProxy(String),
    #[error("{0}")]
    ProxyUnsupported(String),
}

/// Error categories, matching `SendErrorKind` in the app's IPC contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidRequest,
    InvalidUrl,
    Dns,
    Connect,
    Tls,
    Timeout,
    Cancelled,
    TooManyRedirects,
    Protocol,
    Io,
    Unsupported,
    Internal,
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::InvalidUrl(_) => ErrorKind::InvalidUrl,
            Error::UnsupportedScheme(_) => ErrorKind::Unsupported,
            Error::Dns { .. } | Error::NoAddresses(_) => ErrorKind::Dns,
            Error::Connect { .. } | Error::ProxyConnect { .. } | Error::Proxy { .. } => ErrorKind::Connect,
            Error::BodyFile { .. } | Error::InvalidProxy(_) => ErrorKind::InvalidRequest,
            Error::ProxyUnsupported(_) => ErrorKind::Unsupported,
            Error::Tls { .. } => ErrorKind::Tls,
            Error::TlsConfig(_) => ErrorKind::Internal,
            Error::Http2NotNegotiated
            | Error::Http(_)
            | Error::InvalidRedirect { .. }
            | Error::Decompress { .. } => ErrorKind::Protocol,
            Error::InvalidRequest(_) => ErrorKind::InvalidRequest,
            Error::Timeout { .. } => ErrorKind::Timeout,
            Error::Cancelled => ErrorKind::Cancelled,
            Error::TooManyRedirects(_) => ErrorKind::TooManyRedirects,
            Error::Io(_) => ErrorKind::Io,
        }
    }

    /// One-line, user-facing message. `Display` (and the source chain) has the details.
    pub fn message(&self) -> String {
        match self {
            Error::InvalidUrl(e) => format!("The URL is not valid: {e}"),
            Error::UnsupportedScheme(s) => format!("`{s}` URLs are not supported; use http or https"),
            Error::Dns { host, .. } => format!("Could not find the server `{host}` (DNS lookup failed)"),
            Error::NoAddresses(host) => format!("The server `{host}` has no addresses"),
            Error::Connect { host, source } => match source.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    format!("Connection refused by {host}: nothing is listening on that port")
                }
                std::io::ErrorKind::TimedOut => format!("Connecting to {host} timed out"),
                _ => format!("Could not connect to {host}: {source}"),
            },
            Error::Tls { host, source } => {
                format!("Secure connection to {host} failed: {}", tls_reason(source, host))
            }
            Error::Http2NotNegotiated => "The server does not support HTTP/2, but HTTP/2 was required".into(),
            Error::TlsConfig(e) => format!("TLS is misconfigured: {e}"),
            Error::Http(e) => format!("The server sent an invalid response or closed the connection: {e}"),
            Error::InvalidRequest(e) => format!("The request is not valid: {e}"),
            Error::Timeout { limit, stage } => {
                format!("Timed out after {} while {}", humanize(*limit), stage.describe())
            }
            Error::Cancelled => "Request cancelled".into(),
            Error::TooManyRedirects(n) => format!("Stopped after {n} redirects (possible redirect loop)"),
            Error::InvalidRedirect { location, .. } => {
                format!("The server redirected to an invalid location `{location}`")
            }
            Error::Decompress { encoding, .. } => format!("The response body is not valid `{encoding}` data"),
            Error::Io(e) => format!("Could not save the response body: {e}"),
            Error::BodyFile { path, source } => match source.kind() {
                std::io::ErrorKind::NotFound => format!("The file {} doesn't exist", path.display()),
                _ => format!("Could not read the file {}: {source}", path.display()),
            },
            Error::ProxyConnect { proxy, source } => match source.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    format!("Connection refused by the proxy {proxy}: is it running?")
                }
                _ => format!("Could not connect to the proxy {proxy}: {source}"),
            },
            Error::Proxy { message, .. } => message.clone(),
            Error::InvalidProxy(e) => format!("The proxy setting is not valid: {e}"),
            Error::ProxyUnsupported(e) => e.clone(),
        }
    }

    /// True if a request that failed on a pooled connection can be retried on a new one.
    ///
    /// `is_canceled` means hyper never wrote the request (the connection was already
    /// closed), so any method is safe. If the connection closed mid-exchange the server
    /// may already have acted on the request, so only idempotent methods (RFC 9110
    /// §9.2.2) are retried, avoiding duplicate POSTs.
    pub(crate) fn is_safe_to_retry(&self, method: &http::Method) -> bool {
        use http::Method;
        let Error::Http(e) = self else {
            return false;
        };
        let idempotent = matches!(
            *method,
            Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE | Method::PUT | Method::DELETE
        );
        e.is_canceled() || (idempotent && (e.is_incomplete_message() || e.is_closed()))
    }
}

/// Describes why a TLS handshake failed in plain words.
fn tls_reason(err: &std::io::Error, host: &str) -> String {
    let Some(rustls_err) = err.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()) else {
        return err.to_string();
    };
    match rustls_err {
        rustls::Error::InvalidCertificate(cert) => match cert {
            CertificateError::UnknownIssuer => "certificate is not trusted (unknown issuer)".into(),
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                "certificate expired".into()
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "certificate is not valid yet".into()
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                format!("certificate not valid for host {host}")
            }
            CertificateError::Revoked => "certificate has been revoked".into(),
            CertificateError::BadSignature => "certificate has an invalid signature".into(),
            CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => {
                "certificate is not valid for server authentication".into()
            }
            other => format!("certificate rejected ({other:?})"),
        },
        rustls::Error::AlertReceived(alert) => format!("server sent TLS alert `{alert:?}`"),
        rustls::Error::NoCertificatesPresented => "server sent no certificate".into(),
        other => other.to_string(),
    }
}

fn humanize(d: Duration) -> String {
    if d.as_millis() < 1000 {
        format!("{} ms", d.as_millis())
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}
