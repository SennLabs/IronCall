/** "Copy diagnostic info" and "Open logs folder" (PLAN.md Phase 8). */

import { api } from "../ipc/api";
import { attempt, notify } from "./notify";

export async function copyDiagnostics() {
  const text = await attempt(() => api.diagnosticInfo(), "Couldn't collect diagnostic info");
  if (text === undefined) return;
  try {
    await navigator.clipboard.writeText(text);
    notify("Diagnostic info copied. It contains no secrets.", "info");
  } catch {
    notify("Couldn't copy to the clipboard", "error");
  }
}

export function openLogs() {
  void attempt(() => api.openLogsFolder(), "Couldn't open the logs folder");
}

/** Sends uncaught errors to the backend log, so "Copy diagnostic info" includes them. */
export function reportUncaughtErrors(target: Window = window) {
  target.addEventListener("error", (e) => {
    void api.logFrontendError(
      String(e.message),
      e.error instanceof Error ? (e.error.stack ?? null) : null,
    );
  });
  target.addEventListener("unhandledrejection", (e) => {
    const reason = e.reason;
    void api.logFrontendError(
      reason instanceof Error ? reason.message : String(reason),
      reason instanceof Error ? (reason.stack ?? null) : null,
    );
  });
}
