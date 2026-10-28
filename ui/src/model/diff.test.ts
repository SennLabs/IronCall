import { describe, expect, it } from "vitest";
import { diffLines, hasChanges, toDisplayText } from "./diff";

describe("diffLines", () => {
  it("marks unchanged, added and removed lines with line numbers", () => {
    const d = diffLines("a\nb\nc", "a\nx\nc\nd");
    expect(d).toEqual([
      { op: "same", text: "a", left: 1, right: 1 },
      { op: "added", text: "x", right: 2 },
      { op: "removed", text: "b", left: 2 },
      { op: "same", text: "c", left: 3, right: 3 },
      { op: "added", text: "d", right: 4 },
    ]);
    expect(hasChanges(d)).toBe(true);
  });

  it("identical texts have no changes", () => {
    expect(hasChanges(diffLines("a\nb", "a\nb"))).toBe(false);
  });

  it("handles empty sides", () => {
    expect(diffLines("", "a").filter((l) => l.op !== "same")).toHaveLength(2);
  });
});

describe("toDisplayText", () => {
  it("renders requests as readable YAML-like text", () => {
    const text = toDisplayText({
      name: "Get user",
      method: "GET",
      url: "https://x.io/users/:id",
      headers: [{ name: "Accept", value: "application/json", enabled: true }],
      body: { type: "json", content: '{\n  "a": 1\n}' },
      empty: [],
      nothing: null,
    });
    expect(text).toBe(
      [
        "name: Get user",
        "method: GET",
        'url: "https://x.io/users/:id"',
        "headers:",
        "  - name: Accept",
        "    value: application/json",
        "    enabled: true",
        "body:",
        "  type: json",
        "  content: |",
        "    {",
        '      "a": 1',
        "    }",
        "empty: []",
      ].join("\n"),
    );
  });
});
