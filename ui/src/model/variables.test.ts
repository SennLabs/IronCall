import { describe, expect, it } from "vitest";
import type { RequestDraft, VariableContext, VisibleVar } from "../ipc/api";
import {
  applyCompletion,
  completionQuery,
  completions,
  findRefs,
  hoverInfo,
  missingSecretsUsed,
  refAt,
  refState,
  rowsToVars,
  segments,
  sourceLabel,
  unresolvedUsed,
  varsToRows,
} from "./variables";

const v = (name: string, patch: Partial<VisibleVar> = {}): VisibleVar => ({
  name,
  value: `${name}-value`,
  raw: `${name}-value`,
  secret: false,
  missing: false,
  source: { scope: "collection" },
  overrides: [],
  error: null,
  description: null,
  ...patch,
});

const ctx: VariableContext = {
  variables: [
    v("baseUrl", {
      value: "https://dev.example.com",
      source: { scope: "environment", id: "E1", name: "Dev" },
      overrides: [{ scope: "collection" }, { scope: "global" }],
    }),
    v("apiKey", {
      secret: true,
      value: null,
      raw: null,
      source: { scope: "environment", id: "E1", name: "Dev" },
    }),
    v("token", { secret: true, missing: true, value: null, raw: null }),
    v("broken", { error: "cycle: broken → broken" }),
  ],
  helpers: [{ name: "$uuid", description: "Random UUID v4", example: "3f2b-…" }],
  unresolved: ["nope"],
};

const draft = (patch: Partial<RequestDraft> = {}): RequestDraft => ({
  schema: "ironcall/request@1",
  id: "R1",
  name: "r",
  method: "GET",
  url: "{{baseUrl}}/users",
  ...patch,
});

describe("finding references", () => {
  it("finds names, helpers and offsets, allowing inner whitespace", () => {
    expect(findRefs("a{{ x }}b{{$uuid}}")).toEqual([
      { name: "x", from: 1, to: 8, helper: false },
      { name: "$uuid", from: 9, to: 18, helper: true },
    ]);
    expect(findRefs("{{}} {{ bad name }} {single}")).toEqual([]);
  });

  it("finds the reference under an offset", () => {
    expect(refAt("go {{baseUrl}}/x", 5)?.name).toBe("baseUrl");
    expect(refAt("go {{baseUrl}}/x", 1)).toBeUndefined();
  });
});

describe("highlighting", () => {
  it("classifies defined, helper, unresolved and missing references", () => {
    expect(refState("baseUrl", ctx)).toBe("defined");
    expect(refState("$uuid", ctx)).toBe("helper");
    expect(refState("$nope", ctx)).toBe("unresolved");
    expect(refState("nope", ctx)).toBe("unresolved");
    expect(refState("undeclared", ctx)).toBe("unresolved");
    expect(refState("token", ctx)).toBe("missing");
  });

  it("stays neutral while the context is loading", () => {
    expect(refState("anything", undefined)).toBe("unknown");
    expect(refState("$uuid", undefined)).toBe("helper");
  });

  it("splits text into plain runs and references", () => {
    expect(segments("{{baseUrl}}/u/{{nope}}?x", ctx)).toEqual([
      { text: "{{baseUrl}}", state: "defined", name: "baseUrl" },
      { text: "/u/", state: "plain" },
      { text: "{{nope}}", state: "unresolved", name: "nope" },
      { text: "?x", state: "plain" },
    ]);
  });

  it("combines the backend's unresolved list with a local check", () => {
    const d = draft({
      url: "{{baseUrl}}/{{local}}",
      headers: [
        { name: "X", value: "{{nope}}" },
        { name: "Off", value: "{{ignored}}", enabled: false },
      ],
    });
    expect(unresolvedUsed(d, ctx)).toEqual(["local", "nope"]);
  });
});

describe("autocomplete", () => {
  it("detects an unfinished {{ before the caret", () => {
    expect(completionQuery("https://{{ba", 12)).toEqual({ query: "ba", start: 10 });
    expect(completionQuery("{{", 2)).toEqual({ query: "", start: 2 });
    expect(completionQuery("{{baseUrl}}/x", 13)).toBeUndefined();
    expect(completionQuery("plain", 5)).toBeUndefined();
  });

  it("suggests variables and helpers with masked secret previews", () => {
    const all = completions(ctx, "");
    expect(all.map((c) => c.name)).toEqual(["apiKey", "baseUrl", "broken", "token", "$uuid"]);
    expect(all.find((c) => c.name === "apiKey")?.detail).toBe("••••");
    expect(all.find((c) => c.name === "token")?.detail).toBe("secret (not set)");
    expect(all.find((c) => c.name === "baseUrl")).toMatchObject({
      detail: "https://dev.example.com",
      source: "Environment › Dev",
    });
    expect(completions(ctx, "$").map((c) => c.name)).toEqual(["$uuid"]);
    expect(completions(ctx, "ur").map((c) => c.name)).toEqual(["baseUrl"]);
  });

  it("prefers prefix matches", () => {
    const c: VariableContext = { ...ctx, variables: [v("myUrl"), v("urlBase")] };
    expect(completions(c, "url").map((x) => x.name)).toEqual(["urlBase", "myUrl"]);
  });

  it("inserts the name and closes the braces unless they already follow", () => {
    expect(applyCompletion("a {{ba", 4, 6, "baseUrl")).toEqual({
      text: "a {{baseUrl}}",
      caret: 13,
    });
    expect(applyCompletion("{{ba}}/x", 2, 4, "baseUrl")).toEqual({
      text: "{{baseUrl}}/x",
      caret: 11,
    });
  });
});

describe("hover", () => {
  it("shows value, source and what it overrides", () => {
    expect(hoverInfo("baseUrl", ctx)).toEqual({
      title: "{{baseUrl}}",
      value: "https://dev.example.com",
      source: "Environment › Dev",
      overrides: ["Collection", "Global"],
      problem: undefined,
    });
  });

  it("masks secrets and explains problems", () => {
    expect(hoverInfo("apiKey", ctx).value).toBe("••••");
    expect(hoverInfo("token", ctx).problem).toMatch(/no value/);
    expect(hoverInfo("broken", ctx).problem).toMatch(/cycle/);
    expect(hoverInfo("nope", ctx).problem).toMatch(/Not defined/);
    expect(hoverInfo("$uuid", ctx)).toMatchObject({ source: "Random UUID v4" });
  });

  it("labels every source", () => {
    expect(sourceLabel({ scope: "folder", id: "F", name: "Users" })).toBe("Folder › Users");
    expect(sourceLabel({ scope: "request" })).toBe("Request");
    expect(sourceLabel({ scope: "runtime" })).toBe("Runtime");
  });
});

describe("missing secrets", () => {
  it("lists missing secrets the request uses", () => {
    expect(missingSecretsUsed(draft({ url: "{{baseUrl}}?t={{token}}" }), ctx)).toEqual(["token"]);
    expect(missingSecretsUsed(draft(), ctx)).toEqual([]);
    expect(missingSecretsUsed(draft({ url: "{{token}}" }), undefined)).toEqual([]);
  });
});

describe("rows", () => {
  it("round-trips plain, detailed and secret variables, keeping unknown keys", () => {
    const vars = {
      baseUrl: "https://x",
      region: { value: "ap-southeast-2", description: "AWS region", future: 1 },
      apiKey: { secret: true, description: "key" },
    };
    const rows = varsToRows(vars);
    expect(rows.map((r) => [r.name, r.secret, r.value])).toEqual([
      ["baseUrl", false, "https://x"],
      ["region", false, "ap-southeast-2"],
      ["apiKey", true, ""],
    ]);
    expect(rowsToVars(rows)).toEqual(vars);
  });

  it("never writes a value for a secret row, even a pending one", () => {
    const rows = varsToRows({ apiKey: "was-plain" });
    const secret = rows.map((r) => ({
      ...r,
      secret: true,
      pendingSecret: r.value,
      value: "leak?",
    }));
    expect(rowsToVars(secret)).toEqual({ apiKey: { secret: true } });
    expect(JSON.stringify(rowsToVars(secret))).not.toContain("was-plain");
  });

  it("drops unnamed rows", () => {
    expect(
      rowsToVars([{ name: "  ", value: "x", secret: false, description: "", extra: {} }]),
    ).toEqual({});
  });
});
