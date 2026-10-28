/**
 * Auth helpers (PLAN.md Phase 4): switching types without losing unknown keys,
 * describing an auth in one line, and spotting literal secrets.
 */

import type { AuthDraft, AuthOrigin, AuthType } from "../ipc/networkTypes";

export const AUTH_TYPES: readonly AuthType[] = ["inherit", "none", "basic", "bearer", "apiKey"];

export const AUTH_LABELS: Record<AuthType, string> = {
  inherit: "Inherit",
  none: "No auth",
  basic: "Basic",
  bearer: "Bearer token",
  apiKey: "API key",
};

/** The fields each type owns; other keys are unknown and always kept. */
const TYPE_FIELDS: Record<AuthType, readonly string[]> = {
  inherit: [],
  none: [],
  basic: ["username", "password"],
  bearer: ["token"],
  apiKey: ["key", "value", "in"],
};
const ALL_FIELDS = new Set(Object.values(TYPE_FIELDS).flat());

export function authOf(auth: AuthDraft | undefined): AuthDraft {
  return auth ?? { type: "inherit" };
}

/**
 * Switches `auth` to `type`. Unknown keys survive; fields of the old type are dropped,
 * since the backend would reject them for the new type.
 */
export function withAuthType(auth: AuthDraft | undefined, type: AuthType): AuthDraft {
  const current = authOf(auth);
  if (current.type === type) return current;
  const unknown: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(current)) {
    if (k !== "type" && !ALL_FIELDS.has(k)) unknown[k] = v;
  }
  const fresh: Record<AuthType, AuthDraft> = {
    inherit: { type: "inherit" },
    none: { type: "none" },
    basic: { type: "basic", username: "", password: "" },
    bearer: { type: "bearer", token: "" },
    apiKey: { type: "apiKey", key: "", value: "", in: "header" },
  };
  return { ...unknown, ...fresh[type] } as AuthDraft;
}

/** Updates fields of the current auth by spreading (unknown keys survive). */
export function patchAuth(auth: AuthDraft | undefined, patch: Record<string, unknown>): AuthDraft {
  return { ...authOf(auth), ...patch } as AuthDraft;
}

/** One-line summary, e.g. `Bearer {{token}}` or `API key X-Api-Key (header)`. */
export function describeAuth(auth: AuthDraft): string {
  switch (auth.type) {
    case "inherit":
      return "Inherit";
    case "none":
      return "No auth";
    case "basic":
      return `Basic ${auth.username || "(no username)"}`;
    case "bearer":
      return `Bearer ${auth.token || "(no token)"}`;
    case "apiKey":
      return `API key ${auth.key || "(no name)"} (${auth.in === "query" ? "query" : "header"})`;
  }
}

export function describeOrigin(origin: AuthOrigin): string {
  switch (origin.kind) {
    case "request":
      return "this request";
    case "folder":
      return `Folder › ${origin.name}`;
    case "collection":
      return "the collection";
    case "none":
      return "nothing (no auth is set above this request)";
  }
}

/**
 * True if `value` looks like a pasted credential rather than a `{{variable}}`: long,
 * no spaces, mostly token characters, and no variable reference. Used to suggest
 * moving it into a secret variable.
 */
export function looksLikeLiteralSecret(value: string | undefined): boolean {
  const v = (value ?? "").trim();
  if (v.length < 16 || v.includes("{{") || /\s/.test(v)) return false;
  return /^[A-Za-z0-9._~+/=-]+$/.test(v) && /\d/.test(v) && /[A-Za-z]/.test(v);
}
