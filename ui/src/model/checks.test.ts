import { describe, expect, it } from "vitest";
import {
  alwaysSensitive,
  formatExpected,
  newAssertion,
  newExtraction,
  parseExpected,
  withOp,
  withSource,
  withTarget,
} from "./checks";

describe("expected values", () => {
  it("keeps JSON types and treats anything else as a string", () => {
    expect(parseExpected("201", "eq")).toBe(201);
    expect(parseExpected('"201"', "eq")).toBe("201");
    expect(parseExpected("true", "eq")).toBe(true);
    expect(parseExpected("null", "eq")).toBe(null);
    expect(parseExpected("application/json", "contains")).toBe("application/json");
    expect(parseExpected("{{userId}}", "eq")).toBe("{{userId}}");
    expect(parseExpected('{"a":1}', "eq")).toEqual({ a: 1 });
  });

  it("reads lists for `is one of` and raw text for regexes and types", () => {
    expect(parseExpected("200, 201", "in")).toEqual([200, 201]);
    expect(parseExpected('[200, "x"]', "in")).toEqual([200, "x"]);
    expect(parseExpected("", "in")).toEqual([]);
    expect(parseExpected("^\\d+$", "matches")).toBe("^\\d+$");
    expect(parseExpected("123", "matches")).toBe("123");
    expect(parseExpected("integer", "type")).toBe("integer");
  });

  it("round-trips through the input text", () => {
    for (const v of [201, "201", "text", true, null, [1, 2], { a: 1 }, "{{x}}"]) {
      expect(parseExpected(formatExpected(v), "eq")).toEqual(v);
    }
    expect(formatExpected("201")).toBe('"201"');
    expect(formatExpected("plain")).toBe("plain");
    expect(formatExpected(undefined)).toBe("");
    expect(formatExpected("123", "matches")).toBe("123");
  });
});

describe("editing", () => {
  it("drops the value for exists and keeps only the target's field", () => {
    const a = withOp(newAssertion(), "exists");
    expect("value" in a).toBe(false);
    expect(withOp(a, "eq").value).toBe("");
    const body = withTarget({ ...newAssertion(), unknown: 1 }, "body");
    expect(body).toMatchObject({ target: "body", path: "$.", unknown: 1 });
    expect(withTarget(body, "header")).toEqual({
      target: "header",
      op: "eq",
      value: 200,
      enabled: true,
      unknown: 1,
      name: "",
    });
    expect(withTarget(body, "status")).not.toHaveProperty("path");
  });

  it("switches extraction sources", () => {
    const e = withSource({ ...newExtraction(), var: "id" }, "regex");
    expect(e).toEqual({ var: "id", from: "regex", pattern: "", sensitive: false });
    expect(withSource(e, "header")).toMatchObject({ from: "header", name: "" });
  });

  it("knows which extractions are always sensitive", () => {
    const base = { ...newExtraction(), var: "userId", path: "$.id" };
    expect(alwaysSensitive(base)).toBe(false);
    expect(alwaysSensitive({ ...base, var: "authToken" })).toBe(true);
    expect(alwaysSensitive({ ...base, path: "$.access_token" })).toBe(true);
    expect(alwaysSensitive({ ...base, from: "cookie", name: "sid" })).toBe(true);
    expect(alwaysSensitive({ ...base, from: "header", name: "Set-Cookie" })).toBe(true);
    expect(alwaysSensitive({ ...base, from: "header", name: "Location" })).toBe(false);
  });
});
