/**
 * Ranking for quick open (Ctrl+P, PLAN.md Phase 8): every query word must appear in a
 * request's name, folder path, method or URL. Name matches rank first, then prefixes,
 * then shorter names.
 */

export interface QuickOpenEntry {
  id: string;
  name: string;
  /** Folder breadcrumb or "Scratch". */
  where: string;
  method: string;
  url: string;
  scratch: boolean;
}

export function rankEntries(entries: readonly QuickOpenEntry[], query: string, limit = 50) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return entries.slice(0, limit);
  const scored: { entry: QuickOpenEntry; score: number }[] = [];
  for (const entry of entries) {
    const name = entry.name.toLowerCase();
    const hay = `${name} ${entry.where.toLowerCase()} ${entry.method.toLowerCase()} ${entry.url.toLowerCase()}`;
    if (!words.every((w) => hay.includes(w))) continue;
    let score = 0;
    for (const w of words) {
      if (name.startsWith(w)) score += 4;
      else if (name.split(/[\s/_-]+/).some((part) => part.startsWith(w))) score += 3;
      else if (name.includes(w)) score += 2;
    }
    scored.push({ entry, score: score * 1000 - name.length });
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.slice(0, limit).map((s) => s.entry);
}
