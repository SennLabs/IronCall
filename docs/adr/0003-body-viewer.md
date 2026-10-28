# 0003 — Large body viewer: paged rows, scaled scrolling, CodeMirror only for small bodies

- **Status:** Accepted
- **Date:** 2026-10-05
- **Phase:** P0 (Spike C), applies to P1 and P5

## Context
PLAN.md §3.1 requires that response bodies never travel as IPC JSON, and Phase 0 requires that a 100MB file scrolls with no main-thread task over 50ms. We needed to know:
- whether a Rust row index plus paged IPC is fast enough
- how to virtualise millions of rows in WebKitGTK and WebView2
- where CodeMirror 6 stops being suitable
- whether a Tauri custom scheme can serve `Range` requests

## Options considered
1. **CodeMirror for everything.** It can't lazy-load. The whole document would have to cross IPC and sit in JS memory. Rejected for large bodies.
2. **`@tanstack/solid-virtual` over paged rows.** It renders one spacer of `rowCount × rowHeight` px. WebKitGTK caps element height at **33,554,428px** (measured), and 100MB of pretty JSON is ~5.07M rows ≈ 91M px. It would silently stop scrolling partway through. Rejected for the body viewer. It's still fine for lists that stay under the cap (e.g. history), so it isn't installed yet.
3. **Custom paged viewer with a scaled scrollbar (chosen).** Rust indexes rows. The UI fetches ~64KB pages through a small LRU cache. The scroll container is capped at 8M px. The viewer keeps the true position itself and converts wheel and keyboard deltas, so scrolling speed is normal. The scrollbar maps proportionally.

## Decision
- **Row index (`ironcall-store::body`):** a streaming build using 8 bytes per row, plus a line-number checkpoint every 1,024 rows. A row is a line, or a ≤1,024-byte slice of a longer line split on a UTF-8 boundary. Continuation rows are flagged, and the gutter shows `↪`. `CRLF` and missing trailing newlines are handled.
- **IPC:** `open_body(path)` checks the path with the PathGuard. `get_body_lines(id, from, count)` returns at most 2,000 rows. `search_body(id, query, fromOffset, ignoreCase)` returns the next hit's row, byte offset and column. `close_body(id)` releases the body. All file work runs on blocking threads.
- **Viewer (`LargeBodyViewer`):**
  - a sticky render window plus a spacer
  - `<Index>` rendering, so DOM nodes are reused
  - `contain: layout style` on rows, with ligatures and kerning off
  - pages sized to ~64KB (20–200 rows)
  - 100 rows of overscan
- **Custom scheme `ironcall`:**
  - `/body/{id}` answers `Range` with `206` (max 16MB per slice)
  - full bodies are served only up to 32MB; larger ones get `416` without a Range header
  - CORS headers are set
  - `schemeUrl()` builds `http://ironcall.localhost/…` on Windows and `ironcall://localhost/…` elsewhere, and the CSP lists both
- **Thresholds:** CodeMirror is used for request bodies and for responses **≤ 2MB**. The paged viewer is used above that.
  - CodeMirror itself coped with 50MB: first paint 94ms, an edit 19ms. The limiting factor is that the whole body has to cross IPC and be held in JS, which is what the plan forbids for large bodies.
  - Revisit the threshold (e.g. 5MB) in P1, after measuring a whole-body IPC transfer.

## Measurements
**Setup:**
- Linux, WebKitGTK 2.54 under Xvfb (software rendering, no GPU), 1600×1000 window with 40 visible rows
- debug Rust build; test bodies are generated JSON (`spike_create_test_body`)

**Method:**
- WebKitGTK **doesn't support the `longtask` PerformanceObserver** (`supportedEntryTypes` lacks it).
- Instead, frame gaps between `requestAnimationFrame` callbacks were recorded while scrolling programmatically.
- A "blank fraction" was also recorded: the share of on-screen rows still waiting for their page.
- Results are logged as `SPIKE_C_RESULTS` by `IRONCALL_SPIKE_AUTORUN=1 cargo tauri dev`.

| Measure | 100MB pretty JSON (5.07M rows) | 100MB minified JSON (102k rows of 1KB) |
|---|---|---|
| Index build (release, unit bench) | 57ms (5.6M rows) | — |
| Index build (debug app) | 740ms | 145ms |
| `get_body_lines` 200 rows over IPC | p50 2ms, p95 2ms | p50 8ms, p95 14ms |
| Wheel scroll, 10 rows/frame | max gap **22ms**, 0 blank | max gap **48ms**, p95 39ms, 0 blank |
| Fling, 40 rows/frame | max gap **19ms**, 0 blank | — |
| Scrollbar drag across the whole file in 2s | max gap 21ms; rows blank while dragging (expected) | — |
| Jump to random row → all rows filled | p50 17ms, p95 19ms | p50 20ms, p95 41ms |
| Full search with no match (debug / release) | 348ms / 19ms | — |
| `Range` fetch through the scheme | 64KB: 1ms, 4MB: 18ms (206, correct `Content-Range`) | — |

- **Exit criterion met:** no frame gap over 50ms in any scenario.
- **Minified JSON is the weak spot.** Laying out ~40 rows of 1KB non-ASCII text in software rendering gives ~28fps.
  - Smaller rows (256B) didn't help (80ms max).
  - `<Index>` plus CSS containment (65ms → 56ms) and byte-sized pages (→ 48ms) did.

## Consequences
- P1 reuses `BodyStore`, the commands and `LargeBodyViewer` as is. The `spike_*` commands and `SpikeView` are dev tooling and should be removed or gated when P1's real response panel lands.
- **Follow-ups:**
  - **Horizontal virtualisation, or soft-wrap, for long-line bodies.** Render only the visible column slice of each row. This is the next lever for minified JSON. Watch out: wide CJK characters break the fixed-width assumption.
  - **Show a "loading" placeholder during scrollbar drags.**
  - **Background body indexing with progress.** Debug builds take 0.7s for 100MB; release builds are about 10× faster.
  - **Measure whole-body IPC transfer** to settle the CodeMirror threshold.
  - **Search:** add cancellation, and case folding beyond ASCII.
- **Not yet verified on Windows (WebView2)** or with GPU rendering. Repeat the autorun benchmark there before the P1 exit.
- **PLAN.md:** §3.3 still lists `@tanstack/solid-virtual` for the large text viewer. That should change to "custom scaled virtualiser (ADR 0003); TanStack for short lists only". The CodeMirror limit of 2MB stays.
