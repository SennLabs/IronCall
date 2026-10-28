/**
 * Assertions and extractions as edited in the Tests tab (PLAN.md Phase 7). Evaluation
 * happens in Rust (`ironcall-core`); this module only describes the choices and turns
 * typed text into the typed values the files store.
 */

import type {
  AssertionDraft,
  AssertOp,
  AssertTarget,
  ExtractionDraft,
  ExtractSource,
} from "../ipc/api";

export const ASSERT_TARGETS: readonly { key: AssertTarget; label: string }[] = [
  { key: "status", label: "Status" },
  { key: "header", label: "Header" },
  { key: "body", label: "JSON body" },
  { key: "bodyText", label: "Body text" },
  { key: "duration", label: "Time (ms)" },
  { key: "size", label: "Size (B)" },
];

export const ASSERT_OPS: readonly { key: AssertOp; label: string }[] = [
  { key: "eq", label: "equals" },
  { key: "neq", label: "not equal" },
  { key: "lt", label: "<" },
  { key: "lte", label: "≤" },
  { key: "gt", label: ">" },
  { key: "gte", label: "≥" },
  { key: "contains", label: "contains" },
  { key: "matches", label: "matches" },
  { key: "exists", label: "exists" },
  { key: "notExists", label: "doesn't exist" },
  { key: "type", label: "type is" },
  { key: "in", label: "is one of" },
];

export const VALUE_TYPES = ["string", "number", "integer", "boolean", "array", "object", "null"];

export const EXTRACT_SOURCES: readonly { key: ExtractSource; label: string }[] = [
  { key: "body", label: "JSON body" },
  { key: "header", label: "Header" },
  { key: "cookie", label: "Cookie" },
  { key: "regex", label: "Body regex" },
];

/** Whether an operator compares against an expected value. */
export const needsValue = (op: AssertOp) => op !== "exists" && op !== "notExists";

/** Which extra field a target needs: the JSONPath, the header name, or none. */
export function assertionField(target: AssertTarget): "path" | "name" | null {
  if (target === "body") return "path";
  if (target === "header") return "name";
  return null;
}

/** Which field an extraction source reads. */
export function extractionField(from: ExtractSource): "path" | "name" | "pattern" {
  if (from === "body") return "path";
  if (from === "regex") return "pattern";
  return "name";
}

export const FIELD_PLACEHOLDER = {
  path: "$.data.id",
  name: "Header or cookie name",
  pattern: "id=(\\d+)",
} as const;

/**
 * Typed text → stored value. Valid JSON keeps its type (`201`, `true`, `"201"`,
 * `[200, 201]`); anything else is a string, so `application/json` needs no quotes.
 * `is one of` also accepts a comma-separated list.
 */
export function parseExpected(text: string, op: AssertOp): unknown {
  const t = text.trim();
  if (op === "in" && !t.startsWith("[")) {
    return t === "" ? [] : t.split(",").map((part) => parseScalar(part.trim()));
  }
  if (op === "matches" || op === "type") return t;
  return parseScalar(t);
}

function parseScalar(t: string): unknown {
  if (t === "") return "";
  try {
    return JSON.parse(t);
  } catch {
    return t;
  }
}

/**
 * Stored value → text for the input. Strings that would parse as something else are
 * quoted, except for regexes and type names, which are always text.
 */
export function formatExpected(value: unknown, op?: AssertOp): string {
  if (value === undefined) return "";
  if (typeof value === "string") {
    if (op === "matches" || op === "type") return value;
    return typeof parseScalar(value) === "string" ? value : JSON.stringify(value);
  }
  return JSON.stringify(value);
}

export function newAssertion(): AssertionDraft {
  return { target: "status", op: "eq", value: 200, enabled: true };
}

export function newExtraction(): ExtractionDraft {
  return { var: "", from: "body", path: "$.", sensitive: false };
}

/** Changing the operator drops or adds the expected value as needed. */
export function withOp(a: AssertionDraft, op: AssertOp): AssertionDraft {
  if (!needsValue(op)) {
    const { value: _, ...rest } = a;
    return { ...rest, op };
  }
  return { ...a, op, value: a.value ?? "" };
}

/** Changing the target keeps only the field it uses. */
export function withTarget(a: AssertionDraft, target: AssertTarget): AssertionDraft {
  const { path: _p, name: _n, ...rest } = a;
  const field = assertionField(target);
  return {
    ...rest,
    target,
    ...(field === "path" ? { path: a.path ?? "$." } : {}),
    ...(field === "name" ? { name: a.name ?? "" } : {}),
  };
}

export function withSource(e: ExtractionDraft, from: ExtractSource): ExtractionDraft {
  const { path: _p, name: _n, pattern: _r, ...rest } = e;
  const field = extractionField(from);
  return { ...rest, from, [field]: e[field] ?? (field === "path" ? "$." : "") };
}

// Mirrors `ironcall_core::redact` and `extract::is_sensitive` (PLAN.md §6.2).
const SENSITIVE_HEADERS = [
  "authorization",
  "proxy-authorization",
  "cookie",
  "set-cookie",
  "x-api-key",
];
const SENSITIVE_NAME_PARTS = [
  "authorization",
  "cookie",
  "password",
  "passwd",
  "secret",
  "token",
  "api_key",
  "apikey",
  "api-key",
];

export const isSensitiveName = (name: string) =>
  SENSITIVE_NAME_PARTS.some((part) => name.toLowerCase().includes(part));

/** Extractions that are always sensitive, whatever the checkbox says. */
export function alwaysSensitive(e: ExtractionDraft): boolean {
  return (
    e.from === "cookie" ||
    isSensitiveName(e.var) ||
    (e.from === "header" && SENSITIVE_HEADERS.includes((e.name ?? "").trim().toLowerCase())) ||
    (e.from === "body" && isSensitiveName(e.path ?? ""))
  );
}
