import { describe, expect, it } from "vitest";
import { mockDraft } from "../ipc/mock";
import { bodyFilePaths, uploadPercent } from "./bodyFiles";

describe("body files", () => {
  it("lists enabled multipart file parts and binary bodies", () => {
    const multipart = mockDraft({
      body: {
        type: "multipart",
        parts: [
          { name: "a", type: "file", path: "data/a.csv", enabled: true },
          { name: "b", type: "file", path: "/tmp/b.bin", enabled: false },
          { name: "c", type: "text", value: "x" },
          { name: "d", type: "file", path: "" },
        ],
      },
    });
    expect(bodyFilePaths(multipart)).toEqual(["data/a.csv"]);
    expect(bodyFilePaths(mockDraft({ body: { type: "binaryFile", path: "big.bin" } }))).toEqual([
      "big.bin",
    ]);
    expect(bodyFilePaths(mockDraft({ body: { type: "json", content: "{}" } }))).toEqual([]);
  });

  it("computes upload percentage", () => {
    expect(uploadPercent(50, 200)).toBe(25);
    expect(uploadPercent(10, null)).toBeNull();
    expect(uploadPercent(300, 200)).toBe(100);
  });
});
