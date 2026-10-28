/** Formatting for the per-send timeline / console (PLAN.md Phase 5). */

import type { TimelineEntry, TimelineKind } from "../ipc/bindings";

/** Prefix like `curl -v`: `*` info, `>` sent, `<` received, `!` errors. */
export const KIND_PREFIX: Record<TimelineKind, string> = {
  info: "*",
  tls: "*",
  proxy: "*",
  requestHeaders: ">",
  requestBody: ">",
  responseHeaders: "<",
  responseBody: "<",
  redirect: "*",
  error: "!",
};

export const KIND_LABEL: Record<TimelineKind, string> = {
  info: "Info",
  tls: "TLS",
  proxy: "Proxy",
  requestHeaders: "Request",
  requestBody: "Request body",
  responseHeaders: "Response",
  responseBody: "Response body",
  redirect: "Redirect",
  error: "Error",
};

/** True for entries shown as a collapsible block (header lists). */
export const isBlock = (e: TimelineEntry) =>
  (e.kind === "requestHeaders" || e.kind === "responseHeaders") && e.text.includes("\n");

export function formatAt(ms: number | null): string {
  if (ms === null || Number.isNaN(ms)) return "      ";
  if (ms < 1000) return `${ms.toFixed(1).padStart(6)}ms`.slice(-8);
  return `${(ms / 1000).toFixed(2).padStart(6)}s`;
}

/** First line of an entry (the summary shown when a block is collapsed). */
export const firstLine = (e: TimelineEntry) => e.text.split("\n")[0] ?? "";

/** Plain-text copy of the whole timeline, `curl -v` style. */
export function timelineText(entries: TimelineEntry[]): string {
  return entries
    .map((e) => {
      const prefix = KIND_PREFIX[e.kind];
      const at = formatAt(e.atMs).trim();
      return e.text
        .split("\n")
        .map((line, i) =>
          i === 0 ? `[${at}] ${prefix} ${line}` : `${" ".repeat(at.length + 3)}${prefix} ${line}`,
        )
        .join("\n");
    })
    .join("\n");
}
