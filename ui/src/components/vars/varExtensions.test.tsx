import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { render, screen } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import type { VariableContext } from "../../ipc/api";
import { hoverInfo } from "../../model/variables";
import { VarTooltip } from "./VarInput";
import { varCompletionSource } from "./varExtensions";

const ctx: VariableContext = {
  variables: [
    {
      name: "baseUrl",
      value: "https://dev.example.com",
      raw: "https://dev.example.com",
      secret: false,
      missing: false,
      source: { scope: "environment", id: "E1", name: "Dev" },
      overrides: [{ scope: "collection" }],
      error: null,
      description: null,
    },
  ],
  helpers: [{ name: "$uuid", description: "Random UUID v4", example: "3f2b" }],
  unresolved: [],
};

function complete(doc: string, pos = doc.length) {
  const state = EditorState.create({ doc });
  return varCompletionSource(() => ctx)(new CompletionContext(state, pos, false));
}

describe("body editor completions", () => {
  it("offers variables after {{ and closes the braces", () => {
    const result = complete('{"url": "{{ba');
    expect(result?.from).toBe(11);
    expect(result?.options.map((o) => o.label)).toContain("baseUrl");
    expect(result?.options.find((o) => o.label === "baseUrl")?.apply).toBe("baseUrl}}");
  });

  it("doesn't duplicate closing braces that are already there", () => {
    const result = complete("{{ba}}", 4);
    expect(result?.options.find((o) => o.label === "baseUrl")?.apply).toBe("baseUrl");
  });

  it("stays quiet outside {{", () => {
    expect(complete('{"a": 1')).toBeNull();
  });
});

describe("hover tooltip", () => {
  it("shows value, source and overrides", () => {
    render(() => <VarTooltip info={hoverInfo("baseUrl", ctx)} />);
    expect(screen.getByText("{{baseUrl}}")).toBeDefined();
    expect(screen.getByText("https://dev.example.com")).toBeDefined();
    expect(screen.getByText("from Environment › Dev")).toBeDefined();
    expect(screen.getByText("overrides Collection")).toBeDefined();
  });

  it("explains an undefined name", () => {
    render(() => <VarTooltip info={hoverInfo("nope", ctx)} />);
    expect(screen.getByText("Not defined in any scope")).toBeDefined();
  });
});
