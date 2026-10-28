//! HTTP engine for IronCall: hyper + rustls, own connection pool, timings, redirects,
//! decompression, cancellation, file and multipart bodies, HTTP proxies, custom CAs and
//! a cookie jar (PLAN.md §3.3, §3.4, Phase 4, docs/adr/0001-http-engine.md).

mod body;
mod ca;
mod connect;
mod cookies;
mod decode;
pub mod echo;
mod engine;
mod error;
mod pool;
mod proxy;
mod redirect;
mod timings;
mod tls;
mod trace;

pub use body::{Body, Part};
pub use ca::{CaError, CaInfo, RootSet, inspect_ca_file, load_ca_file};
pub use cookies::{CookieInfo, CookieJar, SameSite};
pub use proxy::{NoProxy, ProxyAuth, ProxyConfig, ProxyServer};

pub use engine::{
    DEFAULT_USER_AGENT, Engine, EngineConfig, Hop, HttpVersionPref, Progress, Protocol, Request,
    RequestOptions, Response, ResponseHead, Stage, engine_headers,
};
pub use error::{Error, ErrorKind};
pub use timings::{Phase, Timings};
pub use tokio_util::sync::CancellationToken;
pub use trace::{CertInfo, Failure, Informational, TimelineEvent, TimelineKind, TlsInfo};
