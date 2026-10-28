//! Connection pool (PLAN.md §3.4).
//!
//! hyper-util's legacy `Client` can't report whether a connection was reused, so we keep
//! our own. The key includes everything that changes how a connection was set up, so a
//! connection opened with TLS verification off is never handed to a request with it on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hyper::body::Incoming;
use hyper::client::conn::{http1, http2};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::body::ReqBody;
use crate::engine::{HttpVersionPref, Protocol};
use crate::proxy::ProxyServer;
use crate::trace::TlsInfo;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub verify_tls: bool,
    pub version: HttpVersionPref,
    /// The proxy the connection goes through (including its credentials).
    pub proxy: Option<ProxyServer>,
    /// Identity of the extra CA set the connection was verified against, if any.
    pub roots: Option<[u8; 32]>,
}

pub(crate) enum Sender {
    H1(http1::SendRequest<ReqBody>),
    H2(http2::SendRequest<ReqBody>),
}

impl Sender {
    pub fn protocol(&self) -> Protocol {
        match self {
            Sender::H1(_) => Protocol::Http1_1,
            Sender::H2(_) => Protocol::Http2,
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Sender::H1(s) => s.is_closed(),
            Sender::H2(s) => s.is_closed(),
        }
    }

    pub async fn send(
        &mut self,
        req: http::Request<ReqBody>,
    ) -> Result<http::Response<Incoming>, hyper::Error> {
        match self {
            Sender::H1(s) => {
                s.ready().await?;
                s.send_request(req).await
            }
            Sender::H2(s) => {
                s.ready().await?;
                s.send_request(req).await
            }
        }
    }
}

/// A connection plus the ID we report to callers, so tests and the UI can tell
/// which requests shared a connection.
pub(crate) struct Pooled {
    pub id: u64,
    pub sender: Sender,
    /// When the connection was last returned to (or, for HTTP/2, taken from) the pool.
    pub last_used: Instant,
    /// TLS details from the handshake; `None` for plain HTTP.
    pub tls: Option<Arc<TlsInfo>>,
}

pub(crate) struct Pool {
    idle: Mutex<HashMap<PoolKey, Vec<Pooled>>>,
    /// HTTP/1.1 concurrency per origin; see [`Pool::host_permit`].
    hosts: Mutex<HashMap<(bool, String, u16), Arc<Semaphore>>>,
    idle_timeout: Duration,
    max_per_host: usize,
}

impl Pool {
    pub fn new(idle_timeout: Duration, max_per_host: usize) -> Self {
        Self {
            idle: Mutex::default(),
            hosts: Mutex::default(),
            idle_timeout,
            max_per_host,
        }
    }

    /// Takes an HTTP/1.1 connection out of the pool, or clones a shared HTTP/2 one.
    /// Closed and expired connections are dropped along the way.
    pub fn checkout(&self, key: &PoolKey) -> Option<Pooled> {
        let mut idle = self.idle.lock().expect("pool lock poisoned");
        let conns = idle.get_mut(key)?;
        let timeout = self.idle_timeout;
        conns.retain(|c| !c.sender.is_closed() && c.last_used.elapsed() < timeout);
        if let Some(h2) = conns.iter_mut().find(|c| matches!(c.sender, Sender::H2(_))) {
            h2.last_used = Instant::now();
            let Sender::H2(s) = &h2.sender else { unreachable!() };
            return Some(Pooled {
                id: h2.id,
                sender: Sender::H2(s.clone()),
                last_used: h2.last_used,
                tls: h2.tls.clone(),
            });
        }
        conns.pop()
    }

    /// Returns a connection for reuse. HTTP/2 connections stay in the pool while in use,
    /// so they are only added once, when created.
    pub fn checkin(&self, key: PoolKey, mut conn: Pooled) {
        if conn.sender.is_closed() {
            return;
        }
        conn.last_used = Instant::now();
        let mut idle = self.idle.lock().expect("pool lock poisoned");
        let conns = idle.entry(key).or_default();
        if let Sender::H2(_) = conn.sender
            && conns.iter().any(|c| c.id == conn.id)
        {
            return;
        }
        conns.push(conn);
    }

    /// Waits for a slot to run an HTTP/1.1 request against this origin, so at most
    /// `max_per_host` connections to it are busy at once (PLAN.md §3.4).
    pub async fn host_permit(&self, key: &PoolKey) -> OwnedSemaphorePermit {
        let sem = {
            let mut hosts = self.hosts.lock().expect("pool lock poisoned");
            hosts
                .entry((key.https, key.host.clone(), key.port))
                .or_insert_with(|| Arc::new(Semaphore::new(self.max_per_host)))
                .clone()
        };
        sem.acquire_owned().await.expect("host semaphore is never closed")
    }
}
