# 0007 — WebKitGTK's DMA-BUF renderer is turned off on Linux

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** P8 (performance budgets before v1.0)

## Context
Idle memory was the last v1.0 budget not met: 234MB PSS over all processes against
200MB (PLAN.md Phase 8), on a Linux desktop with Intel Arc + NVIDIA RTX 500 graphics.
WebKitGTK 2.42+ composites through its DMA-BUF renderer by default. Measured on that
machine, release build, one variable changed at a time:

| Setting | Idle PSS |
|---|---|
| Default | 234MB |
| `MALLOC_ARENA_MAX=2` or `TOKIO_WORKER_THREADS=2` | 232–234MB |
| Mesa only (`__EGL_VENDOR_LIBRARY_FILENAMES`, i.e. no NVIDIA driver) | 212MB |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | 206MB |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | 207MB |
| Mesa only + DMA-BUF renderer off | 186MB |

With the DMA-BUF renderer off, the E2E budgets also improved: the worst frame gap
scrolling a 100MB body went from 36ms to 16–17ms, filtering the 2,000-request tree
from 42ms to 17–18ms, and memory with a 100MB body from 317MB to 279–282MB.

## Options considered
1. **Turn off compositing** (`WEBKIT_DISABLE_COMPOSITING_MODE`, or the
   `hardware-acceleration-policy` setting). About the same memory, but everything is
   painted on the CPU.
2. **Turn off only the DMA-BUF renderer (chosen).** WebKit falls back to its earlier
   accelerated path. Many Tauri apps already set it to avoid blank windows on NVIDIA.
3. **Restate the budget** for hybrid-GPU machines. Leaves the slower scrolling.

## Decision
Option 2. The variable must be set before WebKit starts, and `std::env::set_var` is
`unsafe` in Rust 2024, which the workspace forbids. So on Linux the app re-executes
itself once with `WEBKIT_DISABLE_DMABUF_RENDERER=1` (same process ID, so packaging,
AppImage and the WebDriver are unaffected). A value the user has already set, even
`0`, is kept, which is also how to get the default renderer back.

## Consequences
- Idle memory on the test machine is 206–207MB, 7MB over budget. What remains over is
  environment-specific: the NVIDIA driver libraries (about 22MB) and an 8MB
  `glycin-svg` image-loader process GTK starts on GNOME Wayland. A machine without an
  NVIDIA GPU is expected to come in around 186MB.
- If a future WebKitGTK removes the older path, the variable is ignored and the
  default renderer is used.
- Windows is unaffected (WebView2).
