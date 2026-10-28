/**
 * Pure helpers for `{{variables}}` (PLAN.md §5): finding references, highlighting,
 * autocomplete, hover text, and converting variable maps to editable rows.
 *
 * Resolution itself happens in the backend (`variables_for`); these functions only use
 * the `VariableContext` it returns.
 */

import type { RequestDraft } from "../ipc/api";
import type {
  DynamicHelper,
  SecretScope,
  VariableContext,
  VariableDraft,
  VarSource,
  VarsDraft,
  VisibleVar,
} from "../ipc/varsTypes";

/** Matches `{{name}}` and `{{$helper}}` (whitespace inside the braces allowed). */
const REF = /\{\{\s*(\$?[\w.-]+)\s*\}\}/g;

export interface VarRef {
  name: string;
  /** Offsets of the whole `{{…}}`, end exclusive. */
  from: number;
  to: number;
  helper: boolean;
}

export function findRefs(text: string): VarRef[] {
  const refs: VarRef[] = [];
  for (const m of text.matchAll(REF)) {
    const name = m[1] ?? "";
    refs.push({ name, from: m.index, to: m.index + m[0].length, helper: name.startsWith("$") });
  }
  return refs;
}

export type RefState = "defined" | "helper" | "unresolved" | "missing" | "unknown";

/**
 * How a reference should look. With no context yet (still loading) everything is
 * `unknown`, so nothing flashes red while typing.
 */
export function refState(name: string, ctx: VariableContext | undefined): RefState {
  if (!ctx) return name.startsWith("$") ? "helper" : "unknown";
  if (name.startsWith("$")) {
    return ctx.helpers.some((h) => h.name === name) ? "helper" : "unresolved";
  }
  if (ctx.unresolved.includes(name)) return "unresolved";
  const v = ctx.variables.find((x) => x.name === name);
  if (!v) return "unresolved";
  return v.missing ? "missing" : "defined";
}

export interface Segment {
  text: string;
  state: RefState | "plain";
  name?: string;
}

/** Splits text into plain runs and references, for overlay highlighting. */
export function segments(text: string, ctx: VariableContext | undefined): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const r of findRefs(text)) {
    if (r.from > at) out.push({ text: text.slice(at, r.from), state: "plain" });
    out.push({ text: text.slice(r.from, r.to), state: refState(r.name, ctx), name: r.name });
    at = r.to;
  }
  if (at < text.length) out.push({ text: text.slice(at), state: "plain" });
  return out;
}

/** The reference covering `offset`, if any. */
export function refAt(text: string, offset: number): VarRef | undefined {
  return findRefs(text).find((r) => offset >= r.from && offset < r.to);
}

// ---------------------------------------------------------------------------------
// Autocomplete

/** If the caret is inside an unfinished `{{…`, the partial name and where it starts. */
export function completionQuery(
  text: string,
  caret: number,
): { query: string; start: number } | undefined {
  const before = text.slice(0, caret);
  const m = /\{\{\s*(\$?[\w.-]*)$/.exec(before);
  if (!m) return undefined;
  const query = m[1] ?? "";
  return { query, start: caret - query.length };
}

export interface Completion {
  name: string;
  /** Value preview (masked for secrets). */
  detail: string;
  /** Source label, e.g. "Environment › Dev" or "helper". */
  source: string;
  helper: boolean;
}

export const MASK = "••••";

export function valuePreview(v: VisibleVar): string {
  if (v.secret) return v.missing ? "secret (not set)" : MASK;
  if (v.error) return `⚠ ${v.error}`;
  return v.value ?? v.raw ?? "";
}

export function completions(ctx: VariableContext | undefined, query: string): Completion[] {
  if (!ctx) return [];
  const q = query.toLowerCase();
  const vars: Completion[] = ctx.variables.map((v) => ({
    name: v.name,
    detail: valuePreview(v),
    source: sourceLabel(v.source),
    helper: false,
  }));
  const helpers: Completion[] = ctx.helpers.map((h: DynamicHelper) => ({
    name: h.name,
    detail: h.example,
    source: "helper",
    helper: true,
  }));
  const all = q.startsWith("$") ? helpers : [...vars, ...helpers];
  const matches = all.filter((c) => c.name.toLowerCase().includes(q.replace(/^\$/, "")));
  // Prefix matches first, variables before helpers, then alphabetical.
  return matches.sort((a, b) => {
    const pa = a.name.toLowerCase().startsWith(q) ? 0 : 1;
    const pb = b.name.toLowerCase().startsWith(q) ? 0 : 1;
    return pa - pb || Number(a.helper) - Number(b.helper) || a.name.localeCompare(b.name);
  });
}

/**
 * Inserts a completion at `start` (where the partial name begins) up to `caret`, closing
 * the braces unless they already follow. Returns the new text and caret.
 */
export function applyCompletion(
  text: string,
  start: number,
  caret: number,
  name: string,
): { text: string; caret: number } {
  const after = text.slice(caret);
  const close = /^\s*\}\}/.test(after) ? "" : "}}";
  const next = text.slice(0, start) + name + close + after;
  const end = start + name.length + close.length;
  // If braces already followed, put the caret after them.
  const existing = close ? 0 : (/^\s*\}\}/.exec(after)?.[0].length ?? 0);
  return { text: next, caret: end + existing };
}

// ---------------------------------------------------------------------------------
// Hover and labels

export function sourceLabel(source: VarSource): string {
  switch (source.scope) {
    case "global":
      return "Global";
    case "collection":
      return "Collection";
    case "environment":
      return `Environment › ${source.name}`;
    case "folder":
      return `Folder › ${source.name}`;
    case "request":
      return "Request";
    case "runtime":
      return "Runtime";
    case "dynamic":
      return "Helper";
  }
}

/** Where a variable's secret value is stored, for reveal/set/clear. */
export function secretScopeOf(source: VarSource): SecretScope | undefined {
  switch (source.scope) {
    case "global":
      return { kind: "global" };
    case "collection":
      return { kind: "collection" };
    case "environment":
      return { kind: "environment", id: source.id };
    case "folder":
      return { kind: "folder", id: source.id };
    default:
      return undefined;
  }
}

export interface HoverInfo {
  title: string;
  value: string;
  source: string;
  overrides: string[];
  problem?: string;
}

export function hoverInfo(name: string, ctx: VariableContext | undefined): HoverInfo {
  if (name.startsWith("$")) {
    const h = ctx?.helpers.find((x) => x.name === name);
    return h
      ? { title: `{{${name}}}`, value: `e.g. ${h.example}`, source: h.description, overrides: [] }
      : {
          title: `{{${name}}}`,
          value: "",
          source: "Helper",
          overrides: [],
          problem: "Unknown helper",
        };
  }
  const v = ctx?.variables.find((x) => x.name === name);
  if (!v) {
    return {
      title: `{{${name}}}`,
      value: "",
      source: "",
      overrides: [],
      problem: ctx ? "Not defined in any scope" : "Loading…",
    };
  }
  return {
    title: `{{${name}}}`,
    value: valuePreview(v),
    source: sourceLabel(v.source),
    overrides: v.overrides.map(sourceLabel),
    problem: v.missing ? "Secret has no value on this machine" : (v.error ?? undefined),
  };
}

// ---------------------------------------------------------------------------------
// Requests

/** All text in a request where variables can appear. */
export function requestTexts(draft: RequestDraft): string[] {
  const texts = [draft.url];
  for (const p of draft.params ?? []) if (p.enabled !== false) texts.push(p.name, p.value ?? "");
  for (const p of draft.pathParams ?? []) texts.push(p.value ?? "");
  for (const h of draft.headers ?? []) if (h.enabled !== false) texts.push(h.name, h.value ?? "");
  const body = draft.body;
  if (body && typeof body.content === "string") texts.push(body.content);
  return texts;
}

/** Names of secrets with no value that the request uses (directly). */
export function missingSecretsUsed(
  draft: RequestDraft,
  ctx: VariableContext | undefined,
): string[] {
  if (!ctx) return [];
  const used = new Set(requestTexts(draft).flatMap((t) => findRefs(t).map((r) => r.name)));
  return ctx.variables.filter((v) => v.missing && used.has(v.name)).map((v) => v.name);
}

/** Names the request uses that don't resolve, combining the backend's list with a local check. */
export function unresolvedUsed(draft: RequestDraft, ctx: VariableContext | undefined): string[] {
  if (!ctx) return [];
  const names = new Set<string>(ctx.unresolved);
  for (const t of requestTexts(draft)) {
    for (const r of findRefs(t)) {
      if (refState(r.name, ctx) === "unresolved") names.add(r.name);
    }
  }
  return [...names].sort();
}

// ---------------------------------------------------------------------------------
// Editable rows

/**
 * One variable as the tables edit it. `extra` keeps unknown keys of the detailed form.
 * `pendingSecret` holds a secret value typed in the UI but not yet stored; it never
 * goes into the file.
 */
export interface VarRow {
  name: string;
  value: string;
  secret: boolean;
  description: string;
  /** Name when loaded, so a renamed secret's value can be moved. */
  originalName?: string;
  /** Was this a secret when loaded? */
  originalSecret?: boolean;
  pendingSecret?: string;
  extra: Record<string, unknown>;
}

export function varsToRows(vars: VarsDraft | undefined): VarRow[] {
  return Object.entries(vars ?? {}).map(([name, v]) => {
    if (typeof v === "string") {
      return {
        name,
        value: v,
        secret: false,
        description: "",
        originalName: name,
        originalSecret: false,
        extra: {},
      };
    }
    const { value, secret, description, ...extra } = v;
    return {
      name,
      value: secret ? "" : (value ?? ""),
      secret: !!secret,
      description: description ?? "",
      originalName: name,
      originalSecret: !!secret,
      extra,
    };
  });
}

/**
 * Rows → the map saved to the file. Secret rows never carry a value (PLAN.md §6).
 * Rows without a name are dropped; later duplicates win, as in a YAML map.
 */
export function rowsToVars(rows: readonly VarRow[]): VarsDraft {
  const out: VarsDraft = {};
  for (const r of rows) {
    const name = r.name.trim();
    if (!name) continue;
    let v: VariableDraft;
    if (r.secret) {
      v = { ...r.extra, secret: true, ...(r.description ? { description: r.description } : {}) };
    } else if (r.description || Object.keys(r.extra).length > 0) {
      v = { ...r.extra, value: r.value, ...(r.description ? { description: r.description } : {}) };
    } else {
      v = r.value;
    }
    out[name] = v;
  }
  return out;
}

/** Valid variable names: what `{{name}}` can reference. */
export function isValidVarName(name: string): boolean {
  return /^[\w.-]+$/.test(name) && !name.startsWith("$");
}
