import { createRoot } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { setApi, tauriApi } from "../ipc/api";
import type { BodyHit } from "../ipc/bindings";
import { createMockApi, mockDraft } from "../ipc/mock";
import { createBodySearch } from "./bodySearch";

const TEXT = ["alpha", "needle one", "beta", "needle two", "gamma", "needle three"].join("\n");

async function setup(searchLatencyMs = 0) {
  const mock = createMockApi({ searchLatencyMs });
  setApi(mock);
  const r = await mock.sendRequest("s", mockDraft({ url: "https://x.test" }));
  mock.bodies.set(r.body.bodyId as number, TEXT);
  const hits: (BodyHit | undefined)[] = [];
  const search = createRoot(() =>
    createBodySearch({
      bodyId: r.body.bodyId as number,
      byteLen: TEXT.length,
      onHit: (h) => hits.push(h),
      debounceMs: 0,
    }),
  );
  return { mock, search, hits };
}

const tick = (ms = 5) => new Promise((r) => setTimeout(r, ms));
const lineOf = (h: BodyHit | undefined) => (h ? h.row + 1 : null);

describe("body search", () => {
  afterEach(() => setApi(tauriApi));

  it("finds as you type, then next/previous in both directions with wrap-around", async () => {
    const { search } = await setup();
    search.setQuery("needle");
    await tick();
    expect(lineOf(search.hit())).toBe(2);
    await search.next();
    expect(lineOf(search.hit())).toBe(4);
    await search.next();
    expect(lineOf(search.hit())).toBe(6);
    await search.next(); // wraps
    expect(lineOf(search.hit())).toBe(2);
    expect(search.message()).toContain("wrapped");
    await search.previous(); // wraps backwards to the last match
    expect(lineOf(search.hit())).toBe(6);
    await search.previous();
    expect(lineOf(search.hit())).toBe(4);
  });

  it("sends the direction to the backend", async () => {
    const { mock, search } = await setup();
    search.setQuery("needle");
    await tick();
    await search.previous();
    const directions = mock.bodyCalls.filter((c) => c.op === "searchBody").map((c) => c.args[4]);
    expect(directions).toContain("forward");
    expect(directions).toContain("backward");
  });

  it("ignores results from a search superseded by typing", async () => {
    const { mock, search, hits } = await setup(30);
    search.setQuery("nee");
    await tick(2); // the "nee" search is now in flight (30ms)
    search.setQuery("gamma");
    await tick(80);
    expect(lineOf(search.hit())).toBe(5);
    // Only the newest search's hit was ever reported.
    expect(hits.filter(Boolean).map(lineOf)).toEqual([5]);
    // The older one was cancelled by the newer one on the "backend".
    expect(mock.bodyCalls.filter((c) => c.op === "searchBody")).toHaveLength(2);
  });

  it("reports no matches and clears the hit", async () => {
    const { search } = await setup();
    search.setQuery("zzz");
    await tick();
    expect(search.hit()).toBeUndefined();
    expect(search.message()).toBe("No matches");
  });
});
