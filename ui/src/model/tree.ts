/**
 * Pure helpers over the workspace tree: flattening for display, filtering, lookups and
 * drag-and-drop targets. The backend owns ordering (PLAN.md §4.3); the UI shows
 * `children` in the order received.
 */

import type { ItemId, TreeNode } from "../ipc/api";
import { fuzzyMatch } from "./fuzzy";

/** One visible row of the sidebar tree. */
export interface TreeRow {
  node: TreeNode;
  depth: number;
  parentId: ItemId | null;
  /** Position among its siblings. */
  index: number;
  expanded: boolean;
  hasChildren: boolean;
  /** Matched character indices in the name, when filtering. */
  highlight?: number[];
}

const isContainer = (n: TreeNode) => n.kind === "collection" || n.kind === "folder";

/**
 * Flattens the tree into rows, skipping children of collapsed folders. The collection
 * root is always row 0 and always expanded.
 */
export function flatten(root: TreeNode, expanded: ReadonlySet<ItemId>): TreeRow[] {
  const rows: TreeRow[] = [];
  const walk = (node: TreeNode, depth: number, parentId: ItemId | null, index: number) => {
    const open = node.kind === "collection" || expanded.has(node.id);
    rows.push({
      node,
      depth,
      parentId,
      index,
      expanded: open,
      hasChildren: isContainer(node) && node.children.length > 0,
    });
    if (!open) return;
    node.children.forEach((c, i) => {
      walk(c, depth + 1, node.id, i);
    });
  };
  walk(root, 0, null, 0);
  return rows;
}

/** The path segment of a URL-ish text, for matching (`{{baseUrl}}/users/:id` → `/users/:id`). */
function urlPath(url: string): string {
  return url.replace(/^[a-z]+:\/\/[^/]*/i, "").replace(/^\{\{[^}]*\}\}/, "");
}

/**
 * Rows matching `query` (fuzzy, on name or URL path), plus their ancestors so matches
 * keep their context. Everything on the path to a match is shown expanded.
 */
export function filterRows(
  root: TreeNode,
  query: string,
  urlOf: (id: ItemId) => string | undefined = () => undefined,
): TreeRow[] {
  const q = query.trim();
  if (!q) return flatten(root, new Set());
  const rows: TreeRow[] = [];

  // Returns the rows for a subtree, or [] if nothing in it matches.
  const walk = (node: TreeNode, depth: number, parentId: ItemId | null, index: number) => {
    const nameMatch = node.kind === "collection" ? null : fuzzyMatch(q, node.name);
    // Open tabs may have unsaved edits, so prefer their URL over the saved one.
    const url = node.kind === "request" ? (urlOf(node.id) ?? node.url ?? undefined) : undefined;
    const urlMatch = url ? fuzzyMatch(q, urlPath(url)) : null;
    const childRows: TreeRow[] = [];
    node.children.forEach((c, i) => {
      childRows.push(...walk(c, depth + 1, node.id, i));
    });
    if (!nameMatch && !urlMatch && childRows.length === 0) return [];
    return [
      {
        node,
        depth,
        parentId,
        index,
        expanded: childRows.length > 0,
        hasChildren: isContainer(node) && node.children.length > 0,
        highlight: nameMatch?.indices,
      },
      ...childRows,
    ];
  };
  rows.push(...walk(root, 0, null, 0));
  // Keep the collection root even when nothing matches, so the tree isn't blank.
  return rows.length ? rows : flatten({ ...root, children: [] }, new Set());
}

export function findNode(root: TreeNode, id: ItemId): TreeNode | undefined {
  if (root.id === id) return root;
  for (const c of root.children) {
    const found = findNode(c, id);
    if (found) return found;
  }
  return undefined;
}

export function findParent(root: TreeNode, id: ItemId): TreeNode | undefined {
  for (const c of root.children) {
    if (c.id === id) return root;
    const found = findParent(c, id);
    if (found) return found;
  }
  return undefined;
}

/** True if `id` is `ancestor` or inside it. */
export function isWithin(root: TreeNode, ancestor: ItemId, id: ItemId): boolean {
  const a = findNode(root, ancestor);
  return !!a && !!findNode(a, id);
}

/** Every request in the tree, depth first. */
export function allRequests(root: TreeNode): TreeNode[] {
  const out: TreeNode[] = [];
  const walk = (n: TreeNode) => {
    if (n.kind === "request") out.push(n);
    for (const c of n.children) walk(c);
  };
  walk(root);
  return out;
}

/** Every folder (including the collection), for "save to workspace" pickers. */
export function allContainers(root: TreeNode): { node: TreeNode; depth: number }[] {
  const out: { node: TreeNode; depth: number }[] = [];
  const walk = (n: TreeNode, depth: number) => {
    if (!isContainer(n)) return;
    out.push({ node: n, depth });
    for (const c of n.children) walk(c, depth + 1);
  };
  walk(root, 0);
  return out;
}

/** Where a dragged row is dropped relative to the row under the pointer. */
export type DropPosition = "before" | "after" | "inside";

/** Picks before/after/inside from the pointer's vertical position within a row. */
export function dropPosition(offsetY: number, rowHeight: number, container: boolean): DropPosition {
  const f = offsetY / rowHeight;
  if (container) return f < 0.25 ? "before" : f > 0.75 ? "after" : "inside";
  return f < 0.5 ? "before" : "after";
}

/**
 * Computes the `item_move` arguments for dropping `dragId` on `targetId`, or `null` if the
 * drop is invalid (onto itself, into its own subtree, or a no-op).
 *
 * The index is in the target parent's child list *after* removing the dragged item and
 * ignoring error nodes, matching the backend's move semantics (error nodes aren't part
 * of the stored order and can't be moved or dropped next to).
 */
export function dropTarget(
  root: TreeNode,
  dragId: ItemId,
  targetId: ItemId,
  position: DropPosition,
): { parent: ItemId; index: number } | null {
  if (dragId === targetId || dragId === root.id) return null;
  const target = findNode(root, targetId);
  const dragged = findNode(root, dragId);
  if (!target || !dragged || dragged.kind === "error" || target.kind === "error") return null;
  const ordered = (n: TreeNode) => n.children.filter((c) => c.id !== dragId && c.kind !== "error");

  let parent: TreeNode | undefined;
  let index: number;
  if (position === "inside" || target.kind === "collection") {
    if (!isContainer(target)) return null;
    parent = target;
    index = ordered(target).length;
  } else {
    parent = findParent(root, targetId);
    if (!parent) return null;
    const at = ordered(parent).findIndex((c) => c.id === targetId);
    index = position === "before" ? at : at + 1;
  }
  if (isWithin(root, dragId, parent.id)) return null;

  // Dropping where it already is changes nothing.
  const currentParent = findParent(root, dragId);
  if (currentParent?.id === parent.id) {
    const currentIndex = currentParent.children
      .filter((c) => c.kind !== "error")
      .findIndex((c) => c.id === dragId);
    if (currentIndex === index) return null;
  }
  return { parent: parent.id, index };
}
