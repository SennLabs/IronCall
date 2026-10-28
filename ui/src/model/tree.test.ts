import { describe, expect, it } from "vitest";
import type { TreeNode } from "../ipc/api";
import { allContainers, dropPosition, dropTarget, filterRows, findParent, flatten } from "./tree";

let n = 0;
function node(kind: TreeNode["kind"], name: string, children: TreeNode[] = []): TreeNode {
  n += 1;
  return {
    id: `${kind}-${name}-${n}`,
    kind,
    name,
    method: kind === "request" ? "GET" : null,
    url: kind === "request" ? `https://api.example.com/${name}` : null,
    path: name,
    error: kind === "error" ? "bad yaml at line 3" : null,
    readOnly: false,
    children,
  };
}

function sample() {
  const list = node("request", "List users");
  const create = node("request", "Create user");
  const users = node("folder", "Users", [list, create]);
  const health = node("request", "Health");
  const broken = node("error", "broken.yaml");
  const orders = node("folder", "Orders", [node("request", "List orders")]);
  const root = node("collection", "Demo", [users, orders, health, broken]);
  return { root, users, list, create, health, orders, broken };
}

describe("flatten", () => {
  it("shows the collection and top level, in backend order, with folders collapsed", () => {
    const { root } = sample();
    const rows = flatten(root, new Set());
    expect(rows.map((r) => r.node.name)).toEqual([
      "Demo",
      "Users",
      "Orders",
      "Health",
      "broken.yaml",
    ]);
    expect(rows.map((r) => r.depth)).toEqual([0, 1, 1, 1, 1]);
    expect(rows[1]?.hasChildren).toBe(true);
    expect(rows[1]?.expanded).toBe(false);
  });

  it("includes children of expanded folders", () => {
    const { root, users } = sample();
    const rows = flatten(root, new Set([users.id]));
    expect(rows.map((r) => r.node.name)).toEqual([
      "Demo",
      "Users",
      "List users",
      "Create user",
      "Orders",
      "Health",
      "broken.yaml",
    ]);
    expect(rows[3]).toMatchObject({ depth: 2, parentId: users.id, index: 1 });
  });
});

describe("filterRows", () => {
  it("keeps matches and their ancestors, expanded", () => {
    const { root } = sample();
    const rows = filterRows(root, "creat");
    expect(rows.map((r) => r.node.name)).toEqual(["Demo", "Users", "Create user"]);
    expect(rows[1]?.expanded).toBe(true);
    expect(rows[2]?.highlight).toEqual([0, 1, 2, 3, 4]);
  });

  it("matches folders and shows their matching children only", () => {
    const { root } = sample();
    expect(filterRows(root, "orders").map((r) => r.node.name)).toEqual([
      "Demo",
      "Orders",
      "List orders",
    ]);
  });

  it("matches the URL path when known", () => {
    const { root, health } = sample();
    const urls = new Map([[health.id, "{{baseUrl}}/status/ping"]]);
    expect(filterRows(root, "ping", (id) => urls.get(id)).map((r) => r.node.name)).toEqual([
      "Demo",
      "Health",
    ]);
  });

  it("shows just the root when nothing matches", () => {
    const { root } = sample();
    expect(filterRows(root, "zzz").map((r) => r.node.name)).toEqual(["Demo"]);
  });

  it("filters 2,000 requests in under 100ms", () => {
    const folders = Array.from({ length: 40 }, (_, f) =>
      node(
        "folder",
        `Folder ${f}`,
        Array.from({ length: 50 }, (_, r) => node("request", `Request ${f}-${r} update thing`)),
      ),
    );
    const root = node("collection", "Big", folders);
    filterRows(root, "warmup");
    const start = performance.now();
    const rows = filterRows(root, "req 7-1 upd");
    const elapsed = performance.now() - start;
    expect(rows.length).toBeGreaterThan(1);
    expect(elapsed).toBeLessThan(100);
  });
});

describe("dropTarget", () => {
  it("reorders within a folder (index after removing the dragged item)", () => {
    const { root, users, list, create } = sample();
    expect(dropTarget(root, list.id, create.id, "after")).toEqual({ parent: users.id, index: 1 });
    expect(dropTarget(root, create.id, list.id, "before")).toEqual({ parent: users.id, index: 0 });
  });

  it("moves into a folder at the end", () => {
    const { root, orders, health } = sample();
    expect(dropTarget(root, health.id, orders.id, "inside")).toEqual({
      parent: orders.id,
      index: 1,
    });
  });

  it("moves next to an item in another folder", () => {
    const { root, health, list, users } = sample();
    expect(dropTarget(root, health.id, list.id, "before")).toEqual({ parent: users.id, index: 0 });
  });

  it("dropping on the collection moves to the top level end (error nodes don't count)", () => {
    const { root, list } = sample();
    expect(dropTarget(root, list.id, root.id, "before")).toEqual({ parent: root.id, index: 3 });
  });

  it("error nodes can't be dragged or dropped next to", () => {
    const { root, broken, health } = sample();
    expect(dropTarget(root, broken.id, health.id, "before")).toBeNull();
    expect(dropTarget(root, health.id, broken.id, "after")).toBeNull();
  });

  it("rejects no-ops, self drops and moving a folder into itself", () => {
    const { root, users, list, create } = sample();
    expect(dropTarget(root, list.id, list.id, "after")).toBeNull();
    expect(dropTarget(root, list.id, create.id, "before")).toBeNull(); // already there
    expect(dropTarget(root, users.id, list.id, "after")).toBeNull();
    expect(dropTarget(root, users.id, users.id, "inside")).toBeNull();
    expect(dropTarget(root, list.id, create.id, "inside")).toBeNull(); // not a folder
  });

  it("dropPosition splits container rows into thirds-ish", () => {
    expect(dropPosition(2, 24, true)).toBe("before");
    expect(dropPosition(12, 24, true)).toBe("inside");
    expect(dropPosition(22, 24, true)).toBe("after");
    expect(dropPosition(11, 24, false)).toBe("before");
    expect(dropPosition(13, 24, false)).toBe("after");
  });
});

describe("lookups", () => {
  it("findParent and allContainers", () => {
    const { root, users, list } = sample();
    expect(findParent(root, list.id)?.id).toBe(users.id);
    expect(allContainers(root).map((c) => [c.node.name, c.depth])).toEqual([
      ["Demo", 0],
      ["Users", 1],
      ["Orders", 1],
    ]);
  });
});
