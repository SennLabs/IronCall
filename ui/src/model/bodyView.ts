/**
 * How a response body is shown (PLAN.md Phase 5, ADR 0003).
 *
 * - Up to `SMALL_BODY_LIMIT`: the whole text in CodeMirror (pretty-printed by Rust via
 *   `getBodyText(pretty)` when the kind allows).
 * - Larger: the paged viewer. JSON/XML up to `AUTO_PRETTY_LIMIT` are pretty-printed by
 *   default into a derived body (`bodyPretty`); bigger ones default to raw with a
 *   "Pretty-print anyway" button.
 */

import type { BodyKind, ResponseBodyInfo } from "../ipc/bindings";

/** Must match `SMALL_BODY_LIMIT` in src-tauri/src/send.rs. */
export const SMALL_BODY_LIMIT = 2 * 1024 * 1024;
export const AUTO_PRETTY_LIMIT = 10 * 1024 * 1024;

export const canPretty = (kind: BodyKind) => kind === "json" || kind === "xml";

export type BodyMode =
  /** No body to show. */
  | { mode: "empty" }
  | { mode: "binary" }
  /** CodeMirror with the full text. */
  | { mode: "editor"; prettyAvailable: boolean }
  /** Paged viewer; `prettyByDefault` says whether to format into a derived body first. */
  | { mode: "paged"; prettyAvailable: boolean; prettyByDefault: boolean };

export function bodyMode(body: ResponseBodyInfo): BodyMode {
  if (body.bodyId === null || body.kind === "empty") return { mode: "empty" };
  if (body.kind === "binary") return { mode: "binary" };
  if (body.size <= SMALL_BODY_LIMIT) {
    return { mode: "editor", prettyAvailable: body.prettyAvailable };
  }
  const prettyAvailable = canPretty(body.kind);
  return {
    mode: "paged",
    prettyAvailable,
    prettyByDefault: prettyAvailable && body.size <= AUTO_PRETTY_LIMIT,
  };
}

export type PreviewKind = "image" | "pdf" | "html" | "media";

/** What the Preview tab can show for a body, or `null` if there's nothing to preview. */
export function previewKind(contentType: string | null, kind: BodyKind): PreviewKind | null {
  const type = (contentType ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
  if (type.startsWith("image/")) return "image";
  if (type === "application/pdf") return "pdf";
  if (type.startsWith("audio/") || type.startsWith("video/")) return "media";
  if (kind === "html" || type === "text/html" || type === "application/xhtml+xml") return "html";
  return null;
}

/**
 * `sandbox` for the HTML preview iframe. Empty (everything off) by default; scripts are
 * opt-in, and `allow-same-origin` is never added, so a script can't reach the app or its
 * own origin's storage (PLAN.md §9).
 */
export function previewSandbox(allowScripts: boolean): string {
  return allowScripts ? "allow-scripts" : "";
}

/** File name to suggest in "Save response…", from the URL and content type. */
export function suggestedFileName(url: string, contentType: string | null): string {
  let base = "";
  try {
    const path = new URL(url).pathname;
    base = decodeURIComponent(path.split("/").filter(Boolean).pop() ?? "");
  } catch {
    // Not an absolute URL (e.g. an unresolved variable): fall back below.
  }
  base = base.replace(/[\\/:*?"<>|]/g, "_");
  if (base.includes(".")) return base;
  const type = (contentType ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
  const ext =
    {
      "application/json": "json",
      "application/xml": "xml",
      "text/xml": "xml",
      "text/html": "html",
      "text/plain": "txt",
      "text/csv": "csv",
      "application/pdf": "pdf",
      "image/png": "png",
      "image/jpeg": "jpg",
      "image/gif": "gif",
      "image/svg+xml": "svg",
      "image/webp": "webp",
    }[type] ?? "bin";
  return `${base || "response"}.${ext}`;
}
