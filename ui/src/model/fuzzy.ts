/**
 * Fuzzy matching for the sidebar filter: every query character must appear in order.
 * Consecutive runs and word starts score higher, so "getusr" ranks "Get user" first.
 */

export interface FuzzyMatch {
  score: number;
  /** Indices of matched characters in the target, for highlighting. */
  indices: number[];
}

const isWordStart = (text: string, i: number) => {
  if (i === 0) return true;
  const prev = text[i - 1] ?? "";
  const cur = text[i] ?? "";
  return /[\s/_\-.:{}]/.test(prev) || (prev === prev.toLowerCase() && cur !== cur.toLowerCase());
};

/** Returns a match, or `null` if `query` isn't a subsequence of `target` (ignoring case). */
export function fuzzyMatch(query: string, target: string): FuzzyMatch | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return { score: 0, indices: [] };
  const t = target.toLowerCase();
  const indices: number[] = [];
  let score = 0;
  let ti = 0;
  let run = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    run = found === ti && indices.length > 0 ? run + 1 : 0;
    score += 1 + run * 2 + (isWordStart(target, found) ? 3 : 0) - Math.min(found - ti, 5) * 0.1;
    indices.push(found);
    ti = found + 1;
  }
  // Prefer shorter targets for equal matches.
  return { score: score - target.length * 0.01, indices };
}
