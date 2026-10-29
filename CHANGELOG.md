# Changelog

All notable changes to IronCall. The app follows [Semantic Versioning](https://semver.org);
workspace file schemas are versioned separately (see the end of each release).

## [1.0.1]: 2026-10-29

### Changed
- New app icon: an anvil with a rebounding arrow, in the same rust-orange and cream as
  before. The SVG sources (light and a dark variant) live in `src-tauri/icons/src/`;
  regenerate the PNG, ICO and ICNS set with `cargo tauri icon` from a 1024 px render.

## [1.0.0]: 2026-10-08

IronCall 1.0 for Linux: 0.9.0's features, plus fixes from a review of the whole app
(backend, security, UI and docs), security hardening and lower memory use. Every
performance budget is met on a Linux desktop (see PLAN.md › Phase 8).

### Added
- Scratch folders. Right-click the Scratch heading, a scratch folder or a scratch request
  for **New request** and **New folder**; drag to move; F2 renames. The layout lives in
  `scratch/folders.json` in the app data folder, not in a workspace, so it can't be
  shared. Deleting a scratch folder deletes the requests in it permanently, after asking
  ([ADR 0005](docs/adr/0005-scratch-folders.md)).
- Right-clicking the empty space below the workspace tree opens the collection's menu
  (**New request**, **New folder**, …).

### Security
- Workspace folders are picked in a dialog run by the app itself, and only folders picked
  there or on the recent list can be opened, so the UI can't open any other folder.
- File commands can no longer read history, cookies or settings in the app data folder;
  only response bodies are reachable.
- Where a workspace's secrets are stored is chosen per machine, not read from
  `.ironcall/local.yaml`, which a repo could commit to pick the plaintext file. After
  upgrading, workspaces that used the encrypted or plaintext file use the keychain until
  you switch back in **Secret storage…**; a banner says so
  ([ADR 0006](docs/adr/0006-webview-untrusted-paths.md)).
- A symlink in a workspace can no longer upload a file from outside it without asking.
- Secrets whose values contain `{{references}}`, and API keys under custom header or
  query names, are masked in history, the timeline and errors.
- Redirects to another site drop every sensitive header, including API keys and your
  extra sensitive headers.
- cURL export "with variables" masks literal credentials; masked export covers multipart
  text parts.
- Markdown descriptions can't contain forms, and the app window can't navigate away from
  the app. The echo server refuses foreign `Host` headers and masks credential-looking
  values.
- A proxy URL containing `user:password@` is refused (it would be stored in plain text).
- Dependencies updated for the RustSec advisories published in October 2026: rustls
  0.23.46 and rustls-webpki 0.103.16 (certificate verification could accept distrusted
  intermediates; TLS secrets in debug output; a key-exchange downgrade; unauthenticated
  certificate decompression), webpki-root-certs 1.0.11, tokio 1.54, and the zstd crates
  used for response decompression.

### Changed
- On Linux, WebKitGTK's DMA-BUF renderer is turned off: about 27MB less memory when
  idle and smoother scrolling. Set `WEBKIT_DISABLE_DMABUF_RENDERER=0` to get it back
  ([ADR 0007](docs/adr/0007-webkit-dmabuf-renderer-off.md)).

### Fixed
- Closing the window asks about unsaved changes instead of discarding them.
- Escape in a stacked prompt no longer also closes the dialog under it.
- The Tests tab's Expected field keeps what you type (`200, 201`, `1.05`).
- Rename from the tree's context menu works, and renames and deletes reach open tabs.
- A cancel pressed before a send starts is honoured; a run no longer undoes changes made
  to runtime variables while it ran.
- Workspaces with symlinked folders, unreadable folders or files changing mid-scan open
  reliably; a request's own timeout of 0 means no timeout.
- Dark-theme syntax colours meet WCAG AA; the Tests editor's columns fit.

### File schemas
- No changes: `ironcall/request@1`, `ironcall/folder@1`, `ironcall/collection@1`,
  `ironcall/environment@1` and `ironcall/globals@1` (`globals.yaml` in the app data
  folder). A `secretBackend` key in `.ironcall/local.yaml` is now
  ignored (see Security).

## [0.9.0]: 2026-10-06

The first release: a local-first REST API client for Linux, feature-complete for 1.0. Windows support comes after
1.0 ([ADR 0004](docs/adr/0004-v1-linux-only.md)).

### Requests and responses
- Any HTTP method; path and query parameters kept in sync with the URL; JSON, text, XML,
  form, multipart and binary-file bodies (streamed from disk, with upload progress).
- Basic, Bearer and API key auth, inherited from folders and the collection; default
  headers from folders and the collection.
- Own HTTP engine (hyper and rustls): HTTP/1.1 and HTTP/2, connection reuse, redirects,
  cookies per workspace, proxies (environment or manual, with credentials in the OS
  keychain), extra CA certificates, a timing breakdown, TLS details and a timeline.
- Responses of any size: a paged viewer that opens hundreds of MB quickly, search,
  pretty-printing, image, PDF and sandboxed HTML previews, "Save response…".
- History of every send, searchable, with retention limits.

### Workspaces
- Plain YAML files in a folder you can commit; stable IDs; canonical, byte-stable
  output; unknown keys kept; invalid files shown, never overwritten.
- Tree with drag and drop, quick open (Ctrl+P), tabs with undo, session restore, scratch
  requests, and live reload of outside edits with conflict handling.

### Variables, environments and secrets
- Global, collection, environment, folder, request and runtime variables with
  provenance, highlighting and dynamic helpers.
- Secrets in the OS keychain, an encrypted file or a plaintext file, never in YAML;
  missing-secrets banner for teammates; secrets and sensitive headers masked in history,
  logs, the timeline and exports.

### Testing
- Declarative assertions (status, headers, JSONPath, body text, duration, size; twelve
  operators) and extraction into runtime variables.
- Collection runner: workspace or folder, environment, iterations, delay, stop on first
  failure, parallel mode, live results, history drill-down, Markdown report.

### Import and export
- cURL import (bash/POSIX and Chrome's cmd format; paste into the URL bar) and export
  (bash and PowerShell, with secrets masked by default).
- OpenAPI 3.0/3.1 and Swagger 2.0 import with a preview.

### App
- Settings: theme, font size, zoom, request defaults, extra sensitive headers.
- First run: a sample workspace that calls a built-in echo server (works offline).
- Accessibility: WCAG AA contrast in both themes (checked by tests), ARIA roles checked
  with axe, focus returns to where dialogs were opened from.
- Diagnostics: panics and UI errors are logged with secrets scrubbed; "Copy diagnostic
  info" and "Open logs folder".
- Private data folders (history, spool, cookies, settings, logs, and `.ironcall/` when it
  holds secrets) are readable only by the current user.

### Known issues (blocking 1.0)
- Performance budgets haven't been confirmed on reference hardware. Under software
  rendering, scrolling a 100MB response has frame gaps up to ~66ms (budget 50ms) and
  total memory with it open is ~540MB (budget 400MB). Start time and idle memory
  haven't been measured on hardware. See PLAN.md › Phase 8.

### File schemas
- `ironcall/request@1`, `ironcall/folder@1`, `ironcall/collection@1`,
  `ironcall/environment@1` and `ironcall/globals@1`: the first versions. See
  [docs/file-format.md](docs/file-format.md).
