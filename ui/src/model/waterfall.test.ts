import { describe, expect, it } from "vitest";
import type { TimingsInfo } from "../ipc/bindings";
import { buildWaterfall, hopSpan, tickStep } from "./waterfall";

const timings = (over: Partial<TimingsInfo> = {}): TimingsInfo => ({
  dns: { kind: "measured", ms: 10 },
  connect: { kind: "measured", ms: 20 },
  tls: { kind: "measured", ms: 30 },
  ttfbMs: 30,
  downloadMs: 10,
  totalMs: 100,
  ...over,
});

describe("buildWaterfall", () => {
  it("lays phases end to end on the shared scale", () => {
    const w = buildWaterfall([], { status: 200, url: "u", timings: timings() }, 100);
    expect(w.scaleMs).toBe(100);
    const segs = w.rows[0]?.segments ?? [];
    const round = (x: number) => Math.round(x * 1000) / 1000;
    expect(segs.map((s) => [s.phase, round(s.start), round(s.width)])).toEqual([
      ["dns", 0, 0.1],
      ["connect", 0.1, 0.2],
      ["tls", 0.3, 0.3],
      ["ttfb", 0.6, 0.3],
      ["download", 0.9, 0.1],
    ]);
  });

  it("puts redirect hops before the final response", () => {
    const hop = {
      status: 302,
      url: "a",
      timings: timings({ ttfbMs: 0, downloadMs: 0, totalMs: 60 }),
    };
    const w = buildWaterfall(
      [hop],
      {
        status: 200,
        url: "b",
        timings: timings({
          dns: { kind: "reused" },
          connect: { kind: "reused" },
          tls: { kind: "reused" },
          ttfbMs: 30,
          downloadMs: 10,
          totalMs: 40,
        }),
      },
      100,
    );
    expect(w.rows.map((r) => r.label)).toEqual(["Redirect 1", "Response"]);
    expect(w.scaleMs).toBe(100);
    const final = w.rows[1]?.segments ?? [];
    // Reused phases take no space and start where the previous hop ended.
    expect(final[0]).toMatchObject({ state: "reused", width: 0, ms: null, start: 0.6 });
    expect(final[3]).toMatchObject({ phase: "ttfb", start: 0.6, width: 0.3 });
  });

  it("uses the send total when it's longer than the phases (e.g. queueing)", () => {
    const w = buildWaterfall([], { status: 200, url: "u", timings: timings() }, 200);
    expect(w.scaleMs).toBe(200);
    expect(w.rows[0]?.segments[4]?.start).toBe(0.45);
  });

  it("places 1xx markers relative to the final hop's wait", () => {
    const w = buildWaterfall([], { status: 200, url: "u", timings: timings() }, 100, [
      { status: 103, headers: [], atMs: 10 },
    ]);
    expect(w.markers).toEqual([{ at: 0.7, label: "103" }]);
  });

  it("treats missing durations as zero", () => {
    expect(hopSpan(timings({ ttfbMs: null, downloadMs: null }))).toBe(60);
  });
});

describe("tickStep", () => {
  it.each([
    [6, 1],
    [100, 20],
    [450, 100],
    [3000, 500],
  ])("%s ms → %s ms", (scale, step) => {
    expect(tickStep(scale)).toBe(step);
  });
});
