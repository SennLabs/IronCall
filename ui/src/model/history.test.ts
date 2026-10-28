import { describe, expect, it } from "vitest";
import { createHistory } from "./history";

describe("history", () => {
  const clock = () => {
    let t = 0;
    return { now: () => t, advance: (ms: number) => (t += ms) };
  };

  it("undoes and redoes steps", () => {
    const c = clock();
    const h = createHistory("a", { now: c.now, coalesceMs: 100 });
    h.record("b");
    c.advance(500);
    h.record("c");
    expect(h.undo()).toBe("b");
    expect(h.undo()).toBe("a");
    expect(h.undo()).toBeUndefined();
    expect(h.redo()).toBe("b");
    expect(h.redo()).toBe("c");
    expect(h.canRedo()).toBe(false);
  });

  it("coalesces quick edits into one step", () => {
    const c = clock();
    const h = createHistory("", { now: c.now, coalesceMs: 100 });
    for (const s of ["h", "he", "hel", "hell", "hello"]) {
      h.record(s);
      c.advance(50);
    }
    expect(h.undo()).toBe("");
    expect(h.redo()).toBe("hello");
  });

  it("a new edit clears redo", () => {
    const c = clock();
    const h = createHistory(1, { now: c.now, coalesceMs: 0 });
    h.record(2);
    c.advance(10);
    h.undo();
    h.record(3);
    expect(h.canRedo()).toBe(false);
    expect(h.undo()).toBe(1);
  });

  it("breakBurst starts a new step and reset clears everything", () => {
    const c = clock();
    const h = createHistory("a", { now: c.now, coalesceMs: 1000 });
    h.record("b");
    h.breakBurst();
    h.record("c");
    expect(h.undo()).toBe("b");
    h.reset("z");
    expect(h.canUndo()).toBe(false);
  });

  it("keeps at most `limit` steps", () => {
    const c = clock();
    const h = createHistory(0, { now: c.now, coalesceMs: 0, limit: 3 });
    for (let i = 1; i <= 10; i++) {
      c.advance(10);
      h.record(i);
    }
    expect(h.undo()).toBe(9);
    expect(h.undo()).toBe(8);
    expect(h.undo()).toBeUndefined();
  });
});
