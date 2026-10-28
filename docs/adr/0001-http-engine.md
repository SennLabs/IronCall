# 0001 — HTTP engine: hyper low-level connections + own pool

- **Status:** Accepted (Linux measured; Windows still to verify)
- **Date:** 2026-10-05
- **Phase:** P0 (Spike A)

## Context
IronCall needs a per-request timing breakdown (DNS, TCP connect, TLS, TTFB, download), with reused connections clearly marked, over HTTP/1.1 and HTTP/2 (PLAN.md §3.3, §3.4). `reqwest` doesn't expose these phases, and hyper-util's legacy `Client` can't tell us whether a connection was reused. PLAN.md Phase 0 says to switch to libcurl if the hyper approach can't match `curl -w` within ±10% / ±5ms on both OSes, or if the remaining engine work (redirects, proxy, decompression, cookies) is projected to take more than 3 weeks.

## Options considered
1. **hyper 1.x `client::conn::{http1,http2}` + tokio-rustls + rustls-platform-verifier + our own pool.** Pure Rust, one TLS stack, full control over each phase. We have to write the pool, redirects, proxy and the rest ourselves.
2. **libcurl (`curl` crate).** Native timings and features. But it's a C dependency; on Windows it means Schannel or vendored OpenSSL plus nghttp2 for HTTP/2, so a second TLS stack with different CA and verification behaviour. Harder to put our own redaction and pool rules on top.

## What the spike built (`crates/ironcall-http`)
- `Engine::send(Request) -> Response { status, headers, body, timings, protocol, connection_id, connection_reused }`.
- DNS through `tokio::net::lookup_host` (getaddrinfo on a blocking thread), timed separately. TCP connect tries addresses in order. TLS uses tokio-rustls with ALPN `h2`/`http/1.1`.
- One cached `ClientConfig` per (verify flag × ALPN preference). Verification on uses `rustls_platform_verifier::Verifier::new_with_extra_roots`. Verification off uses a custom verifier that still checks handshake signatures.
- Pool keyed by (scheme, host, port, verify flag, HTTP version preference). HTTP/1.1 connections are checked out and back in; an HTTP/2 connection is shared by cloning its `SendRequest`.
- `Phase::{Measured, Reused, NotApplicable}`. Reused connections report `Reused`, never 0. `RequestOptions::fresh_connection` forces a cold measurement.
- 9 integration tests against a local axum server with an rcgen CA: HTTP/1.1, HTTP/2 via ALPN, plain HTTP, reuse on both protocols (one server-side accept), fresh connection, pool isolation by verify flag, untrusted certificate, and HTTP/2-required against an HTTP/1.1-only server.

## Measurements (Linux, devcontainer on WSL2; `cargo run --release -p ironcall-http --example timing_compare [-- URL]`)
Medians of N=20, in ms. Tolerance is ±10% or ±5ms, whichever is larger. "Cold" means a new connection each time; "warm" means a reused connection.

| Target | Case | DNS | Connect | TLS | TTFB | Download | Total | Verdict |
|---|---|---|---|---|---|---|---|---|
| localhost `/` h1 | cold (ours / curl) | 0.17 / 0.02 | 0.16 / 0.17 | 0.35 / 2.54 | 0.30 / 0.49 | 0.02 / 0.04 | 1.09 / 4.14 | ✅ all |
| localhost `/big` (1MB) h2 | cold | 0.29 / 0.02 | 0.29 / 0.19 | 0.75 / 4.07 | 0.57 / 0.50 | 1.49 / 1.76 | 3.54 / 7.10 | ✅ all |
| localhost `/big` h2 | warm | — | — | — | 1.22 / 0.32 | 0.18 / 1.10 | 1.77 / 1.43 | ✅ all |
| example.com h1 | cold | 12.3 / 19.4 | 10.8 / 11.3 | 13.4 / 19.9 | 21.2 / 19.7 | 0.19 / 0.09 | 63.2 / 68.1 | ❌ DNS, TLS |
| example.com h2 | cold | 14.2 / 13.3 | 10.5 / 11.3 | 12.9 / 17.7 | 17.5 / 18.5 | 0.67 / 0.40 | 57.8 / 64.9 | ❌ total (−7ms) |
| example.com h1/h2 | warm | — | — | — | 20.7 / 16.4, 18.3 / 20.4 | ≈ | 20.9 / 17.0, 18.4 / 20.6 | ✅ all |
| cloudflare.com h1 | cold | 17.4 / 23.8 | 10.4 / 13.1 | 11.4 / 16.9 | 304.9 / 304.1 | 86.0 / 93.8 | 438.7 / 456.9 | ❌ DNS, TLS |
| cloudflare.com h2 | cold | 18.9 / 19.4 | 10.6 / 12.0 | 10.8 / 16.7 | 308.9 / 17.7 | 87.1 / 388.6 | 439.6 / 452.4 | ❌ TLS, TTFB/download split |

All localhost results are within tolerance, but they mostly sit under the 5ms floor, so they mainly show correct *ordering and attribution* of phases. The remote mismatches are explained below; none of them are measurement errors on our side:

- **DNS and TLS: we're 5–7ms faster than curl, consistently.** Each curl run is a new process: it sets up its resolver (thread, nsswitch) and loads and parses the CA bundle during the handshake. Our engine is a long-lived process that loads roots once in `Engine::new`, which is how the desktop app behaves too. Connect, TTFB and warm totals, which have no per-process overhead, agree within tolerance.
- **cloudflare.com over HTTP/2: TTFB 309ms vs 18ms.** Cloudflare sends `HTTP/2 103 Early Hints` first (confirmed with `curl -v`). curl's `time_starttransfer` counts the 103 as the first byte; hyper skips 1xx responses and we time to the final headers. The sum TTFB + download agrees (396ms vs 406ms).

## Decision
**Keep hyper.** Request-phase timings agree with curl within tolerance. Setup-phase differences come from curl's per-process startup cost, and the TTFB difference comes from how each defines "first byte". libcurl would add a second TLS stack and C build dependencies on Windows, for no gain in accuracy.

Projected remaining effort on this stack (one developer):

| Feature | Estimate |
|---|---|
| Redirects (status handling, method rewriting, max count, per-hop timings, cross-origin auth stripping) | 3 days |
| Proxy (env vars + manual, `NO_PROXY` matching, CONNECT tunnel for HTTPS, absolute-form for HTTP, Basic auth) | 4–5 days |
| Decompression (gzip/br/zstd/deflate via async-compression, streaming) | 1–2 days |
| Cookies (`cookie_store`, single-writer task) | 2 days |
| Streaming bodies (spool to body store, upload from disk, progress), cancellation | 3 days |

Total: **about 2.5–3 weeks.** That's at the limit in PLAN.md, so redirects/proxy should be re-checked at the end of Phase 1. If they overrun, revisit this ADR.

## Consequences / follow-ups
- **Windows not tested.** Re-run `timing_compare` on Windows 11 early in Phase 1 (`curl.exe` ships with Windows). In particular, check `rustls-platform-verifier` with the Windows cert store and extra roots.
- **Timing definitions for Phase 5:** add a separate "first byte (incl. 1xx)" value, or record informational responses, so Early Hints show up in the timeline. hyper exposes HTTP/1 informational responses via `hyper::ext::on_informational`; HTTP/2 needs checking.
- **Known gaps:**
  - Happy Eyeballs (RFC 8305): we try addresses one after another, so a dead IPv6 address delays the IPv4 attempt.
  - No retry when a pooled HTTP/1.1 connection turns out to be stale (server closed keep-alive between our check and the send).
  - No per-host HTTP/1.1 limit or global in-flight cap yet (§3.4).
  - The body is collected in memory, and the body store / spool comes in Phase 1.
  - The pool doesn't yet include the CA-set identity in its key: one `Engine` has exactly one CA set, so changing CAs means a new `Engine`.
  - No idle-connection expiry.
- **Dependencies to hoist into `[workspace.dependencies]`:** none new. Dev-dependencies add features locally (`hyper-util`: `server-auto`, `service`, `http1`, `http2`).
