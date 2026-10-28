//! Throughput and memory benchmark for the streaming JSON pretty-printer (Linux only:
//! peak memory comes from `/proc/self/status`).
//! Run with `cargo test --release -p ironcall-core --test pretty_bench -- --ignored --nocapture`.

#[cfg(target_os = "linux")]
fn status_kb(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches(" kB").trim().parse().ok())
        .unwrap()
}

#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn bench_100mb_json() {
    let row = r#"{"id":12345,"name":"row with \"escapes\" é","tags":["a","b"],"n":1.5e3,"ok":true},"#;
    let mut input = String::from("[");
    while input.len() < 100 * 1024 * 1024 {
        input.push_str(row);
    }
    input.push_str("null]");
    input.shrink_to_fit();

    // Reset the peak-RSS mark so only the pretty-printer's own memory shows up.
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
    let before = status_kb("VmRSS:");
    let start = std::time::Instant::now();
    let stats = ironcall_core::content::pretty_json_stream(input.as_bytes(), std::io::sink()).unwrap();
    let secs = start.elapsed().as_secs_f64();
    let peak_extra = status_kb("VmHWM:").saturating_sub(before);

    eprintln!(
        "pretty_json_stream: {:.0} MB in {secs:.2}s = {:.0} MB/s, output {:.0} MB, peak RSS growth {peak_extra} KB",
        stats.bytes_in as f64 / 1e6,
        stats.bytes_in as f64 / 1e6 / secs,
        stats.bytes_out as f64 / 1e6,
    );
    assert!(peak_extra < 4 * 1024, "streaming should stay within a few MB");
}
