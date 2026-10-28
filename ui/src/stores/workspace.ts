/**
 * The open workspace: its tree, expansion state, selection and the tree operations
 * (PLAN.md Phase 2). Operations call the backend, then refresh the tree from it; the
 * backend owns file names, IDs and ordering.
 */

import { batch, createSignal } from "solid-js";
import {
  api,
  type ItemId,
  type NewItemKind,
  type RecentWorkspace,
  type TreeNode,
  type WorkspaceInfo,
} from "../ipc/api";
import { allRequests, findNode, findParent } from "../model/tree";
import { attempt, notify } from "./notify";

const expandedKey = (id: string) => `ironcall.expanded.${id}`;

function loadExpanded(id: string): Set<ItemId> {
  try {
    const raw = localStorage.getItem(expandedKey(id));
    return new Set(raw ? (JSON.parse(raw) as ItemId[]) : []);
  } catch {
    return new Set();
  }
}

function saveExpanded(id: string, set: Set<ItemId>) {
  try {
    localStorage.setItem(expandedKey(id), JSON.stringify([...set]));
  } catch {
    // Not persisted; fine.
  }
}

export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;

export interface WorkspaceStoreOptions {
  /** A request was renamed here (open tabs need its new name and file hash). */
  onRenamed?: (id: ItemId) => void;
  /** Requests were moved to the trash here. */
  onRemoved?: (ids: ItemId[]) => void;
}

export function createWorkspaceStore(options: WorkspaceStoreOptions = {}) {
  const [info, setInfo] = createSignal<WorkspaceInfo | null>(null);
  const [tree, setTree] = createSignal<TreeNode | null>(null);
  const [recent, setRecent] = createSignal<RecentWorkspace[]>([]);
  const [expanded, setExpanded] = createSignal<Set<ItemId>>(new Set());
  const [selected, setSelected] = createSignal<ItemId>();
  /** Item whose name is being edited inline in the tree. */
  const [renaming, setRenaming] = createSignal<ItemId>();

  const set = (w: WorkspaceInfo | null) =>
    batch(() => {
      setInfo(w);
      setTree(w?.tree ?? null);
      setExpanded(w ? loadExpanded(w.id) : new Set<ItemId>());
      setSelected(undefined);
      setRenaming(undefined);
    });

  const refresh = async () => {
    const w = await attempt(() => api.workspaceCurrent(), "Couldn't refresh the workspace");
    if (w)
      batch(() => {
        setInfo(w);
        setTree(w.tree);
      });
  };

  const refreshRecent = async () => {
    setRecent(
      (await attempt(() => api.recentWorkspaces(), "Couldn't list recent workspaces")) ?? [],
    );
  };

  const expand = (id: ItemId, open = true) => {
    const w = info();
    setExpanded((s) => {
      const next = new Set(s);
      if (open) next.add(id);
      else next.delete(id);
      if (w) saveExpanded(w.id, next);
      return next;
    });
  };

  /** Runs an operation, then refreshes the tree. */
  const op = async <T>(action: () => Promise<T>, failure: string): Promise<T | undefined> => {
    const result = await attempt(action, failure);
    await refresh();
    return result;
  };

  return {
    info,
    tree,
    recent,
    expanded,
    selected,
    setSelected,
    renaming,
    setRenaming,
    expand,
    toggle: (id: ItemId) => expand(id, !expanded().has(id)),
    refreshRecent,
    refresh,
    /** Applies a tree pushed by a `WorkspaceChanged` event. */
    applyTree(next: TreeNode) {
      setTree(next);
      setInfo((w) => (w ? { ...w, tree: next } : w));
    },

    async open(path: string) {
      const w = await attempt(() => api.workspaceOpen(path), "Couldn't open the workspace");
      if (w) set(w);
      await refreshRecent();
      return w;
    },
    async create(path: string, name: string) {
      const w = await attempt(
        () => api.workspaceCreate(path, name),
        "Couldn't create the workspace",
      );
      if (w) set(w);
      await refreshRecent();
      return w;
    },
    async createSample(path: string) {
      const w = await attempt(
        () => api.workspaceCreateSample(path),
        "Couldn't create the sample workspace",
      );
      if (w) set(w);
      await refreshRecent();
      return w;
    },
    async close() {
      await attempt(() => api.workspaceClose(), "Couldn't close the workspace");
      set(null);
      await refreshRecent();
    },
    async adopt() {
      set((await attempt(() => api.workspaceCurrent(), "Couldn't read the workspace")) ?? null);
    },
    async forget(path: string) {
      await attempt(() => api.forgetRecentWorkspace(path), "Couldn't update recent workspaces");
      await refreshRecent();
    },

    /** Creates an item and starts renaming it inline. */
    async createItem(parent: ItemId, kind: NewItemKind) {
      const name = kind === "folder" ? "New folder" : "New request";
      const node = await op(() => api.itemCreate(parent, kind, name, null), "Couldn't create it");
      if (node) {
        batch(() => {
          expand(parent);
          setSelected(node.id);
          setRenaming(node.id);
        });
      }
      return node;
    },
    async rename(id: ItemId, name: string) {
      const trimmed = name.trim();
      setRenaming(undefined);
      const current = tree() && findNode(tree() as TreeNode, id);
      if (!trimmed || current?.name === trimmed) return;
      const done = await op(
        () => api.itemRename(id, trimmed).then(() => true),
        "Couldn't rename it",
      );
      if (done && current?.kind === "request") options.onRenamed?.(id);
    },
    renameFileToMatch: (id: ItemId) =>
      op(() => api.itemRenameFileToMatch(id), "Couldn't rename the file"),
    async duplicate(id: ItemId) {
      const node = await op(() => api.itemDuplicate(id), "Couldn't duplicate it");
      if (node) setSelected(node.id);
      return node;
    },
    /** Asks, then moves the item to the OS trash. Resolves true if deleted. */
    async remove(id: ItemId) {
      const t = tree();
      const node = t ? findNode(t, id) : undefined;
      if (!node || node.kind === "collection") return false;
      const what =
        node.kind === "folder"
          ? `the folder "${node.name}" and everything in it`
          : `"${node.name}"`;
      const ok = await api.confirm(`Move ${what} to the trash? You can restore it from there.`, {
        title: "Delete",
        okLabel: "Move to trash",
        kind: "warning",
      });
      if (!ok) return false;
      const parent = findParent(t as TreeNode, id);
      const done = await op(() => api.itemDelete(id).then(() => true), "Couldn't delete it");
      if (done && selected() === id) setSelected(parent?.id);
      if (done) {
        options.onRemoved?.(allRequests(node).map((r) => r.id));
        notify(`Moved ${node.name} to the trash`);
      }
      return !!done;
    },
    move: (id: ItemId, parent: ItemId, index: number) =>
      op(() => api.itemMove(id, parent, index), "Couldn't move it").then(() => expand(parent)),
    reveal: (id: ItemId) => attempt(() => api.itemReveal(id), "Couldn't reveal it"),
    openExternal: (id: ItemId) => attempt(() => api.itemOpenExternal(id), "Couldn't open it"),
  };
}
