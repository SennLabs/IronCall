import { describe, expect, it } from "vitest";
import type { KeyValueRow as KeyValue } from "../ipc/api";
import {
  applyUrlText,
  buildUrlText,
  formatQuery,
  mergeQuery,
  parseQuery,
  pathParamNames,
  splitUrl,
  syncPathParams,
} from "./url";

const row = (name: string, value = "", enabled = true): KeyValue => ({ name, value, enabled });

describe("splitUrl / parseQuery", () => {
  it("splits at the first ?", () => {
    expect(splitUrl("https://x.io/a?b=1?c")).toEqual({ base: "https://x.io/a", query: "b=1?c" });
    expect(splitUrl("https://x.io/a")).toEqual({ base: "https://x.io/a", query: null });
    expect(splitUrl("https://x.io/a?")).toEqual({ base: "https://x.io/a", query: "" });
  });

  it("parses pairs without decoding", () => {
    expect(parseQuery("a=1&b=%20x&c&d=")).toEqual([
      { name: "a", value: "1" },
      { name: "b", value: "%20x" },
      { name: "c", value: "" },
      { name: "d", value: "" },
    ]);
  });

  it("keeps = inside values and skips empty segments", () => {
    expect(parseQuery("sig=a=b==&&x=1&")).toEqual([
      { name: "sig", value: "a=b==" },
      { name: "x", value: "1" },
    ]);
  });

  it("keeps {{vars}} and + untouched", () => {
    expect(parseQuery("q={{term}}&s=a+b")).toEqual([
      { name: "q", value: "{{term}}" },
      { name: "s", value: "a+b" },
    ]);
  });
});

describe("formatQuery / buildUrlText", () => {
  it("renders only enabled rows, without encoding", () => {
    const params = [row("a", "1"), row("b", "x y", false), row("c", "%2F")];
    expect(formatQuery(params)).toBe("a=1&c=%2F");
    expect(buildUrlText("https://x.io", params)).toBe("https://x.io?a=1&c=%2F");
  });

  it("omits = for empty values and skips blank rows", () => {
    expect(formatQuery([row("flag"), row("", "")])).toBe("flag");
  });

  it("has no ? when nothing is enabled", () => {
    expect(buildUrlText("https://x.io/p", [row("a", "1", false)])).toBe("https://x.io/p");
  });
});

describe("mergeQuery", () => {
  it("keeps disabled rows in place", () => {
    const existing = [row("a", "1"), row("off", "z", false), row("b", "2")];
    const merged = mergeQuery(existing, parseQuery("a=1&b=3"));
    expect(merged.map((r) => [r.name, r.value, r.enabled])).toEqual([
      ["a", "1", true],
      ["off", "z", false],
      ["b", "3", true],
    ]);
  });

  it("reuses row objects (and their unknown keys) when names match", () => {
    const a = { ...row("a", "1"), note: "keep me" } as KeyValue;
    const merged = mergeQuery([a], parseQuery("a=2"));
    expect(merged[0]).toMatchObject({ name: "a", value: "2", note: "keep me" });
    expect(mergeQuery([a], parseQuery("a=1"))[0]).toBe(a);
  });

  it("keeps a renamed row's other keys at the same position", () => {
    const a = { ...row("a", "1"), note: "x" } as KeyValue;
    expect(mergeQuery([a], parseQuery("ab=1"))[0]).toMatchObject({ name: "ab", note: "x" });
  });

  it("appends new pairs and drops removed ones", () => {
    expect(
      mergeQuery([row("a", "1"), row("b", "2")], parseQuery("a=1")).map((r) => r.name),
    ).toEqual(["a"]);
    expect(mergeQuery([row("a", "1")], parseQuery("a=1&n=9")).map((r) => r.name)).toEqual([
      "a",
      "n",
    ]);
  });

  it("handles duplicate names in order", () => {
    const merged = mergeQuery([row("t", "1"), row("t", "2")], parseQuery("t=1&t=3"));
    expect(merged.map((r) => r.value)).toEqual(["1", "3"]);
  });

  it("follows reordering typed in the URL bar", () => {
    const merged = mergeQuery([row("a", "1"), row("b", "2")], parseQuery("b=2&a=1"));
    expect(merged.map((r) => r.name)).toEqual(["b", "a"]);
  });
});

describe("applyUrlText", () => {
  it("round-trips URL-bar text exactly", () => {
    const cases = [
      "https://api.example.com/users?page=2&q=a%20b",
      "{{baseUrl}}/search?q={{term}}&flag",
      "http://localhost:8080/x?a=1&a=2",
      "https://x.io/p?sig=abc%3D%3D&redirect=https%3A%2F%2Fy.io",
    ];
    for (const text of cases) {
      const { url, params } = applyUrlText(text, []);
      expect(buildUrlText(url, params)).toBe(text);
    }
  });

  it("removing the query clears enabled params but keeps disabled ones", () => {
    const { url, params } = applyUrlText("https://x.io", [row("a", "1"), row("d", "", false)]);
    expect(url).toBe("https://x.io");
    expect(params.map((r) => r.name)).toEqual(["d"]);
  });
});

describe("path params", () => {
  it("finds :name and {name} segments in order", () => {
    expect(pathParamNames("https://x.io/orgs/:orgId/users/{userId}/x")).toEqual([
      "orgId",
      "userId",
    ]);
  });

  it("ignores ports, {{vars}} and the query", () => {
    expect(pathParamNames("http://localhost:8080/a")).toEqual([]);
    expect(pathParamNames("{{baseUrl}}/users/{{id}}")).toEqual([]);
    expect(pathParamNames("https://x.io/a?x=:notParam")).toEqual([]);
  });

  it("ignores colons inside segments and de-duplicates", () => {
    expect(pathParamNames("https://x.io/a:b/:id/:id")).toEqual(["id"]);
  });

  it("works without a scheme", () => {
    expect(pathParamNames(":base/x/:id")).toEqual(["base", "id"]);
  });

  it("keeps existing values and removes rows for deleted segments", () => {
    const existing = [
      { name: "a", value: "1" },
      { name: "b", value: "2" },
    ];
    expect(syncPathParams(existing, "https://x.io/:b/:c")).toEqual([
      { name: "b", value: "2" },
      { name: "c", value: "" },
    ]);
    expect(syncPathParams(existing, "https://x.io/:a/:b")).toBe(existing);
  });
});
