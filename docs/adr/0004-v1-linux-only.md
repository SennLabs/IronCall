# 0004 — V1 ships for Linux; Windows support moves after V1

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** P8, affects the V1 scope

## Context
PLAN.md §2 targeted Windows (WebView2) and Linux (WebKitGTK) for V1, with every phase's
exit criteria checked on both. Phases 0–8 were built and verified on Linux. The Windows
checks (E2E with `msedgedriver`, the real keychain, proxies, trash and CRLF behaviour,
user-only ACLs, installers, and performance on Windows reference hardware) were
pending, and need a Windows machine to run.

## Options considered
1. **Hold V1 until the Windows checks pass.** Keeps the original scope; V1 waits on
   access to Windows hardware and any fixes it turns up.
2. **Ship V1 for Linux; Windows after V1 (chosen).** V1 is usable now. Windows becomes a
   tracked post-V1 item, with the code already written for it kept.

## Decision
V1 targets Linux only. Windows support, including its verification, installers and
performance budgets, moves to PLAN.md §10.

The code stays cross-platform: nothing Windows-specific is removed (path rules, user-only
ACLs via `icacls`, the keychain size-limit chunking, the E2E runner's `msedgedriver`
handling, `just check-windows` and `just test-wine`). New code should keep compiling for
Windows, so `just check-windows` stays a cheap guard.

## Consequences
- V1 exit criteria, the Definition of Done and the reference hardware are Linux only.
- The docs say Windows isn't supported in 1.0; the Windows build instructions stay, marked
  as a preview.
- Post-V1, Windows support starts with the checks listed in §10: they were written down
  per phase, and the cross-compile and Wine runs have already caught some issues.
