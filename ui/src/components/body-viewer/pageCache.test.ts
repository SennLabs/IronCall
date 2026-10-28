import { describe, expect, it, vi } from "vitest";
import { PageCache } from "./pageCache";

const rows = (from: number, count: number) =>
  Promise.resolve(Array.from({ length: count }, (_, i) => from + i));

describe("PageCache", () => {
  it("loads the pages covering a range and serves rows", async () => {
    const fetcher = vi.fn(rows);
    const cache = new PageCache(fetcher, 10);
    expect(cache.row(15)).toBeUndefined();
    await cache.ensure(5, 25);
    expect(fetcher).toHaveBeenCalledTimes(3);
    expect(cache.row(15)).toBe(15);
    expect(cache.row(29)).toBe(29);
  });

  it("shares in-flight fetches and skips cached pages", async () => {
    const fetcher = vi.fn(rows);
    const cache = new PageCache(fetcher, 10);
    await Promise.all([cache.ensure(0, 9), cache.ensure(0, 9)]);
    await cache.ensure(0, 9);
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("evicts the least recently used page", async () => {
    const cache = new PageCache(rows, 10, 2);
    await cache.ensure(0, 0); // page 0
    await cache.ensure(10, 10); // page 1
    await cache.ensure(0, 0); // touch page 0
    await cache.ensure(20, 20); // page 2 evicts page 1
    expect(cache.row(0)).toBe(0);
    expect(cache.row(10)).toBeUndefined();
    expect(cache.row(20)).toBe(20);
    expect(cache.cachedPages).toBe(2);
  });

  it("notifies when a page loads", async () => {
    const onLoad = vi.fn();
    const cache = new PageCache(rows, 10, 8, onLoad);
    await cache.ensure(0, 19);
    expect(onLoad).toHaveBeenCalledTimes(2);
  });
});

import { pageSizeFor } from "./LargeBodyViewer";

describe("pageSizeFor", () => {
  it("targets ~64KB pages within 20..200 rows", () => {
    expect(pageSizeFor(5_000_000, 100_000_000)).toBe(200); // 20B rows
    expect(pageSizeFor(100_000, 100_000_000)).toBe(65); // ~1KB rows
    expect(pageSizeFor(10, 100_000_000)).toBe(20);
    expect(pageSizeFor(0, 0)).toBe(200);
  });
});
