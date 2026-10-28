# Contributing to IronCall

Read [PLAN.md](PLAN.md) first. It defines the scope of each phase, the architecture and the file format.

## Setup
- **Linux:** install the packages and tools listed in [building.md](building.md#prerequisites) (Tauri system packages, `just`, `tauri-cli`, `cargo-deny`, and `tauri-driver`, `webkit2gtk-driver` and Xvfb for E2E).
- **Windows:** install Rust (MSVC toolchain), Visual Studio Build Tools ("Desktop development with C++"), Node 24+, Python 3 (for E2E), `just` (`winget install Casey.Just`), then `cargo install tauri-cli --version "^2" --locked`, `cargo install tauri-driver --version "^2" --locked` and `cargo install cargo-deny --locked`. WebView2 is built into Windows 11; `just e2e` downloads the matching `msedgedriver` into `target/e2e/`.
- Then: `npm --prefix ui install`, and `just dev` to run the app.
- `.gitattributes` keeps line endings LF on every OS, so no `core.autocrlf` setup is needed.

## Testing
- `just check`: unit and integration tests, plus formatting, lints and `cargo deny`.
- `just e2e [suite ...]`: builds a debug app and drives it over WebDriver (`tests/e2e/run.py`). Suites use a temporary data dir via `IRONCALL_DATA_DIR`, never your real app data.
- `just check-windows` (on Linux): clippy for the Windows target, to catch Windows-only compile errors before testing on Windows. One-time setup: `rustup target add x86_64-pc-windows-gnu` and `sudo apt-get install gcc-mingw-w64-x86-64`.
- `just test-wine` (on Linux, with `wine` installed): the core, store and secrets tests as Windows binaries. It found a real Windows bug (copies keep their modified time, so duplicate-ID detection used the wrong file), but Wine differs from Windows in places (see the recipe's comment), so it doesn't replace testing on Windows.
- `just fmt` formats Rust and UI code; `just bindings` regenerates `ui/src/ipc/bindings.ts` after changing a Tauri command or an IPC type (debug builds also do it on start).
- `just screenshots` regenerates the README pictures in `docs/images/` from the real app.
- `just bench`: `timing_compare` (IronCall timings vs `curl`) and the 100MB body viewer benchmark.
- Performance budgets: `IRONCALL_E2E_PERF=1 IRONCALL_APP=<path to release build> python3 tests/e2e/run.py responses` after `cargo tauri build --no-bundle`.

## Workflow
- Work directly on `main`: small commits, pushed as each step lands. Use a short-lived topic branch only for risky or experimental changes.
- `just check` must pass before every commit (format, clippy `-D warnings`, biome, tests, `cargo deny`).
- Follow the Definition of Done in PLAN.md §8.1, including checking UI, file and network changes on Linux. V1 is Linux-only (ADR 0004), but keep `just check-windows` passing.
- Record significant decisions as an ADR in `docs/adr/` (copy `0000-template.md`, take the next number).

## Code rules
See [CLAUDE.md](../CLAUDE.md) "Rules that are easy to get wrong". The short version: logic lives in the `crates/`, not in `src-tauri/`; no networking in the frontend; no secrets in files, logs or history.
