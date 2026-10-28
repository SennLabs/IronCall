/**
 * Pure helpers for cURL paste/import and "Copy as cURL" (PLAN.md Phase 6).
 */

import type {
  CurlDialect,
  CurlExportMode,
  ImportWarning,
  PlanNode,
  RequestDraft,
} from "../ipc/api";
import { detectPlatform, type Platform } from "../ipc/scheme";

/** Name a new request gets; a tab still using it hasn't been named by the user. */
export const DEFAULT_REQUEST_NAME = "Untitled request";

/** Quick synchronous check, mirroring the backend's `curl_detect`. */
export function looksLikeCurl(text: string): boolean {
  return /^\s*curl(\.exe)?\s/i.test(text);
}

/** True if a draft has nothing the user typed (a fresh tab): pasting may replace it. */
export function isEmptyDraft(draft: RequestDraft): boolean {
  const hasRows = (rows?: { name: string; value?: string }[]) =>
    (rows ?? []).some((r) => r.name.trim() !== "" || (r.value ?? "").trim() !== "");
  const body = draft.body;
  // A body type chosen with nothing in it (e.g. JSON with empty content) still counts as empty.
  const hasBody =
    !!body &&
    body.type !== "none" &&
    !(typeof body.content === "string" && body.content.trim() === "");
  return (
    draft.url.trim() === "" &&
    (draft.method || "GET").toUpperCase() === "GET" &&
    !hasRows(draft.params) &&
    !hasRows(draft.headers) &&
    !hasBody &&
    !draft.auth
  );
}

/**
 * The parsed request placed into an existing tab: it keeps the tab's ID (and file),
 * and its name unless the tab still has the default name.
 */
export function adoptParsed(current: RequestDraft, parsed: RequestDraft): RequestDraft {
  const keepName = current.name.trim() !== "" && current.name !== DEFAULT_REQUEST_NAME;
  return {
    ...parsed,
    schema: current.schema,
    id: current.id,
    name: keepName ? current.name : parsed.name,
  };
}

/** "Imported from cURL; ignored: --cert, -o" (flags are listed once each). */
export function curlNoticeText(warnings: readonly ImportWarning[]): string {
  if (warnings.length === 0) return "Imported from cURL";
  const flags = [
    ...new Set(
      warnings.map((w) => w.location).filter((l): l is string => !!l && l.startsWith("-")),
    ),
  ];
  const others = warnings.filter((w) => !w.location?.startsWith("-")).map((w) => w.message);
  const parts = [...(flags.length ? [`ignored: ${flags.join(", ")}`] : []), ...others];
  return `Imported from cURL; ${parts.join("; ")}`;
}

/** Windows users get PowerShell (`curl.exe`); everyone else bash. */
export function defaultDialect(platform: Platform = detectPlatform()): CurlDialect {
  return platform === "windows" ? "powerShell" : "posix";
}

export const EXPORT_PREFS_KEY = "ironcall.curlExport";

export interface ExportPrefs {
  dialect: CurlDialect;
  mode: CurlExportMode;
}

/** Last used dialect and mode. "Resolved with secrets" is never remembered. */
export function loadExportPrefs(platform: Platform = detectPlatform()): ExportPrefs {
  const fallback: ExportPrefs = { dialect: defaultDialect(platform), mode: "masked" };
  try {
    const raw = localStorage.getItem(EXPORT_PREFS_KEY);
    if (!raw) return fallback;
    const p = JSON.parse(raw) as Partial<ExportPrefs>;
    return {
      dialect: p.dialect === "posix" || p.dialect === "powerShell" ? p.dialect : fallback.dialect,
      mode: p.mode === "variables" || p.mode === "masked" ? p.mode : "masked",
    };
  } catch {
    return fallback;
  }
}

export function saveExportPrefs(prefs: ExportPrefs) {
  try {
    const mode: CurlExportMode = prefs.mode === "resolved" ? "masked" : prefs.mode;
    localStorage.setItem(EXPORT_PREFS_KEY, JSON.stringify({ dialect: prefs.dialect, mode }));
  } catch {
    // Not persisted; the choice still applies now.
  }
}

/** Counts of an OpenAPI preview tree. */
export function countPlan(nodes: readonly PlanNode[]): { folders: number; requests: number } {
  let folders = 0;
  let requests = 0;
  const walk = (list: readonly PlanNode[]) => {
    for (const n of list) {
      if (n.method === null) {
        folders++;
        walk(n.children);
      } else requests++;
    }
  };
  walk(nodes);
  return { folders, requests };
}
