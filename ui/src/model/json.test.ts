import { describe, expect, it } from "vitest";
import { formatJson, lintJson } from "./json";

describe("lintJson", () => {
  it("accepts valid JSON and empty text", () => {
    expect(lintJson('{"a": [1, -2.5e3, true, null, "x\\"y"], "b": {}}')).toBeNull();
    expect(lintJson("  ")).toBeNull();
    expect(lintJson("[]")).toBeNull();
  });

  it("accepts {{vars}} as values and keys", () => {
    expect(lintJson('{"n": {{count}}, "s": "{{name}}", {{key}}: 1}')).toBeNull();
    expect(lintJson("[{{a}}, {{$uuid}}]")).toBeNull();
  });

  it("reports line and column of the first error", () => {
    expect(lintJson('{\n  "a": 1,\n  "b" 2\n}')).toMatchObject({
      message: 'Expected ":"',
      line: 3,
      column: 7,
    });
  });

  it("reports trailing commas", () => {
    expect(lintJson('{"a": 1,}')).toMatchObject({ message: "Trailing comma", column: 9 });
    expect(lintJson("[1,]")).toMatchObject({ message: "Trailing comma" });
  });

  it("reports unterminated strings, bad literals and extra content", () => {
    expect(lintJson('{"a": "x')).toMatchObject({ message: "Unterminated string", column: 7 });
    expect(lintJson('{"a": tru}')?.message).toMatch(/Unexpected character/);
    expect(lintJson("{} {}")?.message).toMatch(/after the end/);
    expect(lintJson('{"a": 1')).toMatchObject({ message: 'Expected "}"', column: 8 });
  });

  it("rejects single quotes and leading zeros", () => {
    expect(lintJson("{'a': 1}")).not.toBeNull();
    expect(lintJson("[01]")).not.toBeNull();
  });
});

describe("formatJson", () => {
  it("re-indents without changing values", () => {
    expect(formatJson('{"a":1.50,"b":[1,2],"c":{},"d":"x, y: z"}')).toBe(
      '{\n  "a": 1.50,\n  "b": [\n    1,\n    2\n  ],\n  "c": {},\n  "d": "x, y: z"\n}\n',
    );
  });

  it("keeps {{vars}}", () => {
    expect(formatJson('{"n":{{count}}}')).toBe('{\n  "n": {{count}}\n}\n');
  });

  it("returns null for invalid JSON", () => {
    expect(formatJson('{"a":}')).toBeNull();
  });

  it("is idempotent", () => {
    const once = formatJson('[{"a":[]},{"b":{"c":null}}]') as string;
    expect(formatJson(once)).toBe(once);
  });
});
