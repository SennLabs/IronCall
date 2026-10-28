# IronCall task runner. `just --list` shows all recipes.

set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# Windows installs Python as `python`; `python3` there is often the Microsoft Store stub.
python := if os_family() == "windows" { "python" } else { "python3" }

# Run the desktop app in development mode
dev:
    cargo tauri dev

# Everything that must pass before a commit (PLAN.md §8.1)
check: fmt-check lint test deny

# Format all Rust and frontend code
fmt:
    cargo fmt --all
    npm --prefix ui run format

fmt-check:
    cargo fmt --all -- --check
    npm --prefix ui run format:check

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    npm --prefix ui run lint

test:
    cargo test --workspace
    npm --prefix ui run test

# From Linux: compile and lint everything for Windows (needs the x86_64-pc-windows-gnu
# target and mingw-w64). Catches Windows-only compile errors and warnings early.
check-windows:
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings

# From Linux: run the core, store and secrets tests as Windows binaries under Wine (needs
# `wine` too). Not a substitute for Windows: Wine lacks CryptoAPI (TLS verification),
# real creation times and icacls, and insta snapshot paths break under it.
test-wine:
    WINEDEBUG=-all CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --target x86_64-pc-windows-gnu -p ironcall-core -p ironcall-store -p ironcall-secrets

# Advisory DB fetches are occasionally flaky; fall back to the cached copy.
deny:
    cargo deny fetch || echo "warning: advisory DB fetch failed; checking against the cached copy"
    cargo deny --offline check

# End-to-end tests against a debug build; name suites to run a subset (`just e2e smoke network`)
e2e *suites:
    cargo tauri build --debug --no-bundle
    {{python}} tests/e2e/run.py {{suites}}

# Benchmarks: timings vs curl, and the 100MB body viewer index/paging (release build)
bench:
    cargo run -p ironcall-http --example timing_compare
    cargo test -p ironcall-store --release -- --ignored bench_100mb --nocapture

# Capture the README/docs screenshots from the real app into docs/images (debug build)
screenshots:
    cargo tauri build --debug --no-bundle
    {{python}} tests/e2e/run.py screenshots_docs

# Regenerate TypeScript IPC bindings from Rust (written to ui/src/ipc/bindings.ts)
bindings:
    cargo test -p ironcall-app export_bindings

# Idle memory budget (PLAN.md Phase 8, ADR 0008): release build, run on a real display
idle-memory:
    cargo tauri build --no-bundle
    {{python}} tests/perf/idle_memory.py

# Production build of the desktop app (installers for the current OS)
build:
    cargo tauri build
