/**
 * WCAG 2.x contrast ratios for the theme tokens (PLAN.md Phase 8 accessibility pass).
 * Used by `contrast.test.ts` to keep both themes at AA.
 */

/** Relative luminance of a `#rrggbb` colour. */
export function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1), 16);
  const channel = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return (
    0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255)
  );
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

/** Token values from one CSS block, e.g. the `:root { … }` rule. */
export function tokens(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})\s*;/g)) {
    out[m[1] as string] = (m[2] as string).toLowerCase();
  }
  return out;
}
