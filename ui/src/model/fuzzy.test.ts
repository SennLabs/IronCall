import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches subsequences, ignoring case and spaces", () => {
    expect(fuzzyMatch("gu", "Get user")?.indices).toEqual([0, 4]);
    expect(fuzzyMatch("get user", "Get user")).not.toBeNull();
    expect(fuzzyMatch("xyz", "Get user")).toBeNull();
  });

  it("empty query matches everything", () => {
    expect(fuzzyMatch("", "anything")).toEqual({ score: 0, indices: [] });
  });

  it("ranks word starts and consecutive runs higher", () => {
    const a = fuzzyMatch("cu", "Create user");
    const b = fuzzyMatch("cu", "Account");
    expect(a && b && a.score > b.score).toBe(true);
    const run = fuzzyMatch("user", "List users");
    const spread = fuzzyMatch("user", "Update session error");
    expect(run && spread && run.score > spread.score).toBe(true);
  });
});
