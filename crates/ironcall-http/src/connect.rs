//! TCP connection setup with Happy Eyeballs (RFC 8305, simplified).

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio::net::TcpStream;

/// Delay before starting the next connection attempt while earlier ones are pending
/// (RFC 8305 "Connection Attempt Delay").
pub(crate) const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Orders addresses so families alternate, starting with the family of the first
/// resolved address (RFC 8305 §4).
pub(crate) fn interleave(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addrs.first().copied() else {
        return addrs;
    };
    let (mut primary, mut secondary): (Vec<_>, Vec<_>) =
        addrs.into_iter().partition(|a| a.is_ipv6() == first.is_ipv6());
    primary.reverse();
    secondary.reverse();
    let mut out = Vec::with_capacity(primary.len() + secondary.len());
    loop {
        match (primary.pop(), secondary.pop()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b)),
        }
    }
}

/// Connects to the first address that answers. Attempts start `delay` apart, or
/// immediately when the previous one fails; the first success wins and the rest are
/// dropped. Returns the last error if every attempt fails.
pub(crate) async fn connect_happy(addrs: &[SocketAddr], delay: Duration) -> std::io::Result<TcpStream> {
    let mut pending = addrs.iter().copied();
    let mut attempts = FuturesUnordered::new();
    let mut last_err = None;

    if let Some(addr) = pending.next() {
        attempts.push(attempt(addr));
    }
    loop {
        if attempts.is_empty() {
            match pending.next() {
                Some(addr) => attempts.push(attempt(addr)),
                None => return Err(last_err.unwrap_or_else(|| std::io::Error::other("no addresses"))),
            }
        }
        tokio::select! {
            result = attempts.next() => match result {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(e)) => {
                    last_err = Some(e);
                    // Start the next attempt right away instead of waiting out the delay.
                    if let Some(addr) = pending.next() {
                        attempts.push(attempt(addr));
                    }
                }
                None => {}
            },
            () = tokio::time::sleep(delay), if pending.len() > 0 => {
                if let Some(addr) = pending.next() {
                    attempts.push(attempt(addr));
                }
            }
        }
    }
}

async fn attempt(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::time::Instant;

    use tokio::net::TcpListener;

    use super::*;

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 80)
    }
    fn v6(n: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, n)), 80)
    }

    #[test]
    fn interleave_alternates_families_starting_with_first() {
        assert_eq!(
            interleave(vec![v6(1), v6(2), v4(1), v4(2), v4(3)]),
            vec![v6(1), v4(1), v6(2), v4(2), v4(3)]
        );
        assert_eq!(interleave(vec![v4(1), v6(1)]), vec![v4(1), v6(1)]);
        assert!(interleave(vec![]).is_empty());
    }

    /// An address that never answers (TEST-NET-1 is unroutable) must not hold up a
    /// working address behind it for longer than the attempt delay.
    #[tokio::test]
    async fn falls_through_to_next_address_when_first_hangs() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good = listener.local_addr().unwrap();
        let blackhole = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), good.port());

        let start = Instant::now();
        let stream = connect_happy(&[blackhole, good], Duration::from_millis(100))
            .await
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), good);
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn refused_address_moves_on_immediately() {
        // Bind then drop to get a port that refuses connections.
        let refused = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good = listener.local_addr().unwrap();

        let start = Instant::now();
        let stream = connect_happy(&[refused, good], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), good);
        assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn all_failing_returns_last_error() {
        let refused = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let err = connect_happy(&[refused], ATTEMPT_DELAY).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }
}
