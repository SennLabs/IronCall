# IronCall — REST API Client: Project Plan

> Product name: **IronCall**. A local-first desktop REST client in the spirit of Insomnia, Postman and curl.
> Built with **Tauri 2** (Rust backend) and **SolidJS + TypeScript** (frontend).
> Status: **v1.0.0 released 2026-10-08 (Linux)**. Living document; post-V1 work is in §10. Drafted 2026-10-05 after a multi-agent review and the Phase 0 spikes (see Appendix A, ADRs 0001–0003).

---

## 1. Goals & Non-Goals

### Goals
- Fast, lightweight desktop app for building, sending and inspecting HTTP requests.
- **Local-first, git-friendly**: collections are plain YAML files that can be committed, diffed and reviewed.
- All networking happens in Rust: no browser CORS limits, full control over TLS, HTTP/2, cookies, proxies and timings.
- Secrets and sensitive headers never end up in collection files, logs or exports by default.
- A clean core (Rust crates without Tauri dependencies) that a CLI runner can reuse later.

### Licence
Not decided yet. It will be chosen before IronCall is shared beyond the core team. Until then, keep dependency licences compatible with permissive or proprietary use (enforced by `cargo deny` licence checks), so either option stays open.

### Non-Goals (for V1.0)
- Accounts, cloud sync or team workspaces. Sharing happens through git and files.
- Protocols other than HTTP/HTTPS (GraphQL, WebSocket, gRPC and SSE come after V1).
- Imperative scripting (JavaScript comes after V1; V1 uses declarative assertions and extraction).
- Windows and macOS builds ([ADR 0004](adr/0004-v1-linux-only.md)), auto-update, code signing and release CI.
- Internationalisation: **English only** in V1. Strings are still kept in a single module so translation is possible later.
- Telemetry: **none**. IronCall makes no network calls of its own.

---

## 2. Decisions Summary

| Area | Decision |
|---|---|
| Shell | Tauri **2.x** (pinned; Tauri 3 is in alpha) |
| Backend | Rust (stable), Tokio async runtime |
| Frontend | SolidJS + TypeScript + Vite |
| Platforms (V1) | Linux (WebKitGTK). Windows (WebView2) comes after V1 ([ADR 0004](adr/0004-v1-linux-only.md)); the code stays cross-platform |
| Audience | Personal / small team, local-first, no accounts, no telemetry |
| Storage | Plain files, one **YAML** file per request, folder tree on disk. Every file has a stable `id`, and **file names stay fixed** when a request is renamed |
| History | Local SQLite log in the app data dir (not in the workspace), capped, searchable, with sensitive data redacted |
| Secrets | Pluggable `SecretStore`. **User chooses** per workspace, and per machine ([ADR 0006](adr/0006-webview-untrusted-paths.md)): OS keychain (default), encrypted file, or plaintext local file (with a warning). Values can be migrated between backends |
| Protocols (V1) | HTTP/1.1 + HTTP/2 over TLS (rustls) |
| Auth (V1) | None, Inherit, Basic, Bearer, API Key (header or query) |
| Proxy (V1) | Minimal: `HTTP(S)_PROXY` / `NO_PROXY` env vars, a manual proxy URL setting, a no-proxy list. PAC, SOCKS and OS auto-detect come after V1 |
| Variables | Global < Collection < Environment < Folder < Request < Runtime (most specific wins) |
| Scripting | V1: declarative assertions + JSONPath/header/regex extraction. Post-V1: sandboxed JS |
| Import/Export (V1) | cURL import (bash + Chrome "Copy as cURL (cmd)") and export (bash + PowerShell). OpenAPI 3.0 / 3.1 and Swagger 2.0 import |
| Collection runner | In V1 (GUI with results table). JUnit/JSON reports come after V1 with the CLI |
| CLI runner | Post-V1 (core crates designed for it from day one) |
| UI (V1) | Multiple request tabs, dark/light theme (follows OS, manual override), scratch requests |
| Responses | Large bodies (10MB+) without freezing, image/PDF/HTML/hex preview, timing breakdown |
| Network (V1) | Custom CA / self-signed certs, TLS verify toggle, HTTP/2, persistent cookie jar, minimal proxy |
| Distribution | Manual builds only (documented build steps per platform) |
| Testing | Rust unit + integration (local axum/rcgen TLS server), Vitest (UI), WebDriver E2E smoke tests |

---

## 3. Architecture

### 3.1 High-level

```
┌──────────────────────────── Tauri Window (WebView) ────────────────────────────┐
│ SolidJS UI                                                                     │
│  Sidebar (tree) │ Tabs │ Request editor │ Response viewer │ Env switcher       │
│  Solid stores  ◄── typed IPC client (generated TS bindings) ──►                │
└───────────────────────────────┬────────────────────────────────────────────────┘
          invoke() commands  │  events (progress, send lifecycle)
          custom URI scheme  │  (Range-sliced response bodies & previews)
┌───────────────────────────────┴──── src-tauri (app shell) ─────────────────────┐
│ Command handlers · app state · path guard · window/menu · file watcher         │
└───────┬───────────────┬────────────────┬───────────────┬──────────────┬────────┘
   ironcall-core      ironcall-http       ironcall-store      ironcall-secrets   ironcall-import
   model, vars,    engine, pool,    YAML workspace,  keyring /       curl, OpenAPI
   templating,     timings, proxy,  history (SQLite),encrypted file /
   redaction,      cookies, TLS,    body spool,      plaintext
   assertions,     body streaming   file watching
   runner
```

**Principles**
- **The frontend never does networking.** Everything goes through Rust commands.
- **Core crates have no Tauri dependency**, so they can be reused by a future `ironcall-cli`.
- **Typed IPC**: generate TypeScript types from Rust structs, so the two sides can't drift apart. Choice: `tauri-specta` **2.0.0-rc.x pinned to exact versions** (no stable 2.x release yet; it also generates command wrappers). Fallback: `ts-rs` (stable, types only).
- **Response bodies never travel as IPC JSON** (from Phase 1). Each body is always spooled to a file under `<app data>/spool` (Phase 1 dropped the in-memory tier). The UI reads it through:
  - a **paged text API** (`get_body_lines(id, from, count)`, backed by a line index built in Rust), and
  - a **custom URI scheme** that answers `Range` requests with `206` slices, used for image/PDF previews.
  Tauri custom protocols return a whole body per request and can't stream. Range slices are the workaround (Tauri's own asset protocol does the same).
- **Platform URL difference**: custom schemes are `http://ironcall.localhost/…` on Windows and `ironcall://localhost/…` on Linux. A single frontend helper builds these URLs, and the CSP lists both.
- **Cancellation**: every send has an ID and a `CancellationToken`. See §3.4 for semantics.
- **Path guard**: Tauri capability scopes only restrict JS-side plugins. Rust commands must check every file path themselves against the open workspace, the app data dir, and files the user picked explicitly.
- **Observability from day one**: `tracing` with a **redaction layer** (§6.3) writing to rolling log files, plus an in-memory "recent sends" log available from Phase 1.

### 3.2 Cargo workspace layout

```
/ (repo root)
├── Cargo.toml                 # [workspace]
├── crates/
│   ├── ironcall-core/            # domain model, variable resolution, templating, redaction, assertions, runner
│   ├── ironcall-http/            # HTTP engine (hyper + rustls), connection pool, timings, proxy, cookies, TLS
│   ├── ironcall-store/           # workspace YAML, file watching, history DB, body store/spool
│   ├── ironcall-secrets/         # SecretStore trait + keyring / encrypted-file / plaintext backends
│   └── ironcall-import/          # curl parser/generator, OpenAPI/Swagger importer
├── src-tauri/                 # Tauri app: commands, state, menus, protocol handlers, path guard
├── ui/                        # SolidJS + TS + Vite frontend
│   ├── src/components/ ...
│   ├── src/stores/ ...
│   └── src/ipc/ (generated bindings)
├── tests/e2e/                 # WebDriver (tauri-driver) smoke tests
├── tests/fixtures/            # YAML round-trip, curl corpus, OpenAPI corpus
└── docs/
    ├── PLAN.md
    ├── adr/                   # Architecture Decision Records (template in P0)
    └── CONTRIBUTING.md
```

### 3.3 Key technology choices

| Concern | Choice | Notes |
|---|---|---|
| HTTP engine | **Decided: `hyper` 1.x** (ADR 0001) low-level `client::conn::{http1,http2}` + `tokio-rustls` + **own connection pool** | Needed for the timing breakdown, which `reqwest` doesn't expose. hyper-util's legacy `Client` can't report whether a connection was reused, so IronCall manages its own pool (§3.4). **Fallback:** the `curl` crate (libcurl has native timings), but on Windows that means Schannel or vendored OpenSSL, nghttp2 for HTTP/2, and a second TLS stack with different CA/verify behaviour. Decided in Spike A (time-boxed). |
| DNS timing | `tokio::net::lookup_host` (`getaddrinfo` on a blocking thread; ADR 0001) | Measured separately from connect time. |
| TLS trust | `rustls` + `rustls-platform-verifier` (`Verifier::new_with_extra_roots`) | Uses the OS trust store plus user-added CAs. The extra roots are fixed when the verifier is built, so there is one cached `ClientConfig` per CA set. "Disable verification" uses rustls `dangerous()` with a custom `ServerCertVerifier` that still checks handshake signatures. |
| HTTP/2 | hyper `http2` + ALPN | Negotiated automatically, with a per-request option to force HTTP/1.1. |
| Proxy | Own CONNECT tunnel (HTTPS) and absolute-URI forwarding (HTTP) in the connector | Reads env vars and the manual setting, applies `NO_PROXY` matching, supports Basic proxy auth. |
| Cookies | `cookie_store` 0.22 | Persisted per workspace under the app data dir, not inside the workspace folder. A single writer task serialises updates from parallel sends. |
| Decompression | `async-compression` 0.4 | gzip, br, zstd, and deflate (both zlib-wrapped and raw). |
| YAML | **`serde-saphyr`** (decided, ADR 0002). `serde_norway` rejected (can't force quoting: writes `no`, `on` and dates unquoted). **`serde_yml` and `serde_yaml` are banned** in `deny.toml` | `quote_all: true`, `prefer_block_scalars: false`, with `|` blocks only for multiline text fields. |
| JSONPath | `serde_json_path` | Aims for RFC 9535 compliance. Test the edge cases we rely on. |
| History DB | `rusqlite` (`bundled`, which includes FTS5) | Stored in the app data dir. |
| Secrets | `keyring-core` 1.x + per-platform store crates (`windows-native-keyring-store`, `zbus-secret-service-keyring-store`). `argon2` + `chacha20poly1305` for the encrypted file. `secrecy` for in-memory secret types | See §6. |
| File watching | `notify` 8.x + **`notify-debouncer-full`** | `notify` does not debounce on its own. The watcher ignores events caused by IronCall's own atomic writes (tracked by path + content hash). |
| OpenAPI | Detect the version, then map Swagger 2.0, OpenAPI 3.0 and 3.1 by hand from `serde_json::Value` (Phase 6) | `openapiv3` and `oas3` were considered; neither covers both 3.0 and 3.1, so neither is used. |
| Shell parsing (curl) | `shell-words` (POSIX) + a dedicated parser for Chrome's cmd format (`^` escapes) | PowerShell **import** comes after V1. |
| Code editor (UI) | CodeMirror 6 for request bodies and responses **up to 2MB** (ADR 0003) | The limit is IPC transfer of the whole body, not CodeMirror itself (a 50MB document opened in 94ms). Larger bodies use the paged viewer. |
| Large text viewer | **Custom viewer with a scaled scrollbar** over the paged body API (ADR 0003) | `@tanstack/solid-virtual` can't be used for bodies: WebKitGTK caps element height at about 33.5M px, and 5M rows need about 91M px. Rows are split at 1KB; pages are about 64KB. Rust does indexing, search and pretty-printing. The sidebar tree uses its own small virtualiser too (Phase 2); TanStack is not a dependency. |
| PDF preview | `pdf.js` (bundled) | WebKitGTK has no built-in PDF viewer. |
| UI primitives | Kobalte (pre-1.0, pinned) | Headless and accessible. Wrap it in in-house components so it can be swapped out. |
| Styling | CSS variables (design tokens) + CSS modules | Dark/light theme switches by swapping tokens. |

### 3.4 Connections, concurrency & cancellation
- **Pool key**: scheme, host, port, proxy, TLS config identity (CA set + verify flag + ALPN) and HTTP version. A connection opened with verification **off** is never reused for a request with it **on**.
- **Timings on reused connections**: DNS, connect and TLS are reported as `reused` rather than 0. Each request has a "**fresh connection**" option, and a global setting, to always measure cold.
- **Concurrency**: many tabs and runner requests can be in flight at once. There's a global in-flight cap (default 32) and a per-host cap for HTTP/1.1 (default 6).
- **Cancellation semantics**: the partial response is kept and marked `cancelled` in the UI and history. `extract` and assertions don't run. Pooled connections in a mid-transfer state are dropped.
- **Uploads**: file and multipart bodies are streamed from disk with progress events, never loaded fully into memory.

---

## 4. On-Disk Workspace Format

A **workspace** (one collection) is a folder that can be opened from anywhere, including inside a git repo. V1 opens **one workspace per window**, with a quick switcher for recent workspaces. Multiple windows come after V1.

```
my-api/
├── ironcall.yaml                     # collection metadata, workspace id, vars, default headers/auth, description
├── environments/
│   ├── dev.yaml
│   └── prod.yaml
├── users/
│   ├── _folder.yaml               # folder vars/auth/headers, description, child ordering
│   ├── list-users.yaml
│   └── create-user.yaml
├── .gitattributes                 # written on workspace creation: `*.yaml text eol=lf`
└── .ironcall/                     # LOCAL ONLY — contains its own `.gitignore` with `*`
    ├── local.yaml                 # active environment, UI state
    └── secrets.json | secrets.enc # only for the plaintext / encrypted-file backends
```

Cookies, history, the body spool, scratch requests, global variables and app settings live in the **app data dir** (`$XDG_DATA_HOME/io.ironcall.desktop` on Linux, `%APPDATA%\io.ironcall.desktop` on Windows), not in the workspace.

### 4.1 Example request file (`users/create-user.yaml`)

This is the **canonical form** IronCall writes (ADR 0002): every string in single quotes, table rows as one-line flow maps, multiline text as `|` blocks, and default values (such as `auth: inherit`) left out. Hand-written files in other styles are accepted and normalised on the first save.

```yaml
schema: 'ironcall/request@1'
id: '01J9Z3K7T2V8Q4M6N0P5R1S3W7'
name: 'Create user'
description: |
  Creates a user. Markdown supported.
method: 'POST'
url: '{{baseUrl}}/users/:orgId/members'
pathParams:
- {name: 'orgId', value: '{{orgId}}'}
params:
- {name: 'notify', value: 'true', enabled: true}
headers:
- {name: 'Content-Type', value: 'application/json', enabled: true}
body:
  type: 'json'
  content: |
    {
      "name": "{{userName}}",
      "email": "{{$randomEmail}}"
    }
vars:
  userName: 'Ada'
extract:
- {var: 'userId', from: 'body', path: '$.id', sensitive: false}
assert:
- {target: 'status', op: 'eq', value: 201}
- {target: 'body', path: '$.email', op: 'exists'}
- {target: 'duration', op: 'lt', value: 500}
settings:
  timeoutMs: 30000
  followRedirects: true
  maxRedirects: 10
  verifyTls: true
  httpVersion: 'auto'
  freshConnection: false
  encodeUrl: true
```

Methods can be any token (PROPFIND, PURGE, …). The full set of fields and variants is defined in `ironcall-core::model`, with one fixture per variant in `tests/fixtures/yaml/canonical/`.

### 4.2 Example environment (`environments/dev.yaml`)

```yaml
schema: 'ironcall/environment@1'
id: '01J9Z3M0A1B2C3D4E5F6G7H8J9'
name: 'Dev'
vars:
  baseUrl: 'https://dev.api.example.com'
  apiKey:
    secret: true          # value lives in the selected SecretStore, never in this file
  region:
    value: 'ap-southeast-2'
    description: 'AWS region for the dev stack'
```

**Schema values**: `ironcall/collection@1` (`ironcall.yaml`), `ironcall/folder@1` (`_folder.yaml`), `ironcall/request@1`, `ironcall/environment@1`.

### 4.3 Identity, naming & ordering
- **Stable IDs**: every request, folder and environment has a ULID `id`. History, tab restore and runner results refer to IDs, not paths. If two files share an ID (e.g. after a copy-paste on disk), the newer file gets a fresh ID and the user is told.
- **File names are fixed at creation** from a slug of the name. **Renaming a request only changes `name:`**, which keeps `git blame` intact and avoids rename conflicts between teammates. A separate "Rename file to match name" action exists.
- **Slug rules** (safe on Windows and Linux):
  - lowercase ASCII `[a-z0-9-]`, with Unicode transliterated or dropped and names NFC-normalised first
  - at most 60 characters, no trailing dots or spaces
  - reserved names blocked (`con`, `prn`, `aux`, `nul`, `com1-9`, `lpt1-9`)
  - case-insensitive collisions resolved with `-2`, `-3`, …
  - the full path is checked against the 260-character `MAX_PATH` limit, with a warning near it
- **Paths** inside files always use `/` and are relative to the workspace root.
- **Ordering**: `order: [<id>, …]` in `_folder.yaml` / `ironcall.yaml`. Files missing from `order` (e.g. added by a git merge) appear at the end, alphabetically. IDs in `order` with no matching file are skipped silently and cleaned up on the next save of that folder.

### 4.4 File format rules
- **Schema versioning**: every file has a `schema` field. Migrations run **in memory**. A file is only rewritten when the user saves it, so untouched files never produce diffs. Files with a **newer** schema than the app understands open read-only.
- **Unknown keys**: each struct, including nested list items such as headers and assertions, has a `#[serde(flatten)] extra: IndexMap<String, Value>` field, so unknown keys survive a round-trip.
- **Known limitations** (ADR 0002; documented in the file-format reference):
  - YAML comments are lost on save, and hand formatting is normalised on the first save.
  - Unknown keys are written after the known ones. Unquoted values *inside unknown keys* keep YAML 1.1 typing (`no` becomes `false`). Known string fields keep their source text.
  - Strings containing a backslash (e.g. regexes) are written double-quoted with escapes.
  - Unquoted float variables are re-formatted (`1.50` becomes `'1.5'`).
- **Stable output** (`serde-saphyr` with `quote_all`): fixed key order, every string quoted, multiline strings written as `|` blocks, table rows as flow maps, LF line endings (CRLF input is normalised), trailing newline. Saving an unchanged request produces a **byte-identical** file (enforced by tests).
- **Atomic writes**: write a temp file in the same directory, fsync, then rename.
- **Invalid files** (bad YAML, leftover merge-conflict markers, schema errors): shown as an **error node** in the tree with the parse error and line number. They are never overwritten or deleted, and "Open in external editor" and "Reveal in file manager" are offered.
- **Deleting** a request or folder moves it to the OS trash/recycle bin (the `trash` crate).

### 4.5 Workspace identity & secrets for teammates
- The workspace `id` is committed in `ironcall.yaml`.
- Secret values are keyed by `(workspace id, environment id, variable name)` **on the local machine only**.
- When a teammate clones a workspace, the secret variables are declared in the YAML but have no local value. IronCall shows a "**N secrets missing for environment Dev — enter now**" banner, and requests that use a missing secret show a warning before sending.

### 4.6 Scratch requests
- Tabs can hold **scratch requests** that don't belong to any workspace: a new blank tab, a pasted cURL command, or a request reopened from history.
- They are stored in the app data dir and survive restarts. "Save to workspace…" moves one into a folder.
- They can be grouped into (nested) **scratch folders**, created from the right-click menu on the Scratch section, a folder or a request; requests and folders are dragged into folders. Folders are a display layout in `scratch/folders.json`: request files stay at `scratch/<id>.yaml`, and a missing or unreadable layout shows a flat list ([ADR 0005](adr/0005-scratch-folders.md)). Deleting a folder deletes its scratch requests permanently (after asking); they are not in a workspace, so there is no trash.
- Right-clicking empty space below the workspace tree opens the collection menu (new request or folder at the top level).

---

## 5. Variables, Templating & URL Encoding

- **Syntax**: `{{name}}`. Dynamic helpers in V1 are **zero-argument only**: `{{$uuid}}`, `{{$ulid}}`, `{{$timestamp}}`, `{{$isoTimestamp}}`, `{{$randomInt}}`, `{{$randomEmail}}`. Helpers that take arguments come after V1, with JS.
- **Precedence**, lowest to highest (the highest one that defines a variable wins):
  1. Global (app data dir)
  2. Collection (`ironcall.yaml`, defaults)
  3. Environment (active environment)
  4. Folder (`_folder.yaml`; nested folders chain, innermost wins)
  5. Request (`vars:`)
  6. Runtime (values set by `extract` during a session or runner run). The scope exists from Phase 3 but stays **empty until Phase 7** adds extraction. Values can optionally be saved to the active environment.
- **Provenance**: resolution in `ironcall-core` records where each value came from. The UI uses this for hover tooltips ("from: Environment › Dev"), red highlighting of unresolved variables, and the environment quick-look panel.
- Variables can reference other variables (max depth 10), with cycle detection.
- Secret and sensitive values are masked in the UI unless the user reveals them, and are always redacted in history and logs (§6.3).

### URL encoding rules
- **Path params** (`:name` in the path, and `{name}` from OpenAPI, normalised to `:name`) are edited in their own table and percent-encoded as path segments after variables are resolved.
- **Query params**: names and values are encoded after variable resolution, using the `application/x-www-form-urlencoded`-compatible set for query strings. Sequences that are already valid `%XX` are **not** encoded twice.
- **Raw mode** (`settings.encodeUrl: false`) sends the URL exactly as typed after variable resolution, for APIs with unusual encoding needs.
- The URL bar and the params table stay in sync in both directions. Editing either one reparses the other without changing the encoding.
- The "resolved request" preview shows the **exact** bytes that will be sent.

---

## 6. Secrets & Sensitive Data

### 6.1 Secret backends
Defined by a `SecretStore` trait in `ironcall-secrets`, with three backends. **The user picks one per workspace, per machine** in settings; the choice is stored in `<app data>/secret-backends.json` and never read from the workspace's `.ironcall/local.yaml` ([ADR 0006](adr/0006-webview-untrusted-paths.md)). Values can be migrated between backends.

| Backend | How | Pros | Cons |
|---|---|---|---|
| **OS keychain** (default) | `keyring` v4: Windows Credential Manager, Linux Secret Service (GNOME Keyring / KWallet) | No passphrase, OS-protected | Linux needs a running Secret Service (often missing in minimal WMs, WSL, some sandboxes). **Windows: 2,560-byte blob limit per entry** (about 1,280 characters if stored as UTF-16), so values are stored as UTF-8 bytes and **split across entries** when larger |
| **Encrypted file** | `.ironcall/secrets.enc`, Argon2id KDF + XChaCha20-Poly1305, unlocked once per session | Works everywhere, no size limit | Passphrase prompt; a forgotten passphrase means lost secrets |
| **Plaintext local file** | `.ironcall/secrets.json` (git-ignored via `.ironcall/.gitignore`, mode 0600) | Simplest | Only as safe as the disk. The UI shows a clear warning |

- If the keychain is unavailable at startup, IronCall suggests switching to the encrypted file.
- In memory, secrets use `secrecy::SecretString`, whose `Debug` output is redacted.
- **Global secrets use the OS keychain only**, because they belong to no workspace and so have no `.ironcall/` folder to hold a file. If the keychain is unavailable, global secrets can't be stored and the UI says so. Workspace-level secrets work with any backend.

### 6.2 What counts as sensitive
- Variables marked `secret: true`.
- Values from `extract` with `sensitive: true`. This defaults to `true` when extracting from an auth-like source: token/password fields, `Set-Cookie`, `Authorization`.
- A **header denylist** that is always treated as sensitive: `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, plus user-configurable additions.
- Basic auth passwords, Bearer tokens, API key values, and proxy credentials.

### 6.3 Redaction — applied centrally in `ironcall-core`
- **History**: sensitive header values and secrets are stored as `••••`. A per-workspace setting "**Don't store response bodies in history**" is available, and history can be cleared per workspace or entirely.
- **Logs and crash reports**: the `tracing` redaction layer covers them, `secrecy` types keep secrets out of `Debug` output, and the panic hook never prints request or response structs.
- **Copy as cURL**: by default secrets are left as `{{var}}` references or masked. Copying resolved secrets needs an explicit "include secrets" option.
- **Body spool files**: written to the app data dir (not the system temp dir) with user-only permissions (0600 on Linux, user-only ACL on Windows). They are deleted when the tab or history entry is evicted, and swept on startup and exit.

---

## 7. Phased Delivery

Each phase ends with a **scripted demo** and passes its exit criteria on **Linux**. Windows checks moved after V1 ([ADR 0004](adr/0004-v1-linux-only.md)); they are collected in §10 › Windows support. Phases 2 and later are usable for real work.

**Team assumption:** 1–2 developers. Effort figures are rough **single-developer** estimates and get re-planned after Phase 0.

**Dependencies:** P0 → P1 → P2 → P3 → P4 → P5 → P7. P6 needs only P2 and P3. With 2 developers, a second developer takes P6 while the first does P4–P5. With 1 developer, P6 runs after P5.

### Phase 0 — Foundations & Spikes (≈2–3 weeks) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:** scaffold, tooling and logging in place; `just check` green. Spike decisions: [ADR 0001](adr/0001-http-engine.md) keep hyper; [ADR 0002](adr/0002-yaml-library.md) serde-saphyr; [ADR 0003](adr/0003-body-viewer.md) custom paged viewer (worst frame gap 48ms on 100MB, under the 50ms target). **Still to do on Windows 11 (post-V1, §10):** `just dev`, `timing_compare`, and the viewer benchmark.

**Goal:** Project skeleton and answers to the riskiest technical questions.
- Cargo workspace + crates skeleton, `src-tauri`, `ui/` (Vite + Solid + TS). Pin Tauri 2.x, tauri-specta rc, and Kobalte.
- Tooling: `rustfmt`, `clippy` (deny warnings), `biome` (lint+format), `vitest`, `cargo deny`, and a `justfile` with `dev`, `test`, `lint`, `check`, `build`.
- `tracing` + redaction layer + rolling log files. ADR template, CONTRIBUTING.md, Definition of Done (§8.1).
- Typed IPC pipeline with one round-trip command. Path guard skeleton.
- **Spike A — HTTP engine (time-box: 5 days):** hyper low-level connections + own pool + instrumented resolver/connector, giving DNS/connect/TLS/TTFB/download timings over HTTP/1.1 and HTTP/2, with reused connections shown as `reused`. **Decision criteria:** if, at the end of the time-box, the hyper version can't produce timings within ±10% of `curl -w` on both OSes, or the projected effort for redirects + proxy + decompression goes over 3 weeks, switch to libcurl. Record the decision as an ADR.
- **Spike B — YAML (time-box: 3 days):** `serde-saphyr` vs `serde_norway`. Round-trip fixtures for the **full V1 schema** (including fields whose engines come later), with forced quoting, `|` bodies, `flatten` extras, and byte-identical re-save.
- **Spike C — Body viewing (time-box: 3 days):** a Rust line index + paged API + virtualised Solid viewer for a 100MB file. CodeMirror 6 in Solid with `{{var}}` decorations.
- Design tokens + theme switcher (dark/light, follows OS).
- **Exit criteria:**
  - `just dev` starts the app on Ubuntu 24.04 (Windows 11: post-V1).
  - `just check` (fmt, clippy, biome, tests, cargo deny) passes.
  - The three spike ADRs are merged.
  - A 100MB file scrolls in the spike viewer with no main-thread task over 50ms (WebView devtools performance trace).

### Phase 1 — Core Request/Response (≈3–4 weeks) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:** a single scratch request can be built, sent, cancelled and inspected end to end. `just check` is green (151 Rust tests, 74 UI tests), and `just e2e` drives the real app over WebDriver (send with path and query params, a connection error, the 20MB body, dark theme). Measured on Linux: 20MB response downloaded in about 65ms, first rows visible about 0.2s after clicking Send.

**Deviations from the plan:**
- Response bodies are always spooled to disk (user-only permissions), not held in memory under 1MB. This is simpler, and fast enough.
- A failed request on a stale pooled connection is retried for every method only when it was never written. If the connection closed mid-exchange, only idempotent methods are retried, so a POST is never sent twice.
- The UI keeps a hand-written `RequestDraft` type and casts it to and from the generated `RequestFile`, which preserves unknown keys at runtime.

**Carried forward:**
- Run `just dev`, `just e2e`, `timing_compare` and the viewer benchmark on Windows 11. *(post-V1: §10 › Windows support)*
- Recent sends doesn't refresh on its own.
- Copying is disabled for bodies over 2MB.
- "Previous" in large-body search only walks back through hits already found.
- Code-split CodeMirror (the JS bundle is about 600KB).
- Hoist `encoding_rs`/`quick-xml` into the workspace dependencies.

**Goal:** Send any HTTP request and inspect the response reliably. Scratch tabs only, nothing saved to workspaces yet.
- Request editor:
  - method (common verbs + free-text custom methods)
  - URL, with a **path params** table and a **query params** table kept in sync both ways, following the encoding rules in §5
  - headers table (enable/disable rows) with **header-name and common-value autocomplete**
  - raw body (JSON / text / XML) with syntax highlighting, a **Beautify** button, and non-blocking lint warnings for invalid JSON
- **Auto headers**: Content-Type is set from the body type. Headers the engine adds (Host, Content-Length, User-Agent, Accept-Encoding) are shown as read-only "auto" rows that can be overridden.
- Send / Cancel (§3.4 semantics), timeout, follow redirects toggle.
- Body store + spool + paged body API + Range-capable custom protocol (from §3.1), so the body-handling path never has to be rebuilt.
- Response viewer:
  - status, total time, size, headers table
  - body with pretty-print (JSON/XML/HTML) under 2MB, raw text above 2MB in the paged viewer
  - **charset decoding** (from Content-Type / BOM, falling back to UTF-8 with replacement characters)
  - fallback to raw when the body doesn't match its Content-Type
  - clean empty states for 204 and HEAD
  - **Ctrl+F search** (CodeMirror search under 2MB; Rust-side search with hit navigation in the paged viewer)
  - copy body, copy a header value
- **Timings captured** from day one: a total time plus a phase tooltip. The full waterfall comes in P5.
- In-memory "recent sends" log (last 50, redacted) to help with debugging.
- **Spike follow-ups:**
  - remove or `debug`-gate the `spike_*` commands and `SpikeView`
  - HTTP engine: Happy Eyeballs (parallel IPv4/IPv6 connect), retry once on a stale pooled HTTP/1.1 connection, the in-flight and per-host caps and idle expiry from §3.4, stream bodies to the body store instead of memory
  - re-run `timing_compare` and the viewer benchmark on Windows 11 *(post-V1: §10 › Windows support)*
- Error display: DNS failure, connection refused, TLS error (with cert subject and reason), timeout, cancelled. Each has a specific message.
- Integration tests against a local `axum` server (+ `rcgen` self-signed TLS). `wiremock` is only for plain HTTP/1.1 cases (it has no TLS or HTTP/2).
- **Exit criteria:**
  - Automated integration tests cover all standard methods, path/query encoding cases (≥30 table-driven cases), redirects, timeout, cancel, gzip/br/zstd/deflate, and invalid JSON responses.
  - A 20MB response renders its first page in under 1s with no main-thread task over 50ms.
  - The demo script runs in both themes.

### Phase 2 — Workspaces, Collections & Tabs (≈3 weeks) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:** workspaces are YAML folders with a sidebar tree, tabs, scratch requests, conflict handling and session restore. `just check` is green (188 Rust tests, 136 UI tests). `just e2e` now also runs `tests/e2e/workspace.py` against the real app, checking: restore from session, the error node for a merge-conflict file (left byte-identical), Ctrl+S writing the canonical form with the ID kept, silent reload of a clean tab, the conflict bar plus Keep mine for a dirty tab, and tab restore after restart. Measured: opening a workspace of 2,000 requests takes 23ms (release; 173ms in debug), a rescan 7ms, and the UI filter is under 100ms at 2,000 nodes.

**Decisions made while building:**
- Every operation rescans the tree, with parsed files cached by mtime and size.
- A hand-made folder (no `_folder.yaml`) gets one written only when it is renamed, reordered, moved or given a description.
- A new item with no index is placed alphabetically unless the parent already has an explicit `order`, so the parent file isn't touched.
- Duplicating places the copy right after the original, which writes the parent's `order`.
- "Save to workspace" gives the request a new ID, and the UI re-keys the tab.
- IronCall's own writes don't emit `WorkspaceChanged`; the UI refreshes the tree after each of its own operations.
- E2E tests run against a temporary data folder (`IRONCALL_DATA_DIR`), so they never touch the user's data.

**Carried forward:**
- Run everything on Windows 11, including trash, file locking and CRLF behaviour. *(post-V1: §10 › Windows support)*
- Folder duplicate is only covered by store tests.
- In plain inputs, the per-tab snapshot undo replaces the browser's own undo.
- The tree virtualiser is hand-rolled (fixed 24px rows).

**Goal:** Save and organise requests as git-friendly files.
- Create / open workspace (folder picker), recent workspaces list and quick switcher. One workspace per window.
- YAML read/write per §4: stable IDs, fixed file names, slug rules, ordering rules, in-memory migrations, atomic writes, `.ironcall/.gitignore` and `.gitattributes` written automatically.
- Sidebar tree:
  - folders and requests
  - create, rename (only changes `name:`), duplicate (new ID), delete (to OS trash), drag-and-drop reorder/move
  - **filter box** (fuzzy match on name/URL)
  - **error nodes** for invalid files
- Markdown `description` editor for collections, folders and requests.
- **Tabs**:
  - open multiple requests, plus scratch tabs (§4.6)
  - dirty indicator, Ctrl+S save, "Save to workspace…" for scratch tabs
  - close / close others, warn on closing unsaved
  - **restore tabs by ID** on restart
- Undo/redo inside editors and tables. Tree operations are undone through the OS trash.
- File watcher (debounced, ignores IronCall's own writes): external changes reload clean tabs, and a dirty tab with a conflicting change shows a "keep mine / take theirs / compare" prompt.
- **Exit criteria:**
  - Scripted test: edit in IronCall → change the same file on disk (simulated `git pull`) → reload. Zero lost edits, and the conflict prompt appears for dirty tabs.
  - Re-saving an unchanged request is byte-identical (property test over the fixture corpus).
  - Slug tests pass: reserved names, case collisions, Unicode, long paths.
  - A file with merge-conflict markers shows an error node and is left untouched.
  - The tree filter returns results in under 100ms with 2,000 requests.

### Phase 3 — Variables, Environments & Secrets (≈2–3 weeks) — ✅ done 2026-10-05 (Linux; Windows keychain moved post-V1)
**Outcome:** layered variables with provenance, environments, globals, the three secret backends with migration, editor support for `{{var}}`, resolved-request preview, and the missing-secrets banner. `just check` is green (243 Rust tests, 178 UI tests).

`just e2e` now also runs `tests/e2e/variables.py` against the real app, from a fresh clone:
- the banner shows the missing secret
- the value is entered through the UI and sent in a header
- switching environment changes both the URL and the secret
- the active environment is persisted in `.ironcall/local.yaml`
- a byte scan finds no secret value in any workspace file, log or app-data file (including WebKit storage)

A glue test checks the same with the encrypted backend, including the locked state. The 4KB Windows-keychain round trip is covered by chunking tests against a backend that enforces the 2,560-byte limit. The real-keychain test (`cargo test -p ironcall-secrets -- --ignored`) needs a desktop with a keychain (the container has no D-Bus session); it passed on one on 2026-10-07, below.

**Decisions made while building:**
- A nested reference resolves with full precedence.
- A secret with no stored value is left unresolved, and Send asks "send anyway?".
- Secrets shorter than 4 characters aren't masked by value, because masking them would hide too much ordinary text.
- Keychain stores use `keyring-core` 1.0, with the pure-Rust zbus Secret Service store on Linux and the native store on Windows.
- Saving any file with a `value` on a secret variable is a hard error.
- Errors carry a typed `conflict` flag.

**Carried forward:**
- ~~Run the ignored real-keychain test on a Linux desktop~~ ✅ passed 2026-10-07 on Ubuntu 26.04 (GNOME Keyring, 4KB secret). Windows: post-V1, §10 › Windows support.
- The "send anyway?" prompt is a native dialog, which WebDriver can't drive, so it is covered only by UI unit tests.
- Highlighting `{{var}}` in plain inputs uses an overlay that hasn't been checked for alignment on Windows/WebView2. *(post-V1: §10 › Windows support)*

**Goal:** Reusable, parameterised requests.
- Variable resolution engine: the §5 precedence order, zero-argument dynamic helpers, cycle detection, provenance tracking. The runtime scope is created but stays empty until P7.
- Environment manager (create/edit/delete/duplicate) and an active-environment switcher. **Environment quick-look** panel (an "eye" popover listing resolved variables and their sources).
- Collection, folder and global variable editors.
- Editor support: `{{` autocomplete, hover tooltip showing value and source, red highlight for unresolved variables.
- "Preview resolved request" view showing the exact bytes that will be sent.
- `SecretStore` with all three backends, a backend picker, **migration between backends**, Windows chunking for large values, a fallback prompt when the keychain is unavailable, and the **missing-secrets banner** for cloned workspaces (§4.5).
- Sensitive-header denylist and redaction wired into the recent-sends log.
- **Exit criteria:**
  - Precedence unit tests cover every pair of scopes.
  - Switching environments changes resolved URLs and headers (E2E).
  - A test asserts that no secret value appears in any workspace file, log file, or the recent-sends log after a scripted session.
  - A 4KB secret round-trips through the Windows keychain. *(post-V1: §10 › Windows support)*
  - A fresh clone shows the missing-secrets banner.

### Phase 4 — Auth, Bodies & Network (≈3 weeks) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:** auth with inheritance, default headers, form/multipart/binary bodies streamed from disk, minimal proxy, custom CAs, and per-workspace cookie jars. `just check` is green (302 Rust tests, 199 UI tests).

**Engine integration tests cover every exit criterion:**
- a **500MB multipart upload in 0.9s with RSS growing by 13MB** (under the 50MB budget)
- a cookie login across a redirect
- a custom-CA server, plus verify-off
- HTTP/2, including through a proxy tunnel
- HTTP and HTTPS proxies with Basic auth, a 407, and `NO_PROXY` matching
- `Proxy-Authorization` never reaching the origin
- pool isolation by proxy and by CA set

`just e2e` adds `tests/e2e/network.py` against the real app:
- a form login whose 302 sets a cookie, which is then sent on the redirect hop and on later requests
- folder Bearer auth inherited with its `{{token}}` resolved
- the collection's default header sent
- a multipart upload of a workspace file
- the cookies dialog
- the jar persisted in the app data dir, never in the workspace

**Decisions made while building:**
- Files outside the workspace are uploaded only if picked this session. Pickers run in Rust (`PathGuard::is_allowed_file`), so a cloned workspace can't silently upload local files.
- Scratch requests resolve collection and environment variables, but don't inherit collection auth or default headers.
- Header precedence: user, then auth, then folder defaults (innermost first), then collection defaults, then derived `Content-Type`.
- The multipart boundary is derived from the parts, so the preview matches what is sent.
- `NO_PROXY` doesn't bypass loopback implicitly.
- The proxy password and global secrets are stored in the keychain.
- Cookie files are written user-only (0600).
- App settings are in `<app data>/settings.json`; workspace CA paths are in `.ironcall/local.yaml`.

**Carried forward:**
- Any directory appears in the tree, including folders that only hold upload data.
- Cookie renames are a delete followed by a set.
- Cookie expiry is edited in UTC.
- Windows checks, including the proxy against a real corporate proxy. *(post-V1: §10 › Windows support)*

**Goal:** Cover everyday real-world API needs, including corporate networks.
- Auth: None, Inherit (from folder/collection), Basic, Bearer, API Key (header/query). The UI shows where inherited auth comes from.
- Bodies: form-urlencoded, multipart/form-data (text and file parts), and binary file. File paths are stored **relative to the workspace** when possible. A missing file shows a clear pre-send error. Uploads stream with progress (§3.4).
- **Minimal proxy**: `HTTP(S)_PROXY` / `NO_PROXY` env vars, a manual proxy URL with optional Basic auth (credentials stored as secrets), a no-proxy list, and a per-request "bypass proxy" toggle. HTTPS goes through a CONNECT tunnel.
- Cookie jar: persisted per workspace in the app data dir, auto-send, viewer/editor, clear.
- TLS:
  - OS trust store
  - custom CA certs (global and per workspace)
  - per-request "verify TLS" toggle with a visible warning badge
  - connection pool kept separate per TLS config (§3.4)
- HTTP/2 via ALPN with a per-request version override. Redirect settings (follow on/off, max count). Decompression.
- Default headers at collection/folder level (e.g. `User-Agent`).
- **Exit criteria:** integration tests pass for:
  - Basic, Bearer and API key auth, including inheritance
  - multipart upload of a 500MB file with flat memory use (RSS grows by less than 50MB)
  - a cookie login flow
  - a self-signed server with a custom CA, and with verification off
  - HTTP/2 negotiation
  - an HTTP and HTTPS proxy (local test proxy), including `NO_PROXY` matching
  - pool isolation, so a verify-off connection is never reused for a verify-on request

### Phase 5 — Advanced Responses & History (≈3 weeks) — ✅ done 2026-10-05; performance budgets met 2026-10-07 on a GPU desktop (see Phase 8)
**Outcome:**
- **Large bodies:** streaming pretty-print of JSON/XML of any size (431MB/s, constant memory); cancellable forward and backward search with Unicode case folding; columns of very long rows rendered on demand; "Save response…".
- **Previews:** image, PDF (bundled pdf.js) and HTML, the HTML served from a separate `ironcall-preview` origin with `default-src 'none'` in an empty-sandbox iframe. The `ironcall` scheme serves only inert media types as-is (no HTML or SVG).
- **Timing and TLS:** the waterfall, a TLS and certificate summary, captured 1xx responses (HTTP/1.1; hyper doesn't expose them on HTTP/2), and a per-send timeline/console with sensitive headers and secrets masked. The timeline is also kept for failed sends.
- **History:** SQLite plus FTS5 in the app data dir (0600). It stores the authored request (`{{vars}}` kept, literal credentials masked), the masked as-sent request and response metadata, and bodies up to the cap. Search, filters, paging, reopen or resend, delete and clear, retention settings, and a per-workspace "don't store bodies" setting.

`just check` is green (343 Rust tests, 261 UI tests). `just e2e` now also runs `tests/e2e/responses.py`, and all 52 E2E checks pass.

**Exit criteria, measured in this container** (release build, Xvfb with software rendering, memory as PSS summed over the app and WebKit processes):

| Criterion | Result |
|---|---|
| 100MB JSON, search's first hit | 0.66–0.70s ✅ (budget 1s) |
| 100MB JSON, first rows visible | 0.4s after Send |
| Image and PDF render | ✅ |
| HTML preview makes zero network requests | ✅ (a tracker image, CSS and a script were all blocked) |
| History holds no sensitive header value | ✅ (byte scan of the database) |
| Timings vs `curl -w` | ✅ (Phase 0 `timing_compare`; unchanged) |
| **Worst frame gap scrolling 100MB** | **55–59ms ❌** (budget 50ms) |
| **Total memory with a 100MB body** | **498–546MB ❌** (budget 400MB) |

Most of the memory is the webview's idle baseline under software rendering (about 225MB, with 371MB for the whole app idle). The Rust side adds about 22MB for a 100MB body. The webview adds about 42MB showing it and about 77MB during an aggressive scroll test (bounded page cache, so this is JavaScript-engine garbage, not a leak). Both budgets are reported rather than enforced unless `IRONCALL_E2E_PERF=1`.

**Phase 5 budgets, follow-up:**
1. ~~Re-measure on the reference hardware~~ ✅ 2026-10-07 on a Linux desktop with a GPU: worst frame gap 29–39ms, total memory 319–322MB, both within budget (16–17ms and 279–282MB after ADR 0007; see Phase 8).
2. ~~If still over budget, send body pages as one binary blob instead of JSON row objects, and/or shrink page and overscan sizes.~~ Not needed; kept in §10 as an option if a slower machine misses the budget.

**Decisions made while building:**
- Scratch and history bodies are spooled like responses.
- Memory budgets are measured as PSS, not RSS, because RSS counts shared WebKit/GTK libraries once per process.
- An external change that lands while IronCall's own save is still in flight is treated as a conflict (safe, but it can show the conflict bar instead of a silent reload in that narrow window).

**Carried forward:**
- Audio/video preview (post-V1, as planned).
- 1xx on HTTP/2.
- The horizontal virtualisation assumes a monospace font.
- The store's body search still has its own `fold_case`, which should switch to `ironcall_core::content::fold_case`.

**Goal:** Inspect anything, including very large or binary responses, and get back to earlier requests.
- Large bodies:
  - streaming download progress
  - virtualised paged viewer with Rust-side search
  - pretty-printing runs in Rust on a background task; bodies over 10MB default to raw view, with a "pretty-print anyway" option
  - "Save response to file"
- Previews:
  - **images** through the custom protocol
  - **PDF** with bundled pdf.js
  - **HTML** (see below)
  - **hex view** for other binary types
- **HTML preview hardening**: the HTML is served from a separate custom-protocol origin with a response CSP of `default-src 'none'; style-src 'unsafe-inline'; img-src data:`, so no remote fetches happen. The iframe is sandboxed without scripts by default. If scripts are enabled, `allow-same-origin` is **never** added at the same time.
- **Timing waterfall**: DNS / connect / TLS / TTFB / download, with `reused` connections marked. **Redirect chain** with per-hop timings. Negotiated protocol and a TLS summary (version, cipher, leaf certificate subject/issuer/expiry).
- **Spike follow-ups:** surface `1xx` responses (e.g. `103 Early Hints`) in the timeline, and define TTFB as "first final response headers" (curl counts the 103). Render only the visible columns of very long rows (minified JSON is the viewer's weak case: 48ms worst frame). Show placeholder rows during scrollbar drags. Make search cancellable, with Unicode case folding.
- **Timeline / console** for each send, like `curl -v`: the request line and headers as sent (redacted), redirects, TLS events, proxy CONNECT, and errors, in one scrolling log.
- **History**:
  - every send is logged to SQLite: request snapshot (redacted per §6.3), response metadata, timings, and the body up to a cap
  - searchable (FTS5), with filters by workspace, status and method
  - an entry can be reopened into a scratch tab or resent
  - configurable retention (defaults: 1MB per body, 500MB total, 90 days), a per-workspace "don't store bodies" setting, and Clear history
- **Exit criteria:**
  - A 100MB JSON response: no main-thread task over 50ms, total RSS across all processes under 400MB, search returns its first hit in under 1s.
  - Image and PDF responses render on both OSes.
  - Timings fall within ±10% or ±5ms (whichever is larger) of `curl -w` over 20 runs against a local server, for both cold and reused connections.
  - The HTML preview makes zero network requests (checked in devtools).
  - History contains no sensitive header values (automated test).

### Phase 6 — Import & Export (≈2–3 weeks; can run in parallel with P4–P5) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:**
- **cURL import:** a hand-written tokenizer (bash/POSIX, `$'…'`, continuations, Chrome's cmd `^` format) and a parser for the plan's flag set plus `--json`, `-A`, `-e`, `--oauth2-bearer`, `-m`, `-I` and `--http1.1`/`--http2`. Unsupported flags are listed in warnings and never dropped silently. Paste a command into the URL bar (undoable) or use the Import dialog.
- **Copy as cURL:** POSIX and PowerShell, in three modes: with `{{variables}}` (inherited auth and default headers also left unresolved), resolved with secrets masked (the default), or resolved with secrets (needs an explicit confirm, and is never remembered).
- **OpenAPI import:** Swagger 2.0, OpenAPI 3.0 and 3.1, from JSON or YAML, with a preview before anything is written.
  - Tags become folders, path params become `:name`, query and header params map across, and bodies come from spec examples only.
  - Servers (and server variables) become an environment, and security schemes become auth.
  - Local `$ref` is followed with cycle protection; external refs give warnings.
  - The spec's auth is set only on folders and requests the import creates; existing folders and the collection are never changed.

`just check` is green (384 Rust tests, 289 UI tests). `just e2e` adds `tests/e2e/importing.py`, and all 63 E2E checks pass:
- a cURL import sent end to end
- Copy as cURL masking a literal token
- a pasted Petstore spec imported into a real workspace: folder, 3 requests on disk, active environment with `baseUrl`, and the collection file byte-identical

**Exit criteria:**
- **Specs:** the Petstore specs (2.0, 3.0, 3.1) plus 10 edge-case specs have `insta` snapshots. Stripe (8.3MB, 612 requests, 62ms) and GitHub (13MB, 1,232 requests in 44 folders, 63ms) plan without errors; that test is ignored by default because it downloads the specs.
- **cURL:** 35 cURL samples (Chrome bash and cmd, Firefox, Postman, Insomnia, docs styles) have snapshots, and all 35 round-trip curl → IronCall → curl.

**Decisions made while building:**
- Specs are mapped from `serde_json::Value` by hand rather than with `openapiv3`/`oas3`, whose strict types break on real specs.
- Untagged operations stay at the root.
- With several security options, only the first is used, with a warning.
- `WireContext.allow_unresolved` exists for template exports only.

**Fixed along the way:**
- Context-menu actions ran after the menu unmounted.
- Modals didn't trap focus.
- The Import dialog's buttons could be cut off on short windows (now a sticky footer).
- The environment switcher didn't show a newly created active environment (it now uses `<option selected>`).

**Carried forward:**
- PowerShell and plain cmd cURL *import* (post-V1, as planned).
- The import preview tree isn't an ARIA tree.
- A large import (e.g. Stripe's 612 requests) rescans the tree after every created file; it's fine at this size, but a batch API would be faster.

**Goal:** Get existing work in and out.
- **cURL import**: paste into the URL bar (auto-detected) or use the Import dialog.
  - Supported flags: `-X`, `-H`, `-d/--data/--data-raw/--data-binary/--data-urlencode`, `-F`, `-u`, `-b`, `-k`, `--compressed`, `-L`, `--url`, `-G`
  - Shell formats: **bash/POSIX** quoting, and **Chrome's "Copy as cURL (cmd)"** (`^` escapes)
  - Unsupported flags are listed in a warning, never silently dropped
- **cURL export**: "Copy as cURL" in bash and PowerShell flavours. Secrets stay as `{{var}}` or masked by default (§6.3).
- **OpenAPI import** (from file; YAML or JSON):
  - detects 3.0 / 3.1 / Swagger 2.0 and uses the matching parser (§3.3)
  - tags become folders and operations become requests
  - `{param}` paths become path params
  - servers become a `baseUrl` variable in a generated environment
  - security schemes map to Basic/Bearer/API key auth where possible
  - descriptions go into the `description` field
  - example bodies are taken **only from examples in the spec** (`example` / `examples`); requests without one get an empty body of the right content type
  - local `$ref` is supported; external refs are reported but not followed
  - an import preview shows a warnings list before anything is written
- **Exit criteria:**
  - Named corpus: Petstore (2.0, 3.0, 3.1), Stripe, GitHub, plus 3 or more internal specs all import without errors, and their outputs match `insta` snapshots.
  - 30 or more real "Copy as cURL" samples (Chrome/Firefox, bash and cmd) import correctly.
  - A curl → IronCall → curl round-trip is semantically equal for the bash corpus.

### Phase 7 — Assertions, Chaining & Collection Runner (≈2–3 weeks; needs P5) — ✅ done 2026-10-05 (Linux; Windows checks moved post-V1)
**Outcome:**
- **Assertions:** all twelve operators over status, header, body (JSONPath, RFC 9535 via `serde_json_path`), body text, duration and size, edited in the request's **Tests** tab and shown in the response's **Tests** tab (`Tests 3/4`, failures marked).
- **Extraction** from body (JSONPath), header, cookie (`Set-Cookie`) and regex into the **runtime scope**. Runtime variables win over every other scope and are never written to disk. The quick look can save one to the active environment (as a secret if sensitive) or clear it.
- **Collection runner:** run the workspace (toolbar **Run**) or a folder (context menu), with environment, iterations, delay, stop on first failure, and parallel mode. Live progress, a results table with per-request details, **Open in history** drill-down, and **Copy results as Markdown**.
- All evaluation and run logic is in `ironcall-core` (`assertions`, `extract`, `checks`, `runtime`, `runner`). Sending is behind an `Executor` trait, so core needs no HTTP stack or async runtime; the Tauri side (`run.rs`) only loads requests, sends them through the normal send path and emits `RunProgress`.

`just check` is green (416 Rust tests, 302 UI tests). `just e2e` adds `tests/e2e/runner.py`, and all 75 E2E checks pass.

**Exit criteria:**
- **Chaining:** login → create → fetch → delete runs with chained IDs and a bearer token against a local server, both as a Rust test (`run::tests`) and as E2E. The extracted token never reaches history metadata, logs, session state or workspace files.
- **Operators:** every operator has unit tests, including typed versus loose comparison, missing values, and invalid regex, type and list errors.
- **Cancelling:** a cancelled run keeps only values from requests that finished. A request's extractions apply only after it completes, which is tested in core with a fake executor and in the app with a request cancelled mid-flight.

**Decisions made while building:**
- **Comparison typing:** body values are typed (`201` ≠ `"201"`). Headers, body text, status, duration and size compare loosely, and so does any expected value written with `{{variables}}`, since variables are text. Labels show the authored text, so a resolved secret never appears in one.
- **Always sensitive:** cookies, denylisted headers, and token- or password-like variable names and JSON fields, whatever the checkbox says (§6.2). Sensitive runtime values become secrets in the resolver, so existing masking and redaction apply.
- **Run scope:** a run starts from the session's runtime variables and uses its own environment choice. Its runtime scope is merged back into the session when it ends, so a login run sets up later manual sends.
- **Failures:** a request fails if any enabled assertion fails or any extraction fails. An HTTP 5xx with no assertions still passes. Connection errors count as errors, not failures.
- **Parallel mode:** up to 6 requests at once. Chaining between requests of the same iteration isn't reliable in this mode, and the UI says so.
- **Body limit:** assertions and extraction read bodies up to 32MB; larger or binary bodies fail body checks with a reason.

**Carried forward:**
- History doesn't store assertion results, so drill-down shows the request and response but not the checks.
- `extract` from cookies reads the final response's `Set-Cookie` only, not redirect hops (the jar still sends those cookies).
- JUnit/JSON reports come with the CLI (post-V1), as planned.

**Goal:** Lightweight automated API checks without scripting.
- Declarative **assertions** editor:
  - targets: status, header, body (JSONPath), body text/regex, duration, size
  - operators: `eq`, `neq`, `lt`, `lte`, `gt`, `gte`, `contains`, `matches`, `exists`, `notExists`, `type`, `in`
  - pass/fail shown in the response panel
- **Extraction** into the runtime scope (JSONPath, header, regex, cookie), with a `sensitive` flag (§6.2). Values can optionally be saved to the active environment.
- **Collection runner**:
  - run a workspace or folder in `order`
  - options: environment, number of iterations, delay between requests, stop on first failure, run requests in parallel or one at a time
  - live progress, a per-request results table, and drill-down into the stored request/response (from P5)
  - "Copy results as Markdown" for sharing
- All runner logic lives in `ironcall-core`, with no Tauri dependency.
- **Exit criteria:**
  - A login → create → fetch → delete flow against the test server runs with chained IDs.
  - Every assertion operator is covered by unit tests.
  - A cancelled run leaves no runtime-scope values from requests that didn't finish.

### Phase 8 — Polish, Hardening & V1.0 (≈3 weeks) — ✅ released as 1.0.0 (2026-10-08, Linux)
**Outcome:**
- **Shortcuts:** Ctrl+P quick open (workspace and scratch requests, by name, folder, method or URL), Ctrl+L, Ctrl+, and Ctrl +/−/0 zoom, plus the existing ones.
- **Settings:** theme, font size, zoom (the webview's native zoom), request defaults (timeout, redirects, HTTP version, always-fresh connections) and extra sensitive headers, which are masked wherever the built-in list is. TLS verification can only be turned off per request.
- **First run:** a sample workspace (3 folders, 7 requests with assertions and a chained token) against a built-in echo server in `ironcall-http::echo`, reached through the `{{$localEcho}}` helper so it survives port changes. A Rust test runs the whole sample against the real echo server.
- **Accessibility:** `theme/contrast.test.ts` keeps both themes at WCAG AA (text 4.5:1, accent and control borders 3:1). axe-core audits the main screen, every dialog, every Settings section, runner results and the first-run screens. Focus returns to whatever opened a dialog.
- **Crashes and diagnostics:** a panic hook logs scrubbed messages with a backtrace (`redact::scrub`), UI errors are logged through the backend, a render failure shows a recovery screen, and Settings › About has "Copy diagnostic info" and "Open logs folder".
- **Windows:** user-only ACLs (see below); `just check-windows` (clippy for the Windows target) and `just test-wine` (core, store and secrets tests as Windows binaries under Wine) run from Linux. Wine found a real bug: Windows copies keep their modified time, so duplicate-ID detection could pick the wrong file. It now uses the later of creation and modification time.
- **Docs:** [user guide](user-guide.md), [file format reference](file-format.md), [building](building.md), [CHANGELOG](../CHANGELOG.md) and a README. One app version in `Cargo.toml`, `tauri.conf.json` and `ui/package.json`, checked by a test.

**Released as 0.9.0** (2026-10-06): the Linux AppImage and `.deb`, feature-complete for V1.

**Released as 1.0.0** (2026-10-08, Linux only, [ADR 0004](adr/0004-v1-linux-only.md)). The former blockers:
1. **Performance budgets** ✅. Measured 2026-10-07 on a Linux desktop with a GPU, release build: Ubuntu 26.04, 16 cores, 30GB, Intel Arc + NVIDIA RTX 500 (both drivers load). That's faster than the §12 reference box (4-core, 8GB, Ubuntu 24.04), so the start times have less headroom there. Run: `cargo tauri build --no-bundle`, then `IRONCALL_E2E_XVFB=0 IRONCALL_E2E_PERF=1 IRONCALL_APP=target/release/ironcall python3 tests/e2e/run.py`. All 91 E2E checks passed with the budgets enforced on that run (two more checks were added on 2026-10-08 with ADR 0006). Start time and idle memory were measured with a one-off script: WebDriver launch until the URL field exists, timed from the process's start; idle PSS summed over all processes with no driver attached.
   | Budget | Target | Measured (desktop, GPU; 2026-10-08 after [ADR 0007](adr/0007-webkit-dmabuf-renderer-off.md), with the 2026-10-07 figure in brackets) | Earlier (container, software rendering) |
   |---|---|---|---|
   | Worst frame gap scrolling a 100MB body (Phase 5) | < 50ms | 16–17ms ✅ (29–39ms) | 55–66ms |
   | Total memory with a 100MB body (Phase 5) | < 400MB | 279–282MB ✅ (319–322MB) | 498–546MB |
   | First start / warm start | < 2s / < 1s | 0.93s / 0.84–0.97s (median 0.90s) ✅ | not measured |
   | Idle memory, all processes, GPU driver libraries excluded ([ADR 0008](adr/0008-idle-memory-excludes-gpu-drivers.md)) | < 200MB | 174–181MB ✅ (206–214MB including 32MB of drivers; 214–234MB before ADR 0007) | ~370MB |
   | Tree with 2,000+ requests stays smooth | smooth | opens in 0.9s; worst frame gap 18ms scrolling, 17–18ms filtering ✅ (21ms before ADR 0007; new check in `workspace.py`) | filter < 100ms at 2,000 nodes |

   - The "first start" is not a true cold start: the OS file cache couldn't be dropped without root. To check: `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches`, then launch.
   - **Idle memory** (`just idle-memory`):
     - 2026-10-08: WebKitGTK's DMA-BUF renderer is now off on Linux ([ADR 0007](adr/0007-webkit-dmabuf-renderer-off.md)). Idle memory went from 234MB to 206–207MB. Scrolling a 100MB body went from 36ms to 16–17ms worst frame gap, filtering the 2,000-request tree from 42ms to 17–18ms, and memory with a 100MB body from 317MB to 279–282MB.
     - What remains: the app process about 85MB (40MB its own memory, 18MB the binary), the web process about 99MB (41MB its own), the network process 15MB, and an 8MB `glycin-svg` loader GTK starts on GNOME Wayland.
     - Environment-specific: the NVIDIA driver libraries add about 22MB on this hybrid-GPU laptop. Forcing Mesa only gave 186MB, so a machine without an NVIDIA GPU should be within budget.
     - Tried without effect: `MALLOC_ARENA_MAX=2`, `TOKIO_WORKER_THREADS=2`. The release profile already uses `codegen-units = 1`, `strip` and thin LTO.
     - Closed 2026-10-08 by excluding GPU driver libraries from the budget ([ADR 0008](adr/0008-idle-memory-excludes-gpu-drivers.md)): 174–181MB over several runs, including on the final 1.0.0 build. Idle memory is the only row re-measured on the final build; the others are from the 2026-10-08 ADR 0007 run.
   - Measured on the GPU desktop above, not the §12 reference box; accepted for 1.0 on 2026-10-08. A true cold start (file cache dropped) is still unmeasured.
   - Known E2E flake: on a real display, a WebDriver screenshot occasionally times out late in a full run (1 in about 8 runs). The suite passes on rerun, and the same suite passed 5 of 5 alone with either renderer.
2. **Release** ✅: version 1.0.0, Linux packages built with `just build`, tagged `v1.0.0`. The README was reworked with dark-theme screenshots.

**Review 2026-10-06** (four review agents: backend, security, UI, docs). Fixed: symlink uploads and symlinked folders, secrets with `{{references}}` and API keys under custom names reaching history, cross-origin redirects forwarding sensitive headers, early cancels lost, runs undoing runtime-variable changes, unsaved work lost on window close, stacked-dialog Escape, the Expected field rewriting input, rename/delete not reaching tabs, rename from the context menu, and more (see the git log from `d4db68f`).

**Security hardening** ✅ fixed 2026-10-08 ([ADR 0006](adr/0006-webview-untrusted-paths.md)):
- **Workspace paths from the webview.** Folders are picked with `workspace_pick_folder`, a Rust-side dialog. `workspace_create` and `workspace_create_sample` accept only folders picked this session, and `workspace_open` also accepts the recent list. The JS `dialog:allow-open` permission is gone. An E2E check calls `workspace_open` from the webview with an unpicked folder and expects a refusal. The picker itself (open, new, first-run sample) was tested by hand on Linux on 2026-10-08.
- **The app data folder was a path-guard root.** Only `<app data>/spool` is now; an E2E check expects `open_body` on `session.json` to be refused.
- **A committed `.ironcall/local.yaml` was trusted.** The secret backend is chosen per machine in `<app data>/secret-backends.json`, keyed by the resolved workspace folder. An old `secretBackend` in `local.yaml` is ignored and shown in a banner so the user can switch.
- **Decided, not changed:** workspace CA certificates (`caCerts`) stay in `.ironcall/local.yaml` (decided 2026-10-08). A committed one can add a trusted CA for that workspace's requests, but a repo that can do that already chooses the URLs those requests go to.

**Goal:** Ready for daily use.
- Settings UI: theme, font/size, zoom (Ctrl +/−), default timeout, network defaults (redirects, HTTP version, custom CAs, proxy), history retention, secret backend, sensitive-header list.
- Keyboard shortcuts:
  - Ctrl+Enter send, Ctrl+S save, Ctrl+N new request, Ctrl+W close tab
  - Ctrl+Tab / Ctrl+Shift+Tab switch tabs
  - Ctrl+L focus URL, Ctrl+P quick-open request
  - arrow-key navigation in the tree
- **First run**: empty states, a sample workspace that calls a local echo endpoint built into IronCall (works offline), and a "paste a cURL command" hint.
- Accessibility pass: focus order, ARIA through Kobalte, WCAG AA contrast in both themes.
- Performance budgets, measured on the reference hardware (§12: 4-core, 8GB RAM, SSD; Ubuntu 24.04):
  - cold start under 2s, warm start under 1s
  - idle memory under 200MB **summed over all processes** (WebView2 and WebKitGTK are multi-process), excluding GPU driver libraries ([ADR 0008](adr/0008-idle-memory-excludes-gpu-drivers.md))
  - smooth tree with 2,000+ requests
- Crash/error handling: panic hook (redacted), "Open logs folder", "Copy diagnostic info".
- Windows user-only ACLs: the app data and log folders, and a workspace's `.ironcall` folder when it holds a secrets file, get an explicit user + SYSTEM ACL with inheritance off (`ironcall_core::perms`, via `icacls`, since `unsafe` is forbidden). ✅ implemented; the `responses` E2E suite prints the history database's ACL when run on Windows, which hasn't happened yet (post-V1, §10 › Windows support).
- E2E smoke suite (tauri-driver 2.x pinned) on Linux (`webkit2gtk-driver`). The runner already handles Windows (`msedgedriver` matching WebView2) for post-V1.
- Docs: user guide, file-format reference (including the YAML comment-loss limitation), build instructions, CHANGELOG.
- **Manual build guide**: Linux (AppImage + .deb), with prerequisites listed. Windows instructions (NSIS/MSI, WebView2 bootstrapper) are kept as a preview for post-V1. There is no cross-building.
- Versioning: SemVer for the app. File schema versions (`ironcall/request@N`) change separately and are listed in the CHANGELOG.
- **Exit criteria:**
  - The E2E smoke suite passes on Linux.
  - Every earlier phase's exit tests still pass.
  - No open bugs labelled `data-loss` or `security`.
  - The performance budgets are met on reference hardware (accepted on 2026-10-08 as measured on a faster GPU desktop; see the Outcome above).
  - V1.0.0 is tagged.

### Phase ↔ feature matrix

| Feature | P0 | P1 | P2 | P3 | P4 | P5 | P6 | P7 | P8 |
|---|---|---|---|---|---|---|---|---|---|
| Theme, tokens, logging, IPC | ● | | | | | | | | ◐ polish |
| Send/inspect, path/query params, body store | ◐ spike | ● | | | | | | | |
| Timings | ◐ spike | ◐ total | | | | ● waterfall | | | |
| YAML workspaces, IDs, tree, tabs, scratch | ◐ spike | ◐ scratch | ● | | | | | | |
| Variables / envs / secrets | | | | ● | | | | | |
| Auth, bodies, proxy, cookies, TLS, H2 | | | | | ● | | | | |
| Large/binary preview, timeline, history | ◐ spike | ◐ paged | | | | ● | | | |
| cURL + OpenAPI | | | | | | | ● | | |
| Assertions, extraction, runner | | | | | | | | ● | |
| Settings, a11y, perf, first run, docs, builds | | | | | | | | | ● |

**Total rough estimate:** about 23–29 developer-weeks.
- **1 developer:** about 6–7 months of calendar time.
- **2 developers:** about 4–5 months. P6 runs in parallel, and the UI and Rust work split from P1 onwards. Less than half the time, because of coordination and shared-schema work.

Re-estimate after Phase 0.

---

## 8. Engineering Practices & Testing

### 8.1 Definition of Done (every change)
- `just check` passes (fmt, clippy `-D warnings`, biome, unit and integration tests, `cargo deny`).
- New behaviour has tests at the lowest sensible layer.
- Checked manually on Linux for any UI-, file- or network-facing change, and `just check-windows` still passes (Windows is post-V1, but the code stays portable).
- No secret or sensitive value can reach logs, history or files (redaction tests updated if relevant).
- Docs and an ADR are updated when a decision changes.

### 8.2 Test layers

| Layer | Tooling | Scope |
|---|---|---|
| Rust unit | `cargo test`, `proptest` | Variable resolution and precedence, slugging, YAML round-trip/byte-identity, curl parser, URL encoding, assertion evaluation, redaction |
| Rust integration | Local `axum` + `rcgen` TLS server, a local test proxy; `wiremock` for simple HTTP/1.1 only | HTTP engine, H2, pool isolation, redirects, cookies, proxy, timings, large bodies, uploads, TLS failures |
| Snapshot | `insta` | OpenAPI import output, curl export, YAML serialisation |
| Frontend unit | Vitest + `@solidjs/testing-library` | Stores, components, IPC adapters (mocked) |
| E2E | `tauri-driver` 2.x + a standard-library Python WebDriver client (`tests/e2e/`, run by `just e2e`; Windows + Linux only) | Smoke flows: create workspace → request → send → save → env switch → run collection. Each suite sets `IRONCALL_DATA_DIR` (app data, logs and webview storage) to a temporary folder, so it never touches the user's data. On Windows the runner fetches the `msedgedriver` matching WebView2 |
| Security | Automated tests | Asserts no secret or sensitive values appear in workspace files, history, logs, spool files left after exit, or default cURL export |

There's no release CI for V1, but `just check` must pass locally before every merge. Adding a test-only GitHub Actions workflow (Windows + Linux) is cheap and strongly recommended.

---

## 9. Security Considerations
- Tauri capabilities/permissions kept to the minimum. A strict CSP that lists both custom-protocol URL forms. No remote content in the main webview.
- **Path guard** in Rust for every file-touching command (§3.1). Capability scopes don't cover Rust commands.
- HTML preview isolation as described in Phase 5.
- Central redaction (§6.3), `secrecy` types, and spool files with user-only permissions that get cleaned up.
- Disabling TLS verification is always visible (badge on the request and in the response). Connections without verification are kept in a separate pool.
- Proxy credentials are stored as secrets.
- Dependency auditing with `cargo deny` (advisories, licences, bans: e.g. `serde_yml`) and `npm audit`.

---

## 10. Post-V1.0 Extended Features (Optional)

Grouped by theme. These are not in a fixed order; prioritise after V1 feedback.

### Shortlisted (picked 2026-10-06; first candidates after V1)

- **Webhook catcher.** Inspect callbacks from payment, Git, chat and CI services
  locally. It builds on the built-in echo server (`ironcall-http::echo`).
  - **Catchers:** create a catcher in a workspace. It listens on 127.0.0.1 at a path such
    as `/hooks/<name>`, and its URL is available as `{{$hook.<name>}}` (or a
    `{{$localEcho}}`-style helper) to paste into a provider's settings.
  - **Inbox:** each received request is listed with its time, method, headers and body,
    shown in the normal response viewers. Sensitive headers are masked, as everywhere.
  - **Replies:** a catcher answers with a configurable status, headers and body
    (default `200 {}`), with an optional delay to test provider retries.
  - **Actions:** open any received call as a new request, to replay it against your own
    service, or as an assertion target. "Wait for the next hook" can be a runner step
    with a timeout, for end-to-end flows such as triggering a payment and then
    asserting on the webhook.
  - **Reaching it from the internet:** IronCall only listens locally. Users bring their
    own tunnel (cloudflared, ngrok); a built-in tunnel is out of scope.
  - **Signature checks** for common providers (Stripe, GitHub, Slack HMAC) as an
    assertion, using a secret variable.
  - **Storage:** received calls live in the app data dir with the same user-only
    permissions and retention as history, never in the workspace.
- **Record and replay.** Run requests and collections offline, in demos, against flaky
  APIs, or in CI without the real backend.
  - **Recording:** a request, folder or run saves responses (status, headers and body)
    into a **cassette** keyed by request ID and the resolved method and URL, with
    optional body matching.
  - **Replay mode:** set per run or per environment ("Replay: cassette X"). Matching
    requests are answered from the cassette without touching the network, and misses
    are an error by default, or fall through to the network.
  - **Storage:** cassettes are files in the workspace (e.g. `recordings/<name>/`),
    committable and diffable, so they also work as fixtures for teammates and the
    future CLI. Responses are redacted when recorded: sensitive headers are masked,
    and a scrub list removes values such as tokens from bodies.
  - **Relation to the planned mock server:** this is the same idea, but recorded rather
    than generated from a spec. Share the matching engine.
- **Pagination helper.** Fetch every page of a paginated endpoint with one request.
  - **Setup:** in a request's Settings or Tests tab, choose a strategy:
    - `Link` header (`rel="next"`)
    - a next-URL or cursor field (JSONPath), sent back as a query parameter
    - page or offset numbers with a stop condition (an empty page, or a total field)
  - **Results:** the response view shows each page plus an optional combined view, e.g.
    concatenating `$.items` across pages into one JSON array. Assertions can run per page
    or on the combined result.
  - **Limits:** a maximum page count (default 50) and the existing delay setting, to stay
    within rate limits. Honouring `Retry-After` belongs with rate-limit awareness.
  - **Runner and history:** the runner treats a paginated request as one step. History
    records each page as its own entry, grouped under the run.

### Undecided (ideas from 2026-10-06, to review)

Not yet accepted or rejected. Move an item to Shortlisted or a themed list below once
decided, or delete it.

- **Testing and automation**
  - Click a value in the response to add an assertion or extraction for it (fills in the JSONPath).
  - JSON tree view of the response body with "Copy JSONPath" on every node.
  - Response validation against the imported OpenAPI spec (status, headers, body schema) as an assertion.
  - Folder-level assertions inherited by every request inside (e.g. status < 500, duration < 1000ms).
  - Snapshot assertions: save a response, fail when it changes, ignoring fields marked volatile.
  - "Run from here" in a folder, and re-run only the failures of the last run.
  - Trend chart of a request's response times across history.
- **Interop:** import and export of `.http` files (VS Code REST Client, JetBrains HTTP Client) and possibly Hurl.
- **Secrets and teams**
  - External secret providers: 1Password CLI, Bitwarden, HashiCorp Vault, AWS Secrets Manager, `pass`, `.env` import.
  - Team-shared encrypted secrets committed to the repo with `age`/SOPS (one recipient per teammate), alongside the per-machine model.
  - `ironcall lint` as a pre-commit hook: literal tokens or passwords in YAML, unresolved variables, requests with no assertions.
- **Editing and navigation**
  - Bulk-edit headers and params as plain text (`Name: value` lines).
  - Workspace-wide find and replace (rename a variable, change a path prefix).
  - Raw request mode: write and send the exact HTTP/1.1 bytes.
  - Favourites and tags on requests, and a workspace-wide "recently sent" view (the toolbar's Recent covers only the active tab).
  - Environment diff: compare two environments' variables side by side.
- **Network and protocol**
  - Capture proxy: a local proxy that records another app's or browser's traffic as requests (like Charles or HTTP Toolkit).
  - Rate-limit awareness: show `RateLimit-*` and `Retry-After`, and optionally have the runner honour them.
  - Compressed request bodies (gzip, br).
- **Platform and other**
  - Portable mode: the data folder next to the executable.
  - High-contrast and custom themes; screen-reader announcements when a response arrives.
  - An MCP server so AI coding agents can list, run and check collections (fits with `ironcall-cli`).

### Protocols
- GraphQL: query editor, variables, schema introspection, autocomplete, docs explorer.
- WebSocket: connect, send, message log, saved messages.
- Server-Sent Events: stream viewer.
- gRPC: `.proto` loading and server reflection, unary and streaming calls.
- HTTP/3 (QUIC via `quinn`/`h3`).

### Scripting & Automation
- Sandboxed JavaScript pre-request/post-response scripts (`rquickjs` or `boa`) with a `ironcall.*` API (a Postman `pm.*` compatibility subset).
- Dynamic helpers that take arguments (`{{$base64 …}}`, `{{$hmac …}}`, date formatting).
- **`ironcall-cli`**: `ironcall run <workspace> --env dev` for CI pipelines.
- **JUnit XML and JSON run reports** (desktop runner and CLI).
- Data-driven runs (CSV/JSON iteration data).
- Scheduled / monitored runs.
- Simple load testing (concurrency, RPS, latency percentiles).

### Import / Export / Interop
- Postman v2.1 collections and environments import/export.
- Insomnia v4/v5 import.
- Bruno and HAR import.
- PowerShell and Windows cmd cURL **import** (beyond Chrome's cmd format).
- OpenAPI **example generation from JSON Schema** (3.1 / 2020-12, `allOf`/`oneOf`, cycles), import from URL, and following external `$ref`s.
- OpenAPI **export** and spec sync (detect drift between spec and collection).
- Code generation: Python `requests`, JS `fetch`/axios, Rust `reqwest`, C# `HttpClient`, Go, PowerShell.
- Collection documentation generation (static HTML/Markdown).

### Storage
- **Sidecar body files** (`*.body.json`) for editor-friendly diffs of large bodies.
- Comment-preserving YAML edits (CST-based writer).

### Auth
- OAuth 2.0: authorization code + PKCE (system browser + loopback redirect), client credentials, device code, password. Token caching and refresh.
- Client certificates / mTLS (PEM, PFX/P12, per-host).
- Digest, NTLM/Negotiate (useful for Windows intranet APIs), AWS SigV4, Hawk, JWT generation.

### Network
- Full proxy support: OS system proxy auto-detection, PAC, SOCKS5, NTLM/Negotiate proxy auth.
- DNS overrides (host → IP mapping), IPv4/IPv6 preference.
- Request throttling / network condition simulation.

### Responses
- **Audio/video preview.** On Linux this needs a loopback HTTP server or blob URLs, because GStreamer can't load custom schemes.
- **Full certificate chain viewer.**
- **Body pages as one binary blob.** If a slower machine misses the 100MB-body budgets (Phase 5), send each page over the custom protocol as one binary response instead of JSON row objects, and shrink the page and overscan sizes. Not needed on the hardware measured for 1.0.
- Response body filtering (JSONPath/XPath) in the viewer.
- Response diff (vs. history or another response).
- Saved example responses per request.

### UI / UX
- Command palette and fully configurable keybindings.
- Multiple windows / multiple workspaces open at once; split panes; detachable tabs.
- Global search across workspaces and history.
- Built-in git awareness (changed files, branch indicator, commit from the app).
- Internationalisation.

### Platform & Distribution
- **Windows support** ([ADR 0004](adr/0004-v1-linux-only.md)). The code is already cross-platform; what's left is verification and packaging on Windows 11:
  - `just check` and `just e2e` (the runner fetches the `msedgedriver` matching WebView2)
  - `timing_compare` and the viewer benchmark (`just bench`), and the performance budgets on Windows reference hardware
  - the real-keychain test (4KB secret through Credential Manager)
  - trash, file locking and CRLF behaviour; duplicate IDs after an Explorer copy-paste
  - `{{var}}` highlight alignment in WebView2
  - a real corporate proxy
  - the user-only ACLs (`icacls` on the app data folder)
  - NSIS and MSI installers (`just build`) with the WebView2 bootstrapper
- macOS support (WKWebView, signing/notarisation, WebDriver E2E isn't available on macOS).
- GitHub Actions release pipeline (Windows + Linux + macOS artefacts).
- Tauri auto-updater with signed releases.
- Windows code signing.
- Flatpak / winget / Scoop packaging.

### Extensibility & Other
- Plugin system (WASM-based via `wasmtime`, or JS) for custom auth, importers and template helpers.
- Local mock server generated from a collection or OpenAPI spec.
- Optional sync (self-hosted or file-share based) for teams that don't use git.
- AI assistant features (generate requests from natural language, explain responses). Opt-in only.

---

## 11. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Own hyper-based stack (pool, timings, proxy, redirects) grows into "rewriting reqwest" | High | Spike A kept hyper, but the remaining work (redirects, proxy, decompression, cookies, streaming) is estimated at 2.5–3 weeks, right at the limit. Track it against that estimate in P1/P4, and keep the engine behind a trait so libcurl stays possible |
| WebKitGTK quirks and performance on Linux (large DOMs, media, PDF) | Medium | Paged viewer, pdf.js, no media in V1, test on Linux every phase |
| Large bodies blow up IPC/memory | High | Body store + spool + paged API + Range slices from Phase 1 |
| YAML edge cases (implicit typing, comments lost, merge conflicts) | Medium | Forced quoting, byte-identical re-save tests, stable IDs and file names, error nodes for invalid files, documented comment loss |
| Windows file-system rules (reserved names, case, MAX_PATH, CRLF) | Medium | Slug rules, `.gitattributes`, dedicated tests on Windows |
| Linux keychain unavailable / Windows keychain size limit | Medium | User-selectable backends, auto-detect + suggestion, chunking on Windows |
| Sensitive data leaks (history, logs, spool, cURL export, HTML preview) | High | Central redaction, `secrecy`, isolated preview origin, security test layer |
| Corporate network blocks traffic without a proxy | High | Minimal proxy in P4; full proxy post-V1 |
| Pre-1.0 / RC dependencies (tauri-specta, Kobalte) | Medium | Exact version pins, thin wrappers, fallbacks (ts-rs, own components) |
| OpenAPI and curl variety | Medium | Scoped-down V1 (spec examples only, bash + Chrome cmd), named test corpora, warnings rather than silent drops |
| Scope creep toward Postman parity | High | Measurable phase exit criteria; everything else goes to §10 |

---

## 12. Resolved Questions

All open questions are resolved, except two that are due before IronCall is distributed publicly: the licence (there is no `LICENSE` file yet) and the trademark search.

| Question | Decision |
|---|---|
| Product name | **IronCall** (iron for Rust, "call" for API calls; crates `ironcall-*`, CLI `ironcall`). Earlier candidates were dropped after clash checks: "Rivet" (existing software), "Reqo" (reqo.dev is an API toolkit, and a `reqo` CLI HTTP client exists). On 2026-10-05, `ironcall` was free on crates.io and npm, and ironcall.io looked unregistered. Known prior use: an old interventional-radiology reference iPhone app called IronCall. Run a formal trademark search (IP Australia, USPTO, EUIPO; classes 9 and 42) before any public release |
| Licence | Not decided yet; chosen before sharing beyond the core team (see §1) |
| Team size | 1–2 developers until stated otherwise (see §7) |
| History retention defaults | 1MB per stored response body, 500MB total, 90 days. All configurable in Settings |
| Reference hardware | A modest office PC: 4-core CPU (2019 or newer), 8GB RAM, SSD, 1080p display, running **Ubuntu 24.04 LTS** (Windows 11 after V1) |

New questions go here as they come up.

---

## Appendix A — Review Log (2026-10-05)

Three review agents checked draft v1 (feature gaps vs. Insomnia/Postman/Bruno/Hoppscotch/curl, technical fact-check, adversarial review). Main changes in v2:

- **Corrected:**
  - YAML crate is `serde-saphyr`, not `saphyr`; `serde_yml` banned
  - documented how unknown keys are preserved with serde, and its limits
  - OpenAPI parser chosen by version
  - `keyring` v4, with the real Windows size limit and chunking
  - custom protocol can't stream, so Range slices instead; platform URL forms
  - tauri-specta is RC-only, so pinned
  - `notify-debouncer-full`
  - CodeMirror isn't suitable for very large bodies
  - Linux media/PDF limits
  - Tauri capability scopes don't cover Rust commands
- **Decided with the user:**
  - Environment overrides Collection
  - minimal proxy moved into V1
  - stable file names + IDs
  - moved to post-V1: JUnit/JSON reports, audio/video and cert-chain viewer, sidecar bodies, schema-based OpenAPI example generation
- **Added to V1:**
  - path params and URL encoding rules
  - scratch requests
  - auto headers shown, header autocomplete, beautify
  - invalid JSON and charset handling, response search, sidebar filter
  - descriptions, delete to trash, error nodes for invalid files
  - missing-secrets banner, sensitive-data redaction model, HTML preview isolation
  - timeline/console, own connection pool with TLS isolation and `reused` timings
  - cancellation and upload semantics
  - first-run sample workspace
  - Definition of Done, ADRs, effort estimates, measurable exit criteria
  - English-only and no-telemetry statements
- **Follow-up decisions:** product renamed from Rivet to IronCall (Reqo was considered and rejected after a clash check); history retention defaults set; reference hardware defined; team size 1–2; licence deferred.
- **Re-ordered:**
  - body store/spool and timing capture moved into P1
  - logging into P0
  - the runtime scope stays empty until P7
  - P7 depends on P5; P6 can run in parallel with P4–P5

### Phase 0 spike results (2026-10-05)
- **ADR 0001, HTTP engine:** kept hyper. Timings agree with `curl -w` on localhost and on warm requests. On cold remote requests, DNS/TLS came out 5–7ms lower than curl's; this is put down to curl's per-process startup (resolver setup, CA bundle loading). The Cloudflare TTFB gap is due to `103 Early Hints`. Linux only so far.
- **ADR 0002, YAML:** serde-saphyr. All 18 canonical fixtures re-save byte-identically, and a proptest of 256 random bodies round-trips. Merge-conflict markers and syntax errors are reported with their line.
- **ADR 0003, body viewer:** a custom scaled-scrollbar viewer instead of TanStack, which can't work because of WebKitGTK's element-height cap. On a 100MB body: index built in 740ms (debug; 57ms in a release bench), 200-row pages in 2ms p50 over IPC, worst frame gap 48ms. Range-capable custom scheme confirmed.
