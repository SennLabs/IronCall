//! Real-world specs (Stripe, GitHub). They're large (several MB) and not committed, so
//! this test downloads them into the build directory and is ignored by default:
//!
//! ```sh
//! cargo test -p ironcall-import --release --test openapi_big -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Instant;

use ironcall_import::openapi::plan;

const SPECS: &[(&str, &str, usize)] = &[
    (
        "stripe-spec3.json",
        "https://raw.githubusercontent.com/stripe/openapi/master/openapi/spec3.json",
        400,
    ),
    (
        "github-api.json",
        "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json",
        800,
    ),
];

fn cache_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("openapi-cache")
}

/// Downloads with the system `curl` (avoids an HTTP client dev-dependency).
fn fetch(name: &str, url: &str) -> Vec<u8> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    if !path.exists() {
        let status = std::process::Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&path)
            .arg(url)
            .status()
            .expect("curl is needed to download the specs");
        assert!(status.success(), "download of {url} failed");
    }
    std::fs::read(&path).unwrap()
}

#[test]
#[ignore = "downloads large specs from GitHub"]
fn stripe_and_github_import_without_errors() {
    for (name, url, min_requests) in SPECS {
        let bytes = fetch(name, url);
        let start = Instant::now();
        let plan = plan(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let elapsed = start.elapsed();
        let folders = plan
            .tree
            .iter()
            .filter(|i| matches!(i, ironcall_import::openapi::PlanItem::Folder { .. }))
            .count();
        println!(
            "{name}: {:.1} MB, {} requests in {folders} folders, {} env vars, {} warnings, planned in {elapsed:?}",
            bytes.len() as f64 / 1e6,
            plan.request_count(),
            plan.environment.len(),
            plan.warnings.len(),
        );
        for w in plan.warnings.iter().take(5) {
            println!(
                "  warning: {} ({})",
                w.message,
                w.location.as_deref().unwrap_or("-")
            );
        }
        assert!(
            plan.request_count() >= *min_requests,
            "{name}: only {} requests",
            plan.request_count()
        );
        assert!(
            !plan.environment[0].value.is_empty(),
            "{name}: baseUrl from servers"
        );
    }
}
