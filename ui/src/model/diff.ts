/**
 * Line diff for the conflict "Compare" view (LCS-based; files are small).
 */

export type DiffLine =
  | { op: "same"; text: string; left: number; right: number }
  | { op: "removed"; text: string; left: number }
  | { op: "added"; text: string; right: number };

/** Diffs two texts line by line. Line numbers are 1-based. */
export function diffLines(a: string, b: string): DiffLine[] {
  const left = a.split("\n");
  const right = b.split("\n");
  const n = left.length;
  const m = right.length;
  // lcs[i][j] = length of the LCS of left[i..] and right[j..].
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    const row = lcs[i] as Uint32Array;
    const below = lcs[i + 1] as Uint32Array;
    for (let j = m - 1; j >= 0; j--) {
      row[j] =
        left[i] === right[j]
          ? (below[j + 1] as number) + 1
          : Math.max(below[j] as number, row[j + 1] as number);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && left[i] === right[j]) {
      out.push({ op: "same", text: left[i] as string, left: i + 1, right: j + 1 });
      i++;
      j++;
    } else if (j < m && (i >= n || (lcs[i]?.[j + 1] ?? 0) >= (lcs[i + 1]?.[j] ?? 0))) {
      out.push({ op: "added", text: right[j] as string, right: j + 1 });
      j++;
    } else {
      out.push({ op: "removed", text: left[i] as string, left: i + 1 });
      i++;
    }
  }
  return out;
}

/** True if the diff has any changes. */
export const hasChanges = (d: DiffLine[]) => d.some((l) => l.op !== "same");

/**
 * Renders a request (or any JSON value) as indented YAML-like text, for comparing.
 * Only for display: key order is kept, strings are quoted only when needed.
 */
export function toDisplayText(value: unknown, indent = 0): string {
  const pad = "  ".repeat(indent);
  const scalar = (v: unknown): string => {
    if (typeof v === "string") {
      return v === "" || /[:#\n{}[\],&*?|<>=!%@`"']|^\s|\s$/.test(v) ? JSON.stringify(v) : v;
    }
    return String(v);
  };
  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    return value
      .map((item) => {
        if (item !== null && typeof item === "object") {
          const inner = toDisplayText(item, indent + 1).trimStart();
          return `${pad}- ${inner}`;
        }
        return `${pad}- ${scalar(item)}`;
      })
      .join("\n");
  }
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value).filter(([, v]) => v !== undefined && v !== null);
    if (entries.length === 0) return "{}";
    return entries
      .map(([k, v]) => {
        if (typeof v === "string" && v.includes("\n")) {
          const body = v
            .replace(/\n$/, "")
            .split("\n")
            .map((l) => `${pad}  ${l}`)
            .join("\n");
          return `${pad}${k}: |\n${body}`;
        }
        if (v !== null && typeof v === "object") {
          const empty = Array.isArray(v) ? v.length === 0 : Object.keys(v).length === 0;
          return empty
            ? `${pad}${k}: ${Array.isArray(v) ? "[]" : "{}"}`
            : `${pad}${k}:\n${toDisplayText(v, indent + 1)}`;
        }
        return `${pad}${k}: ${scalar(v)}`;
      })
      .join("\n");
  }
  return pad + scalar(value);
}
