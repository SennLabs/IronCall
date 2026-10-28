import { describe, expect, it } from "vitest";
import type { ResponseBodyInfo } from "../ipc/bindings";
import {
  AUTO_PRETTY_LIMIT,
  bodyMode,
  previewKind,
  previewSandbox,
  SMALL_BODY_LIMIT,
  suggestedFileName,
} from "./bodyView";

const body = (over: Partial<ResponseBodyInfo>): ResponseBodyInfo => ({
  bodyId: 1,
  kind: "json",
  contentType: "application/json",
  charset: "utf-8",
  charsetLossy: false,
  size: 100,
  wireSize: 100,
  rowCount: 1,
  prettyAvailable: true,
  ...over,
});

describe("bodyMode", () => {
  it("uses the editor up to the small-body limit", () => {
    expect(bodyMode(body({ size: SMALL_BODY_LIMIT }))).toEqual({
      mode: "editor",
      prettyAvailable: true,
    });
  });

  it("pretty-prints JSON/XML into the paged viewer by default up to 10 MB", () => {
    expect(bodyMode(body({ size: SMALL_BODY_LIMIT + 1, prettyAvailable: false }))).toEqual({
      mode: "paged",
      prettyAvailable: true,
      prettyByDefault: true,
    });
    expect(bodyMode(body({ kind: "xml", size: AUTO_PRETTY_LIMIT }))).toMatchObject({
      prettyByDefault: true,
    });
  });

  it("starts raw over 10 MB, but pretty-printing stays available", () => {
    expect(bodyMode(body({ size: AUTO_PRETTY_LIMIT + 1 }))).toEqual({
      mode: "paged",
      prettyAvailable: true,
      prettyByDefault: false,
    });
  });

  it("never pretty-prints text, and handles empty and binary bodies", () => {
    expect(bodyMode(body({ kind: "text", size: 5 * 1024 * 1024 }))).toMatchObject({
      prettyAvailable: false,
      prettyByDefault: false,
    });
    expect(bodyMode(body({ bodyId: null }))).toEqual({ mode: "empty" });
    expect(bodyMode(body({ kind: "empty" }))).toEqual({ mode: "empty" });
    expect(bodyMode(body({ kind: "binary" }))).toEqual({ mode: "binary" });
  });
});

describe("previewKind", () => {
  it.each([
    ["image/png", "binary", "image"],
    ["image/svg+xml; charset=utf-8", "xml", "image"],
    ["application/pdf", "binary", "pdf"],
    ["text/html; charset=utf-8", "html", "html"],
    [null, "html", "html"],
    ["video/mp4", "binary", "media"],
    ["audio/mpeg", "binary", "media"],
    ["application/json", "json", null],
    ["application/octet-stream", "binary", null],
  ] as const)("%s (%s) → %s", (type, kind, expected) => {
    expect(previewKind(type, kind)).toBe(expected);
  });
});

describe("previewSandbox", () => {
  it("is fully locked down by default", () => {
    expect(previewSandbox(false)).toBe("");
  });

  it("only ever adds allow-scripts, never allow-same-origin", () => {
    expect(previewSandbox(true)).toBe("allow-scripts");
    for (const v of [true, false]) expect(previewSandbox(v)).not.toContain("allow-same-origin");
  });
});

describe("suggestedFileName", () => {
  it("uses the last path segment when it has an extension", () => {
    expect(suggestedFileName("https://x.test/files/report.pdf?x=1", null)).toBe("report.pdf");
  });

  it("adds an extension from the content type", () => {
    expect(suggestedFileName("https://x.test/users/42", "application/json")).toBe("42.json");
    expect(suggestedFileName("https://x.test/", "image/png")).toBe("response.png");
    expect(suggestedFileName("{{base}}/x", null)).toBe("response.bin");
  });

  it("removes characters that aren't allowed in file names", () => {
    expect(suggestedFileName("https://x.test/a%3Ab.txt", null)).toBe("a_b.txt");
  });
});
