import { describe, expect, it } from "vitest";
import type { ScratchLayout } from "../ipc/api";
import { mockDraft } from "../ipc/mock";
import {
  folderOf,
  moveScratchItem,
  removeScratchFolder,
  requestsInFolder,
  scratchRows,
} from "./scratchTree";

function sample() {
  const a = mockDraft({ name: "A" });
  const b = mockDraft({ name: "B" });
  const c = mockDraft({ name: "C" });
  const layout: ScratchLayout = {
    folders: [
      { id: "F1", name: "Outer", parent: null },
      { id: "F2", name: "Inner", parent: "F1" },
      { id: "F3", name: "Empty", parent: null },
    ],
    placement: { [a.id]: "F2", [b.id]: "F1", [c.id]: "gone" },
  };
  return { requests: [a, b, c], layout, a, b, c };
}

const labels = (rows: ReturnType<typeof scratchRows>) =>
  rows.map((r) =>
    r.kind === "folder"
      ? `${"  ".repeat(r.depth)}[${r.folder.name} ${r.count}]`
      : `${"  ".repeat(r.depth)}${r.request.name}`,
  );

describe("scratch tree", () => {
  it("nests folders and requests; unknown folders fall back to the top level", () => {
    const { requests, layout, c } = sample();
    expect(labels(scratchRows(requests, layout, new Set()))).toEqual([
      "[Outer 2]",
      "  [Inner 1]",
      "    A",
      "  B",
      "[Empty 0]",
      "C",
    ]);
    expect(folderOf(layout, c.id)).toBeNull();
  });

  it("hides the contents of collapsed folders", () => {
    const { requests, layout } = sample();
    expect(labels(scratchRows(requests, layout, new Set(["F1"])))).toEqual([
      "[Outer 2]",
      "[Empty 0]",
      "C",
    ]);
  });

  it("moves requests and folders, refusing a folder into itself or a descendant", () => {
    const { layout, a } = sample();
    expect(moveScratchItem(layout, { kind: "request", id: a.id }, null)?.placement[a.id]).toBe(
      undefined,
    );
    expect(moveScratchItem(layout, { kind: "request", id: a.id }, "F3")?.placement[a.id]).toBe(
      "F3",
    );
    expect(moveScratchItem(layout, { kind: "request", id: a.id }, "F2")).toBeUndefined();
    expect(moveScratchItem(layout, { kind: "folder", id: "F1" }, "F2")).toBeUndefined();
    expect(moveScratchItem(layout, { kind: "folder", id: "F1" }, "F1")).toBeUndefined();
    const moved = moveScratchItem(layout, { kind: "folder", id: "F2" }, "F3");
    expect(moved?.folders.find((f) => f.id === "F2")?.parent).toBe("F3");
  });

  it("removes a folder with its subfolders and placements", () => {
    const { layout, a, b } = sample();
    expect(requestsInFolder(layout, "F1").sort()).toEqual([a.id, b.id].sort());
    const next = removeScratchFolder(layout, "F1");
    expect(next.folders.map((f) => f.id)).toEqual(["F3"]);
    expect(Object.keys(next.placement)).toHaveLength(1);
  });
});
