/**
 * URL bar ↔ params table sync (PLAN.md §5).
 *
 * A request stores `url` without its query string, plus `params` rows. The URL bar
 * shows `url` followed by the enabled params. Nothing here encodes or decodes: text is
 * kept exactly as the user typed it, and the backend applies the encoding rules at send
 * time. That way editing one side never rewrites the other's characters.
 */

import type { KeyValueRow as KeyValue, PathParamRow as PathParam } from "../ipc/api";

export interface QueryPair {
  name: string;
  value: string;
}

/** Splits URL-bar text at the first `?` into the stored `url` and the raw query. */
export function splitUrl(text: string): { base: string; query: string | null } {
  const q = text.indexOf("?");
  return q < 0 ? { base: text, query: null } : { base: text.slice(0, q), query: text.slice(q + 1) };
}

/** Parses a raw query string into pairs, without decoding. Empty segments are skipped. */
export function parseQuery(query: string): QueryPair[] {
  return query
    .split("&")
    .filter((part) => part !== "")
    .map((part) => {
      const eq = part.indexOf("=");
      return eq < 0
        ? { name: part, value: "" }
        : { name: part.slice(0, eq), value: part.slice(eq + 1) };
    });
}

const isEnabled = (row: KeyValue) => row.enabled !== false;

/** Renders enabled rows as a query string. Rows with an empty value render as just `name`. */
export function formatQuery(params: readonly KeyValue[]): string {
  return params
    .filter((p) => isEnabled(p) && (p.name !== "" || (p.value ?? "") !== ""))
    .map((p) => ((p.value ?? "") === "" ? p.name : `${p.name}=${p.value}`))
    .join("&");
}

/** The text shown in the URL bar for a stored `url` and `params`. */
export function buildUrlText(url: string, params: readonly KeyValue[]): string {
  const query = formatQuery(params);
  return query ? `${url}?${query}` : url;
}

/**
 * Merges pairs typed in the URL bar into the existing rows.
 *
 * Disabled rows (not shown in the URL bar) stay where they are. Enabled rows are
 * replaced by the typed pairs in order; a row object is reused, unknown keys and all,
 * when its name matches, so editing a value doesn't lose data. Extra pairs are appended.
 */
export function mergeQuery(existing: readonly KeyValue[], pairs: readonly QueryPair[]): KeyValue[] {
  const enabled = existing.filter(isEnabled);
  const used = new Set<KeyValue>();

  const rowFor = (pair: QueryPair, slot: KeyValue | undefined): KeyValue => {
    const match =
      slot && slot.name === pair.name && !used.has(slot)
        ? slot
        : enabled.find((row) => row.name === pair.name && !used.has(row));
    if (match) {
      used.add(match);
      return match.value === pair.value ? match : { ...match, value: pair.value };
    }
    if (slot && !used.has(slot)) {
      // Same position, renamed: keep the row's other keys.
      used.add(slot);
      return { ...slot, name: pair.name, value: pair.value };
    }
    return { name: pair.name, value: pair.value, enabled: true };
  };

  // First pass: assign pairs to enabled slots by position.
  const assigned: (KeyValue | null)[] = enabled.map(() => null);
  const leftovers: QueryPair[] = [];
  pairs.forEach((pair, i) => {
    if (i < enabled.length) assigned[i] = rowFor(pair, enabled[i]);
    else leftovers.push(pair);
  });

  const result: KeyValue[] = [];
  let slot = 0;
  for (const row of existing) {
    if (!isEnabled(row)) {
      result.push(row);
      continue;
    }
    const replacement = assigned[slot++];
    if (replacement) result.push(replacement);
  }
  for (const pair of leftovers) result.push(rowFor(pair, undefined));
  return result;
}

/** Applies URL-bar text to a request's `url` and `params`. */
export function applyUrlText(
  text: string,
  params: readonly KeyValue[],
): { url: string; params: KeyValue[] } {
  const { base, query } = splitUrl(text);
  return { url: base, params: mergeQuery(params, query === null ? [] : parseQuery(query)) };
}

const NAME = "[A-Za-z_][A-Za-z0-9_-]*";
const COLON_PARAM = new RegExp(`(?:^|/):(${NAME})(?=/|$|#)`, "g");
// `{id}` but not `{{var}}`.
const BRACE_PARAM = new RegExp(`(?<!\\{)\\{(${NAME})\\}(?!\\})`, "g");

/** Strips `scheme://authority` so `host:8080` isn't read as a path param. */
function pathPart(url: string): string {
  const m = /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[^/]*/.exec(url);
  return m ? url.slice(m[0].length) : url;
}

/** Path parameter names in a stored `url`, in order, without duplicates. */
export function pathParamNames(url: string): string[] {
  const path = pathPart(splitUrl(url).base);
  const found: { name: string; at: number }[] = [];
  for (const re of [COLON_PARAM, BRACE_PARAM]) {
    for (const m of path.matchAll(re)) found.push({ name: m[1] as string, at: m.index ?? 0 });
  }
  found.sort((a, b) => a.at - b.at);
  return [...new Set(found.map((f) => f.name))];
}

/** Rows for the path params in `url`: existing rows are kept (with values), others added. */
export function syncPathParams(existing: readonly PathParam[], url: string): PathParam[] {
  const names = pathParamNames(url);
  const unchanged =
    names.length === existing.length && names.every((n, i) => existing[i]?.name === n);
  if (unchanged) return existing as PathParam[];
  return names.map((name) => existing.find((p) => p.name === name) ?? { name, value: "" });
}
