# Building IronCall

IronCall 1.0 supports **Linux**. Windows support comes after 1.0
([ADR 0004](adr/0004-v1-linux-only.md)); the Windows instructions below are a preview and
untested. Packages are built on the platform they're for; there's no cross-building.

| Platform | Packages | Output folder |
|---|---|---|
| Windows 10/11 (x64), preview | NSIS installer (`.exe`) and MSI | `target/release/bundle/nsis/`, `target/release/bundle/msi/` |
| Linux (x64) | AppImage and `.deb` | `target/release/bundle/appimage/`, `target/release/bundle/deb/` |

## Prerequisites

### Windows (preview, not supported in 1.0)

1. [Rust](https://rustup.rs) with the MSVC toolchain: `rustup default stable-msvc`.
2. Visual Studio Build Tools with **Desktop development with C++**
   (`winget install Microsoft.VisualStudio.2022.BuildTools`, then add the workload).
3. Node.js 24 or newer (`winget install OpenJS.NodeJS`).
4. `just` (`winget install Casey.Just`), Python 3 (for `just e2e`) and the Tauri CLI:
   `cargo install tauri-cli --version "^2" --locked`.
5. For the MSI: WiX Toolset v3 is downloaded by the Tauri bundler on first use. For the
   NSIS installer, NSIS is too.

The WebView2 runtime comes with Windows 11 and up-to-date Windows 10. The installers
include the small WebView2 bootstrapper, which installs the runtime silently on machines
that don't have it (this needs internet access during installation).

### Linux (Ubuntu 24.04 or Debian 12; other distributions need the equivalent packages)

```sh
sudo apt-get install build-essential curl pkg-config libwebkit2gtk-4.1-dev librsvg2-dev
```

(`librsvg2-dev` is for the AppImage bundler, which checks for it. The longer list in
Tauri's own docs adds `libssl-dev`, `libxdo-dev` and `libayatana-appindicator3-dev`,
which IronCall doesn't use: TLS is rustls and there's no tray icon.)

Then Rust 1.90 or newer (via [rustup](https://rustup.rs)), Node.js (tested with 24), `just`,
`cargo install tauri-cli --version "^2" --locked`, and (for `just check`)
`cargo install cargo-deny --locked`. For `just e2e`: `sudo apt-get install
webkit2gtk-driver xvfb python3` and `cargo install tauri-driver --version "^2" --locked`.

Bundling the AppImage downloads `linuxdeploy` and its plugins from GitHub the first
time, so the build machine needs internet access.

At runtime the `.deb` depends on `libwebkit2gtk-4.1-0` and `libgtk-3-0`, which `apt`
installs with it. The AppImage bundles GTK but needs FUSE 2 (`libfuse2` on Ubuntu 22.04
and later, or `libfuse2t64` on 24.04); without it, run it with
`--appimage-extract-and-run`. Storing secrets in the OS keychain needs a Secret Service provider (GNOME Keyring or KWallet); without
one, IronCall offers the encrypted-file backend.

## Build

```sh
git clone https://github.com/sennlabs/IronCall.git
cd IronCall
npm --prefix ui install
just check     # optional, recommended: formatting, lints, tests
just build     # release build and installers for this platform
```

`just build` runs `cargo tauri build`, which first builds the UI (`npm run build`) and
then the release binary with link-time optimisation. Expect a few minutes for a first
build. To build only some package types: `cargo tauri build --bundles nsis` (Windows) or
`cargo tauri build --bundles deb` (Linux).

To get just the executable without installers: `cargo tauri build --no-bundle`
(`target/release/ironcall`, or `ironcall.exe` on Windows).

## Signing

V1 installers are unsigned. On Windows, SmartScreen warns about unsigned installers
until they build reputation; choose **More info › Run anyway**. Code signing is planned
after V1.

## Versions

The app version follows [Semantic Versioning](https://semver.org) and lives in three
places that must match (a test checks this): `version` in the root `Cargo.toml`
(`[workspace.package]`), `src-tauri/tauri.conf.json` and `ui/package.json`. File schema
versions (`ironcall/request@1`, …) change independently and are listed in the
[CHANGELOG](../CHANGELOG.md).

To release (there is no release CI; everything runs on one Linux machine):

1. On `main`, set the version in the three files, add a CHANGELOG section, and commit.
2. `just check` and `just e2e`. To enforce the performance budgets too, build the release
   binary and run the E2E suite against it (the command is in PLAN.md › Phase 8).
3. `just build`. The packages land in `target/release/bundle/appimage/` and
   `target/release/bundle/deb/`.
4. Install the `.deb` and run the AppImage on a clean machine or VM, not the build box.
   Check that Settings › About shows the new version.
5. Checksums, from the repo root:
   `sha256sum target/release/bundle/appimage/*.AppImage target/release/bundle/deb/*.deb > SHA256SUMS`
   (the file is not committed).
6. Tag and push: `git tag -a vX.Y.Z -m "IronCall X.Y.Z" && git push origin main vX.Y.Z`.
7. Create the GitHub release from the tag, with the CHANGELOG section as the notes and
   the AppImage, the `.deb` and `SHA256SUMS` attached:
   `gh release create vX.Y.Z target/release/bundle/appimage/*.AppImage target/release/bundle/deb/*.deb SHA256SUMS --title "IronCall X.Y.Z" --notes-file notes.md`.
