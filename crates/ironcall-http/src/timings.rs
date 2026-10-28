use std::time::Duration;

/// Duration of a connection-setup phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Measured on a new connection.
    Measured(Duration),
    /// The request used a pooled connection, so this phase didn't happen.
    Reused,
    /// The phase doesn't apply (e.g. TLS on plain HTTP).
    NotApplicable,
}

impl Phase {
    pub fn duration(self) -> Option<Duration> {
        match self {
            Phase::Measured(d) => Some(d),
            Phase::Reused | Phase::NotApplicable => None,
        }
    }
}

/// Per-request timing breakdown (PLAN.md Phase 5 waterfall; captured from Phase 1).
///
/// Phases are durations, not cumulative offsets. To compare with `curl -w`:
/// dns = time_namelookup, connect = time_connect - time_namelookup,
/// tls = time_appconnect - time_connect, ttfb = time_starttransfer - time_pretransfer,
/// download = time_total - time_starttransfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    pub dns: Phase,
    pub connect: Phase,
    pub tls: Phase,
    /// From sending the request on the connection to receiving the response headers.
    pub ttfb: Duration,
    /// From response headers to the end of the body.
    pub download: Duration,
    pub total: Duration,
}
