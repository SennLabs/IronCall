import { describe, expect, it } from "vitest";
import {
  clampRealTop,
  type Geometry,
  MAX_SCROLL_PX,
  realTopForRow,
  scrollHeight,
  toRealTop,
  toScrollTop,
  windowAt,
} from "./viewport";

const small: Geometry = { rowCount: 100, rowHeight: 18, viewportHeight: 180 };
const huge: Geometry = { rowCount: 5_600_000, rowHeight: 18, viewportHeight: 600 };

describe("viewport", () => {
  it("is unscaled for small bodies", () => {
    expect(scrollHeight(100, 18)).toBe(1800);
    expect(toRealTop(small, 40)).toBe(40);
    expect(toScrollTop(small, 40)).toBe(40);
    expect(windowAt(small, 36)).toEqual({ first: 2, offset: -0, count: 11 });
    expect(windowAt(small, 40).offset).toBe(-4);
  });

  it("caps the scroll height and scales positions for huge bodies", () => {
    expect(scrollHeight(huge.rowCount, 18)).toBe(MAX_SCROLL_PX);
    const maxScroll = MAX_SCROLL_PX - 600;
    const bottom = windowAt(huge, toRealTop(huge, maxScroll));
    expect(bottom.first + bottom.count).toBeGreaterThanOrEqual(huge.rowCount - 1);
    const middle = windowAt(huge, toRealTop(huge, maxScroll / 2));
    expect(Math.abs(middle.first - huge.rowCount / 2)).toBeLessThan(100);
  });

  it("round-trips between real and scrollbar positions", () => {
    for (const g of [small, huge]) {
      const real = realTopForRow(g, Math.floor(g.rowCount / 3));
      expect(toRealTop(g, toScrollTop(g, real))).toBeCloseTo(real, 0);
      expect(windowAt(g, real).first).toBe(Math.floor(g.rowCount / 3));
    }
  });

  it("clamps and handles empty or tiny bodies", () => {
    expect(clampRealTop(huge, -5)).toBe(0);
    expect(clampRealTop(huge, 1e12)).toBe(huge.rowCount * 18 - 600);
    const empty = { rowCount: 0, rowHeight: 18, viewportHeight: 600 };
    expect(windowAt(empty, 0)).toEqual({ first: 0, offset: -0, count: 0 });
    const tiny = { rowCount: 3, rowHeight: 18, viewportHeight: 600 };
    expect(windowAt(tiny, 50).count).toBe(3);
    expect(toScrollTop(tiny, 30)).toBe(0);
  });
});
