/**
 * Scroll maths for the large body viewer.
 *
 * Browsers cap element heights (WebKitGTK measured at 33,554,428px), and 100MB of
 * pretty-printed JSON is ~5M rows (~91M px at 18px per row). The scroll container is
 * therefore capped at MAX_SCROLL_PX and its scrollbar is *scaled*: one scrollbar pixel
 * can cover many rows. The viewer keeps the true position ("real top", in unscaled
 * pixels) itself and converts wheel/keyboard deltas, so scrolling speed stays normal.
 */

export const MAX_SCROLL_PX = 8_000_000;

export interface Geometry {
  rowCount: number;
  rowHeight: number;
  viewportHeight: number;
}

export function scrollHeight(rowCount: number, rowHeight: number): number {
  return Math.min(rowCount * rowHeight, MAX_SCROLL_PX);
}

function extents({ rowCount, rowHeight, viewportHeight }: Geometry) {
  const maxRealTop = Math.max(rowCount * rowHeight - viewportHeight, 0);
  const maxScrollTop = Math.max(scrollHeight(rowCount, rowHeight) - viewportHeight, 0);
  return { maxRealTop, maxScrollTop };
}

export function clampRealTop(g: Geometry, realTop: number): number {
  return Math.min(Math.max(realTop, 0), extents(g).maxRealTop);
}

/** Converts a (possibly scaled) scrollbar position to the true position. */
export function toRealTop(g: Geometry, scrollTop: number): number {
  const { maxRealTop, maxScrollTop } = extents(g);
  if (maxScrollTop === 0) return 0;
  const clamped = Math.min(Math.max(scrollTop, 0), maxScrollTop);
  return maxScrollTop === maxRealTop ? clamped : (clamped / maxScrollTop) * maxRealTop;
}

/** Converts a true position to the scrollbar position. */
export function toScrollTop(g: Geometry, realTop: number): number {
  const { maxRealTop, maxScrollTop } = extents(g);
  if (maxRealTop === 0) return 0;
  const clamped = clampRealTop(g, realTop);
  return maxScrollTop === maxRealTop ? clamped : (clamped / maxRealTop) * maxScrollTop;
}

export interface VisibleWindow {
  /** First (partly) visible row. */
  first: number;
  /** Pixel offset to apply to the first row (≤ 0). */
  offset: number;
  /** Rows to render. */
  count: number;
}

export function windowAt(g: Geometry, realTop: number): VisibleWindow {
  const { rowCount, rowHeight, viewportHeight } = g;
  const top = clampRealTop(g, realTop);
  const first = Math.min(Math.floor(top / rowHeight), Math.max(rowCount - 1, 0));
  const count = Math.min(Math.ceil(viewportHeight / rowHeight) + 1, rowCount - first);
  return { first, offset: -(top - first * rowHeight), count: Math.max(count, 0) };
}

/** True position that puts `row` at the top of the viewport. */
export function realTopForRow(g: Geometry, row: number): number {
  return clampRealTop(g, row * g.rowHeight);
}
