//! Spike A measurement: compares IronCall's timing breakdown against `curl -w`.
//!
//! Local test server (default):  cargo run -p ironcall-http --example timing_compare
//! Remote URL (system roots):    cargo run -p ironcall-http --example timing_compare -- https://example.com/
//!
//! Each case runs N cold requests (new connection) and N warm requests (reused connection)
//! with both clients and compares medians. Tolerance: ±10% or ±5ms, whichever is larger.

#[path = "../tests/support/mod.rs"]
mod support;

use std::process::Command;
use std::time::Duration;

use ironcall_http::{Engine, EngineConfig, HttpVersionPref, Request, Timings};

const N: usize = 20;
const CURL_FORMAT: &str = "%{time_namelookup} %{time_connect} %{time_appconnect} %{time_pretransfer} %{time_starttransfer} %{time_total}\\n";

/// Phase durations in milliseconds: dns, connect, tls, ttfb, download, total.
type Sample = [f64; 6];
const PHASES: [&str; 6] = ["dns", "connect", "tls", "ttfb", "download", "total"];

#[tokio::main]
async fn main() {
    let remote = std::env::args().nth(1);
    let server;
    let (engine, ca_file, urls): (Engine, Option<std::path::PathBuf>, Vec<String>) = match &remote {
        Some(url) => (
            Engine::new(EngineConfig::default()).unwrap(),
            None,
            vec![url.clone()],
        ),
        None => {
            server = support::TestServer::start().await;
            let ca_file =
                std::env::temp_dir().join(format!("ironcall-spike-a-ca-{}.pem", std::process::id()));
            std::fs::write(&ca_file, &server.ca_pem).unwrap();
            let engine = Engine::new(EngineConfig {
                extra_roots: vec![server.ca.clone()],
                ..EngineConfig::default()
            })
            .unwrap();
            (
                engine,
                Some(ca_file),
                vec![server.https_url("/"), server.https_url("/big")],
            )
        }
    };

    let mut all_ok = true;
    for url in &urls {
        for (version, curl_flag) in [
            (HttpVersionPref::Http1, "--http1.1"),
            (HttpVersionPref::Http2, "--http2"),
        ] {
            println!("\n## {url} {version:?} (N={N}, medians in ms)");
            let ours_cold = ours(&engine, url, version, true).await;
            let curl_cold = curl_cold(url, curl_flag, ca_file.as_deref());
            all_ok &= report("cold", &ours_cold, &curl_cold, &[0, 1, 2, 3, 4, 5]);

            let ours_warm = ours(&engine, url, version, false).await;
            let curl_warm = curl_warm(url, curl_flag, ca_file.as_deref());
            // DNS/connect/TLS don't happen on a reused connection; compare request phases only.
            all_ok &= report("warm", &ours_warm, &curl_warm, &[3, 4, 5]);
        }
    }
    if let Some(f) = ca_file {
        let _ = std::fs::remove_file(f);
    }
    println!(
        "\nOverall: {}",
        if all_ok {
            "ALL WITHIN TOLERANCE"
        } else {
            "SOME PHASES OUT OF TOLERANCE"
        }
    );
}

async fn ours(engine: &Engine, url: &str, version: HttpVersionPref, cold: bool) -> Vec<Sample> {
    // Prime the pool so warm runs start with an open connection.
    let mut prime = Request::get(url).unwrap();
    prime.options.http_version = version;
    engine.send(prime).await.unwrap();

    let mut samples = Vec::with_capacity(N);
    for _ in 0..N {
        let mut req = Request::get(url).unwrap();
        req.options.http_version = version;
        req.options.fresh_connection = cold;
        let resp = engine.send(req).await.unwrap();
        assert_eq!(resp.connection_reused, !cold);
        samples.push(to_sample(&resp.timings));
    }
    samples
}

fn to_sample(t: &Timings) -> Sample {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let phase = |p: ironcall_http::Phase| p.duration().map(ms).unwrap_or(0.0);
    [
        phase(t.dns),
        phase(t.connect),
        phase(t.tls),
        ms(t.ttfb),
        ms(t.download),
        ms(t.total),
    ]
}

fn curl_cold(url: &str, flag: &str, ca: Option<&std::path::Path>) -> Vec<Sample> {
    (0..N).flat_map(|_| run_curl(url, flag, ca, 1)).collect()
}

/// One curl process fetching the URL N+1 times over one connection; the first is dropped.
fn curl_warm(url: &str, flag: &str, ca: Option<&std::path::Path>) -> Vec<Sample> {
    run_curl(url, flag, ca, N + 1).into_iter().skip(1).collect()
}

fn run_curl(url: &str, flag: &str, ca: Option<&std::path::Path>, times: usize) -> Vec<Sample> {
    let mut cmd = Command::new("curl");
    cmd.args(["-s", flag, "-w", CURL_FORMAT]);
    if let Some(ca) = ca {
        cmd.arg("--cacert").arg(ca);
    }
    for _ in 0..times {
        cmd.args(["-o", "/dev/null", url]);
    }
    let out = cmd.output().expect("curl must be installed");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let v: Vec<f64> = line
                .split_whitespace()
                .map(|x| x.parse::<f64>().unwrap() * 1000.0)
                .collect();
            let (lookup, connect, app, pre, start, total) = (v[0], v[1], v[2], v[3], v[4], v[5]);
            // curl reports cumulative offsets; convert to per-phase durations.
            [
                lookup,
                connect - lookup,
                app - connect,
                start - pre,
                total - start,
                total,
            ]
        })
        .collect()
}

fn median(samples: &[Sample], i: usize) -> f64 {
    let mut v: Vec<f64> = samples.iter().map(|s| s[i]).collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn report(label: &str, ours: &[Sample], curl: &[Sample], phases: &[usize]) -> bool {
    println!("| {label} | phase | ironcall | curl | diff | ok |");
    let mut ok_all = true;
    for &i in phases {
        let (a, b) = (median(ours, i), median(curl, i));
        let tolerance = (b.abs() * 0.10).max(5.0);
        let ok = (a - b).abs() <= tolerance;
        ok_all &= ok;
        println!(
            "| | {:<8} | {a:>8.3} | {b:>8.3} | {:>+8.3} | {} |",
            PHASES[i],
            a - b,
            if ok { "yes" } else { "NO" }
        );
    }
    ok_all
}
