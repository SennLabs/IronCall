import { describe, expect, it } from "vitest";
import { columnWindow, contentWidth, OVERSCAN_CHARS, WHOLE_ROW_CHARS } from "./columns";

describe("columnWindow", () => {
  it("renders short rows whole", () => {
    expect(columnWindow(500, 7, 300, 800)).toEqual({ start: 0, end: 500, padLeft: 0 });
    expect(columnWindow(WHOLE_ROW_CHARS, 7, 9999, 800)).toMatchObject({ start: 0 });
  });

  it("renders only the visible characters of long rows, plus overscan", () => {
    // 10px characters, scrolled 5000px right, 800px wide: chars 500..581 are visible.
    const w = columnWindow(100_000, 10, 5000, 800);
    expect(w.start).toBe(500 - OVERSCAN_CHARS);
    expect(w.end).toBe(500 + 81 + OVERSCAN_CHARS);
    expect(w.padLeft).toBe(w.start * 10);
  });

  it("clamps at both ends", () => {
    expect(columnWindow(10_000, 10, 0, 800)).toMatchObject({ start: 0, padLeft: 0 });
    const end = columnWindow(10_000, 10, 1_000_000, 800);
    expect(end.end).toBe(10_000);
    expect(end.start).toBeLessThanOrEqual(end.end);
  });

  it("falls back to whole rows without a character width (no layout, e.g. jsdom)", () => {
    expect(columnWindow(50_000, 0, 100, 800)).toEqual({ start: 0, end: 50_000, padLeft: 0 });
  });
});

describe("contentWidth", () => {
  it("is the gutter plus the longest row", () => {
    expect(contentWidth([3, 10, 7], 8, 72)).toBe(72 + 80);
    expect(contentWidth([], 8, 72)).toBe(72);
  });
});
