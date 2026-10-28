import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { mockDraft } from "../ipc/mock";
import { createRequestStore } from "./request";

const withStore = (fn: (s: ReturnType<typeof createRequestStore>) => void) =>
  createRoot((dispose) => {
    fn(createRequestStore(mockDraft({ url: "https://x.io/a", extraKey: "kept" })));
    dispose();
  });

describe("request store", () => {
  it("URL-bar text updates url, params and path params, keeping the text verbatim", () => {
    withStore((s) => {
      s.setUrlText("https://x.io/users/:id?page=2&q=a%20b");
      expect(s.urlText()).toBe("https://x.io/users/:id?page=2&q=a%20b");
      expect(s.draft().url).toBe("https://x.io/users/:id");
      expect(s.draft().params?.map((p) => [p.name, p.value])).toEqual([
        ["page", "2"],
        ["q", "a%20b"],
      ]);
      expect(s.draft().pathParams).toEqual([{ name: "id", value: "" }]);
    });
  });

  it("table edits rebuild the URL-bar text", () => {
    withStore((s) => {
      s.setUrlText("https://x.io/a?x=1");
      s.setParams([...(s.draft().params ?? []), { name: "y", value: "2", enabled: true }]);
      expect(s.urlText()).toBe("https://x.io/a?x=1&y=2");
      s.setParams((s.draft().params ?? []).map((p) => ({ ...p, enabled: p.name !== "x" })));
      expect(s.urlText()).toBe("https://x.io/a?y=2");
    });
  });

  it("keeps unknown keys on edits", () => {
    withStore((s) => {
      s.setMethod("post");
      s.setBodyType("json");
      s.setBodyContent("{}");
      expect(s.draft()).toMatchObject({ method: "POST", extraKey: "kept" });
      expect(s.draft().body).toEqual({ type: "json", content: "{}" });
    });
  });

  it("switching body types keeps the content; none clears it", () => {
    withStore((s) => {
      s.setBodyType("json");
      s.setBodyContent('{"a":1}');
      s.setBodyType("text");
      expect(s.draft().body).toEqual({ type: "text", content: '{"a":1}' });
      s.setBodyType("none");
      expect(s.draft().body).toEqual({ type: "none" });
    });
  });

  it("merges settings", () => {
    withStore((s) => {
      s.setSettings({ timeoutMs: 5000 });
      s.setSettings({ verifyTls: false });
      expect(s.draft().settings).toEqual({ timeoutMs: 5000, verifyTls: false });
    });
  });

  it("form, multipart and binary bodies come back after switching away", () => {
    withStore((s) => {
      s.setBodyType("formUrlencoded");
      s.setBodyFields([{ name: "a", value: "1", enabled: true }]);
      s.setBodyType("multipart");
      expect(s.draft().body).toEqual({ type: "multipart", parts: [] });
      s.setMultipartParts([{ name: "file", type: "file", path: "data.csv", enabled: true }]);
      s.setBodyType("formUrlencoded");
      expect(s.draft().body).toEqual({
        type: "formUrlencoded",
        fields: [{ name: "a", value: "1", enabled: true }],
      });
      s.setBodyType("multipart");
      expect(s.draft().body?.parts).toHaveLength(1);
      s.setBodyType("binaryFile");
      s.setBinaryFile({ path: "big.bin", contentType: "application/octet-stream" });
      expect(s.draft().body).toEqual({
        type: "binaryFile",
        path: "big.bin",
        contentType: "application/octet-stream",
      });
    });
  });

  it("auth: inherit removes the key, other types keep unknown keys", () => {
    withStore((s) => {
      s.setAuth({ type: "bearer", token: "{{t}}", future: 1 });
      expect(s.draft().auth).toEqual({ type: "bearer", token: "{{t}}", future: 1 });
      s.setAuth({ type: "inherit" });
      expect(s.draft()).not.toHaveProperty("auth");
      expect(s.draft()).toMatchObject({ extraKey: "kept" });
    });
  });
});
