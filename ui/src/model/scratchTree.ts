/**
 * The scratch list as a tree (PLAN.md §4.6): folders from the scratch layout, with each
 * request under the folder it's placed in (or at the top level).
 */

import type { ItemId, RequestDraft, ScratchFolder, ScratchLayout } from "../ipc/api";

export type ScratchRow =
  | { kind: "folder"; folder: ScratchFolder; depth: number; expanded: boolean; count: number }
  | { kind: "request"; request: RequestDraft; depth: number; folder: ItemId | null };

/** What a drag carries or a "move" acts on. */
export type ScratchItem = { kind: "folder" | "request"; id: ItemId };

/** The folder a request is in; `null` for the top level or an unknown folder. */
export function folderOf(layout: ScratchLayout, requestId: ItemId): ItemId | null {
  const id = layout.placement[requestId];
  return id && layout.folders.some((f) => f.id === id) ? id : null;
}

/** Visible rows, depth first: folders (layout order) before requests (list order). */
export function scratchRows(
  requests: RequestDraft[],
  layout: ScratchLayout,
  collapsed: ReadonlySet<ItemId>,
): ScratchRow[] {
  const byFolder = new Map<ItemId | null, RequestDraft[]>();
  for (const r of requests) {
    const f = folderOf(layout, r.id);
    byFolder.set(f, [...(byFolder.get(f) ?? []), r]);
  }
  const count = (id: ItemId): number =>
    (byFolder.get(id)?.length ?? 0) +
    layout.folders.filter((f) => f.parent === id).reduce((n, f) => n + count(f.id), 0);

  const rows: ScratchRow[] = [];
  const walk = (parent: ItemId | null, depth: number) => {
    for (const folder of layout.folders.filter((f) => f.parent === parent)) {
      const expanded = !collapsed.has(folder.id);
      rows.push({ kind: "folder", folder, depth, expanded, count: count(folder.id) });
      if (expanded) walk(folder.id, depth + 1);
    }
    for (const request of byFolder.get(parent) ?? []) {
      rows.push({ kind: "request", request, depth, folder: parent });
    }
  };
  walk(null, 0);
  return rows;
}

/** `id` and every folder nested in it. */
export function folderAndDescendants(layout: ScratchLayout, id: ItemId): Set<ItemId> {
  const out = new Set<ItemId>([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const f of layout.folders) {
      if (f.parent && out.has(f.parent) && !out.has(f.id)) {
        out.add(f.id);
        grew = true;
      }
    }
  }
  return out;
}

/** Requests (by ID) inside a folder, at any depth. */
export function requestsInFolder(layout: ScratchLayout, id: ItemId): ItemId[] {
  const folders = folderAndDescendants(layout, id);
  return Object.entries(layout.placement)
    .filter(([, f]) => folders.has(f))
    .map(([r]) => r);
}

/**
 * The layout with `item` moved into `target` (`null`: top level), or `undefined` if
 * that's not allowed (a folder into itself or a descendant) or changes nothing.
 */
export function moveScratchItem(
  layout: ScratchLayout,
  item: ScratchItem,
  target: ItemId | null,
): ScratchLayout | undefined {
  if (item.kind === "request") {
    if (folderOf(layout, item.id) === target) return undefined;
    const placement = { ...layout.placement };
    if (target) placement[item.id] = target;
    else delete placement[item.id];
    return { ...layout, placement };
  }
  const folder = layout.folders.find((f) => f.id === item.id);
  if (!folder || folder.parent === target) return undefined;
  if (target && folderAndDescendants(layout, item.id).has(target)) return undefined;
  return {
    ...layout,
    folders: layout.folders.map((f) => (f.id === item.id ? { ...f, parent: target } : f)),
  };
}

/** The layout without a folder, its subfolders and the placements into them. */
export function removeScratchFolder(layout: ScratchLayout, id: ItemId): ScratchLayout {
  const gone = folderAndDescendants(layout, id);
  return {
    folders: layout.folders.filter((f) => !gone.has(f.id)),
    placement: Object.fromEntries(Object.entries(layout.placement).filter(([, f]) => !gone.has(f))),
  };
}
