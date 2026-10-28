# 0006 — Folders and the secret backend are chosen in Rust, never taken from the webview or the workspace

- **Status:** Accepted
- **Date:** 2026-10-08
- **Phase:** P8 (security hardening before v1.0)

## Context
The review on 2026-10-06 left three open items (PLAN.md Phase 8). Each one let
something other than the user widen what IronCall reads or where it writes secrets:
1. `workspace_open` and `workspace_create` took a folder path over IPC and granted it in
   the path guard. Script running in the webview (which the CSP and DOMPurify are meant
   to prevent) could grant itself any folder.
2. The whole app data folder was a path-guard root, so body commands could read
   history, cookies and settings.
3. The secret backend was read from `.ironcall/local.yaml`. A repo can force-track that
   file (and commit a `.gitignore` that lets it through) to select the plaintext
   backend, so secrets a user typed in would land in the repo folder without the usual
   warning.

## Options considered
For 1: a Rust-side folder dialog whose results are the only paths accepted (chosen); or
keep JS picking and confirm each new folder in a native dialog (two dialogs per open).

For 3: keep the choice per machine in the app data folder (chosen); or keep reading
`local.yaml` and warn when a newly opened workspace already has a non-default backend
(the warning is easy to click through, and the file still decides).

## Decision
1. Folders are picked with `workspace_pick_folder`, a native dialog opened from Rust.
   The workspace commands accept only folders picked this session, and
   `workspace_open` also accepts folders on the recent list (written only by Rust), so
   session restore and the recent list keep working. The JS `dialog:allow-open`
   permission is removed.
2. Only the spool folder (`<app data>/spool`) is a path-guard root.
3. The backend choice is stored in `<app data>/secret-backends.json` (0600), keyed by
   the workspace's resolved folder. A `secretBackend` in `local.yaml` is ignored; if it
   names a backend other than the one in use, the UI shows a banner and a note in
   Secret storage so the user can switch to it. Switching removes the old key.

## Consequences
- Upgrading from 0.9.0 resets workspaces that used the encrypted or plaintext file to
  the keychain until the user switches back in Secret storage (the banner says so).
  The files are left as they are. Switching copies any values entered in the keychain
  meanwhile into the file, overwriting the same names there.
- Moving or renaming a workspace folder resets its backend choice to the keychain.
- E2E tests seed `recent-workspaces.json` and `secret-backends.json` instead of
  relying on a path given over IPC or a backend in `local.yaml`.
- Workspace CA certificates (`caCerts` in `local.yaml`) are still read from the
  workspace, by decision: a repo that commits one already chooses the URLs its
  requests go to.
