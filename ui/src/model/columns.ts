/**
 * Horizontal virtualisation for the paged viewer (ADR 0003 follow-up): very long rows
 * (minified JSON is split into ~1KB rows by Rust, but even those are wide) render only
 * the characters in view, padded so the horizontal scroll position stays right.
 *
 * Assumes a monospace font with a fixed character width.
 */

export interface ColumnWindow {
  /** First character index to render. */
  start: number;
  /** One past the last character index to render. */
  end: number;
  /** Left padding in px for the skipped characters. */
  padLeft: number;
}

/** Characters rendered beyond each side of the viewport, so short scrolls stay smooth. */
export const OVERSCAN_CHARS = 200;

/** Rows shorter than this are rendered whole (cheap, and keeps selection natural). */
export const WHOLE_ROW_CHARS = 2000;

export function columnWindow(
  length: number,
  charWidth: number,
  scrollLeft: number,
  viewportWidth: number,
  overscan = OVERSCAN_CHARS,
): ColumnWindow {
  if (length <= WHOLE_ROW_CHARS || charWidth <= 0) return { start: 0, end: length, padLeft: 0 };
  const firstVisible = Math.floor(Math.max(scrollLeft, 0) / charWidth);
  const visible = Math.ceil(Math.max(viewportWidth, 0) / charWidth) + 1;
  const start = Math.max(0, Math.min(firstVisible - overscan, length));
  const end = Math.min(length, firstVisible + visible + overscan);
  return { start, end: Math.max(start, end), padLeft: start * charWidth };
}

/** Width in px of the longest row in view, so the content gets a horizontal scrollbar. */
export function contentWidth(lengths: number[], charWidth: number, gutterPx: number): number {
  let max = 0;
  for (const l of lengths) if (l > max) max = l;
  return gutterPx + max * charWidth;
}
