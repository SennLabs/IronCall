/**
 * In-memory workspaces for the mock backend (tests and browser-only dev).
 *
 * Models just enough of `ironcall-store`: a tree of folders, requests and error nodes
 * with display order, content hashes for conflict detection, scratch requests, the
 * session, recent workspaces and dialog answers. Tests drive external changes with
 * `externalEdit` / `externalDelete`, which emit `WorkspaceChanged` like the file watcher.
 */

import type { KeyValueRow, RequestDraft } from "./api";
import type { AuthDraft } from "./networkTypes";
import {
  type CollectionDraft,
  type FolderDraft,
  type ItemId,
  type LoadedRequest,
  type NewItemKind,
  type RecentWorkspace,
  type SaveOutcome,
  type ScratchLayout,
  type Session,
  type TreeNode,
  type WorkspaceChanged,
  WorkspaceFailure,
  type WorkspaceInfo,
} from "./workspaceTypes";

/** Seed for a mock workspace tree. */
export type MockItem =
  | {
      folder: string;
      id?: string;
      description?: string;
      vars?: Record<string, unknown>;
      auth?: AuthDraft;
      headers?: KeyValueRow[];
      children?: MockItem[];
    }
  | {
      request: string;
      id?: string;
      method?: string;
      url?: string;
      readOnly?: boolean;
      /** Extra request fields (auth, headers, body…). */
      fields?: Partial<RequestDraft>;
    }
  | { error: string; message: string; id?: string };

export interface MockWorkspaceSeed {
  name: string;
  id?: string;
  description?: string;
  /** Collection variables (`ironcall.yaml`). */
  vars?: Record<string, unknown>;
  auth?: AuthDraft;
  headers?: KeyValueRow[];
  items?: MockItem[];
}

interface Node {
  id: ItemId;
  kind: "folder" | "request" | "error";
  /** File or directory name. */
  file: string;
  parent: ItemId;
  name: string;
  request?: RequestDraft;
  folder?: FolderDraft;
  error?: string;
  readOnly?: boolean;
}

interface MockWorkspace {
  root: string;
  collection: CollectionDraft;
  nodes: Map<ItemId, Node>;
  /** Child order per container (collection or folder ID). */
  order: Map<ItemId, ItemId[]>;
}

/** Answers the mock dialogs give; tests change them before acting. */
export interface MockDialogAnswers {
  folder: string | null;
  confirm: boolean;
  choose: "yes" | "no" | "cancel";
}

/** Same wording as the backend's metadata conflict error. */
export const CHANGED_ON_DISK =
  "this file changed on disk since it was opened; reload it and try again";

let counter = 0;
/** A ULID-looking unique ID (Crockford base32 alphabet, 26 chars). */
export function mockId(): string {
  counter += 1;
  return `01MOCKWS${String(counter).padStart(18, "0")}`;
}

/** Stable content hash (FNV-1a over JSON), standing in for the backend's file hash. */
export function contentHash(value: unknown): string {
  const text = JSON.stringify(value);
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

const clone = <T>(v: T): T => structuredClone(v);

function slug(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 60) || "untitled"
  );
}

export function createMockWorkspaces(
  seeds: Record<string, MockWorkspaceSeed> = {},
  init: {
    session?: Session;
    scratch?: RequestDraft[];
    scratchLayout?: ScratchLayout;
    recent?: string[];
  } = {},
) {
  const workspaces = new Map<string, MockWorkspace>();
  const listeners = new Set<(c: WorkspaceChanged) => void>();
  const scratch = new Map<ItemId, RequestDraft>();
  const scratchTouched = new Map<ItemId, number>();
  let scratchLayout: ScratchLayout = clone(init.scratchLayout ?? { folders: [], placement: {} });
  let session: Session = init.session ?? { workspace: null, tabs: [], active: null };
  let current: MockWorkspace | null = null;
  const recent: { path: string; at: number }[] = [];
  const dialog: MockDialogAnswers = { folder: null, confirm: true, choose: "cancel" };
  const calls: string[] = [];
  const closeHandlers = new Set<() => Promise<boolean>>();

  for (const r of init.scratch ?? []) {
    scratch.set(r.id, clone(r));
    scratchTouched.set(r.id, Date.now());
  }

  const build = (path: string, seed: MockWorkspaceSeed): MockWorkspace => {
    const collectionId = seed.id ?? mockId();
    const ws: MockWorkspace = {
      root: path,
      collection: {
        schema: "ironcall/collection@1",
        id: collectionId,
        name: seed.name,
        ...(seed.description ? { description: seed.description } : {}),
        ...(seed.vars ? { vars: structuredClone(seed.vars) } : {}),
        ...(seed.auth ? { auth: structuredClone(seed.auth) } : {}),
        ...(seed.headers ? { headers: structuredClone(seed.headers) } : {}),
      },
      nodes: new Map(),
      order: new Map([[collectionId, []]]),
    };
    const add = (items: MockItem[], parent: ItemId) => {
      for (const item of items) {
        const id = item.id ?? mockId();
        ws.order.get(parent)?.push(id);
        if ("folder" in item) {
          ws.order.set(id, []);
          ws.nodes.set(id, {
            id,
            kind: "folder",
            file: slug(item.folder),
            parent,
            name: item.folder,
            folder: {
              schema: "ironcall/folder@1",
              id,
              name: item.folder,
              ...(item.description ? { description: item.description } : {}),
              ...(item.vars ? { vars: structuredClone(item.vars) } : {}),
              ...(item.auth ? { auth: structuredClone(item.auth) } : {}),
              ...(item.headers ? { headers: structuredClone(item.headers) } : {}),
            },
          });
          add(item.children ?? [], id);
        } else if ("request" in item) {
          ws.nodes.set(id, {
            id,
            kind: "request",
            file: `${slug(item.request)}.yaml`,
            parent,
            name: item.request,
            readOnly: item.readOnly,
            request: {
              schema: "ironcall/request@1",
              id,
              name: item.request,
              method: item.method ?? "GET",
              url: item.url ?? "",
              ...structuredClone(item.fields ?? {}),
            },
          });
        } else {
          ws.nodes.set(id, {
            id,
            kind: "error",
            file: item.error,
            parent,
            name: item.error,
            error: item.message,
          });
        }
      }
    };
    add(seed.items ?? [], collectionId);
    return ws;
  };

  for (const [path, seed] of Object.entries(seeds)) workspaces.set(path, build(path, seed));
  for (const path of init.recent ?? Object.keys(seeds)) recent.push({ path, at: Date.now() });

  const ws = (): MockWorkspace => {
    if (!current) throw new WorkspaceFailure("no workspace is open");
    return current;
  };

  const pathOf = (w: MockWorkspace, id: ItemId): string => {
    const parts: string[] = [];
    let node = w.nodes.get(id);
    while (node) {
      parts.unshift(node.file);
      node = w.nodes.get(node.parent);
    }
    return parts.join("/");
  };

  const treeOf = (w: MockWorkspace, id: ItemId = w.collection.id): TreeNode => {
    const node = w.nodes.get(id);
    const children = (w.order.get(id) ?? [])
      .map((c) => w.nodes.get(c))
      .filter((n): n is Node => !!n)
      .map((n) => treeOf(w, n.id));
    if (!node) {
      return {
        id,
        kind: "collection",
        name: w.collection.name,
        method: null,
        url: null,
        path: "",
        error: null,
        readOnly: false,
        children,
      };
    }
    return {
      id,
      kind: node.kind,
      name: node.kind === "request" ? (node.request?.name ?? node.name) : node.name,
      method: node.kind === "request" ? (node.request?.method ?? "GET") : null,
      url: node.kind === "request" ? (node.request?.url ?? "") : null,
      path: pathOf(w, id),
      error: node.error ?? null,
      readOnly: !!node.readOnly,
      children,
    };
  };

  const info = (w: MockWorkspace): WorkspaceInfo => ({
    id: w.collection.id,
    name: w.collection.name,
    root: w.root,
    tree: treeOf(w),
  });

  const emit = (change: Partial<WorkspaceChanged>) => {
    const full: WorkspaceChanged = {
      tree: null,
      changed: [],
      removed: [],
      metaChanged: false,
      environmentsChanged: false,
      ...change,
    };
    for (const l of listeners) l(full);
  };
  const treeChanged = () => current && emit({ tree: treeOf(current) });

  const touchRecent = (path: string) => {
    const i = recent.findIndex((r) => r.path === path);
    if (i >= 0) recent.splice(i, 1);
    recent.unshift({ path, at: Date.now() });
  };

  const node = (id: ItemId): Node => {
    const n = ws().nodes.get(id);
    if (!n) throw new WorkspaceFailure(`no item with ID ${id}`);
    return n;
  };
  const container = (id: ItemId): ItemId => {
    const w = ws();
    if (id === w.collection.id) return id;
    if (node(id).kind !== "folder") throw new WorkspaceFailure("parent must be a folder");
    return id;
  };
  const removeFromOrder = (w: MockWorkspace, id: ItemId) => {
    for (const list of w.order.values()) {
      const i = list.indexOf(id);
      if (i >= 0) list.splice(i, 1);
    }
  };
  const loaded = (n: Node): LoadedRequest => {
    if (!n.request) throw new WorkspaceFailure(`${n.name} is not a request`);
    return { request: clone(n.request), hash: contentHash(n.request), readOnly: !!n.readOnly };
  };

  const insertRequest = (parent: ItemId, request: RequestDraft, index: number | null): TreeNode => {
    const w = ws();
    const p = container(parent);
    const n: Node = {
      id: request.id,
      kind: "request",
      file: `${slug(request.name)}.yaml`,
      parent: p,
      name: request.name,
      request: clone(request),
    };
    w.nodes.set(n.id, n);
    const list = w.order.get(p) ?? [];
    list.splice(index ?? list.length, 0, n.id);
    w.order.set(p, list);
    treeChanged();
    return treeOf(w, n.id);
  };

  const api = {
    async workspaceCreate(path: string, name: string) {
      calls.push("workspaceCreate");
      if (workspaces.has(path)) throw new WorkspaceFailure(`${path} already has a workspace`);
      const w = build(path, { name });
      workspaces.set(path, w);
      current = w;
      touchRecent(path);
      return info(w);
    },
    async workspaceOpen(path: string) {
      calls.push("workspaceOpen");
      const w = workspaces.get(path);
      if (!w) throw new WorkspaceFailure(`${path} has no ironcall.yaml`);
      current = w;
      touchRecent(path);
      return info(w);
    },
    async workspaceClose() {
      current = null;
    },
    async workspaceCurrent() {
      return current ? info(current) : null;
    },
    async recentWorkspaces(): Promise<RecentWorkspace[]> {
      return recent.map((r) => {
        const w = workspaces.get(r.path);
        return {
          path: r.path,
          name: w?.collection.name ?? r.path.split("/").pop() ?? r.path,
          lastOpened: new Date(r.at).toISOString(),
          available: !!w,
        };
      });
    },
    async forgetRecentWorkspace(path: string) {
      const i = recent.findIndex((r) => r.path === path);
      if (i >= 0) recent.splice(i, 1);
    },

    async itemCreate(parent: ItemId, kind: NewItemKind, name: string, index: number | null) {
      calls.push(`itemCreate:${kind}`);
      const w = ws();
      if (kind === "request") {
        return insertRequest(
          parent,
          { schema: "ironcall/request@1", id: mockId(), name, method: "GET", url: "" },
          index,
        );
      }
      const p = container(parent);
      const id = mockId();
      w.nodes.set(id, {
        id,
        kind: "folder",
        file: slug(name),
        parent: p,
        name,
        folder: { schema: "ironcall/folder@1", id, name },
      });
      w.order.set(id, []);
      const list = w.order.get(p) ?? [];
      list.splice(index ?? list.length, 0, id);
      treeChanged();
      return treeOf(w, id);
    },
    async itemRename(id: ItemId, name: string) {
      const n = node(id);
      n.name = name;
      if (n.request) n.request = { ...n.request, name };
      if (n.folder) n.folder = { ...n.folder, name };
      treeChanged();
    },
    async itemRenameFileToMatch(id: ItemId) {
      const n = node(id);
      n.file = n.kind === "request" ? `${slug(n.name)}.yaml` : slug(n.name);
      treeChanged();
    },
    async itemDuplicate(id: ItemId) {
      const w = ws();
      const n = node(id);
      if (n.kind !== "request" || !n.request)
        throw new WorkspaceFailure("only requests in the mock");
      const copy = { ...clone(n.request), id: mockId(), name: `${n.request.name} copy` };
      const list = w.order.get(n.parent) ?? [];
      return insertRequest(n.parent, copy, list.indexOf(id) + 1);
    },
    async itemDelete(id: ItemId) {
      calls.push("itemDelete");
      const w = ws();
      const drop = (target: ItemId) => {
        for (const child of w.order.get(target) ?? []) drop(child);
        w.order.delete(target);
        w.nodes.delete(target);
      };
      removeFromOrder(w, id);
      drop(id);
      treeChanged();
    },
    async itemMove(id: ItemId, parent: ItemId, index: number) {
      calls.push("itemMove");
      const w = ws();
      const n = node(id);
      const p = container(parent);
      // Can't move a folder into itself or its descendants.
      for (let cur: ItemId | undefined = p; cur; cur = w.nodes.get(cur)?.parent) {
        if (cur === id) throw new WorkspaceFailure("can't move a folder into itself");
      }
      removeFromOrder(w, id);
      n.parent = p;
      const list = w.order.get(p) ?? [];
      list.splice(Math.min(index, list.length), 0, id);
      w.order.set(p, list);
      treeChanged();
    },
    async itemReveal(id: ItemId) {
      calls.push(`itemReveal:${id}`);
    },
    async itemOpenExternal(id: ItemId) {
      calls.push(`itemOpenExternal:${id}`);
    },

    async requestLoad(id: ItemId) {
      return loaded(node(id));
    },
    async requestSave(
      request: RequestDraft,
      baseHash: string | null,
      force: boolean,
    ): Promise<SaveOutcome> {
      calls.push("requestSave");
      const w = ws();
      const n = w.nodes.get(request.id);
      if (!force && baseHash !== null) {
        const diskHash = n?.request ? contentHash(n.request) : null;
        if (diskHash !== baseHash) {
          return { outcome: "conflict", disk: n?.request ? loaded(n) : null };
        }
      }
      if (n?.readOnly) throw new WorkspaceFailure("this file uses a newer format and is read-only");
      if (n) {
        n.request = clone(request);
        n.name = request.name;
      } else {
        // Saving a request deleted on disk recreates it at the top level.
        insertRequest(w.collection.id, request, null);
      }
      treeChanged();
      return { outcome: "saved", hash: contentHash(request) };
    },
    async folderLoad(id: ItemId) {
      const n = node(id);
      if (!n.folder) throw new WorkspaceFailure(`${n.name} is not a folder`);
      return { folder: clone(n.folder), hash: contentHash(n.folder) };
    },
    async folderSave(folder: FolderDraft, baseHash: string | null) {
      calls.push("folderSave");
      const n = node(folder.id);
      if (baseHash !== null && n.folder && contentHash(n.folder) !== baseHash) {
        throw new WorkspaceFailure(CHANGED_ON_DISK, true);
      }
      n.folder = clone(folder);
      n.name = folder.name;
      treeChanged();
      return contentHash(folder);
    },
    async collectionLoad() {
      const w = ws();
      return { collection: clone(w.collection), hash: contentHash(w.collection) };
    },
    async collectionSave(collection: CollectionDraft, baseHash: string | null) {
      calls.push("collectionSave");
      const w = ws();
      if (baseHash !== null && contentHash(w.collection) !== baseHash) {
        throw new WorkspaceFailure(CHANGED_ON_DISK, true);
      }
      w.collection = clone(collection);
      treeChanged();
      return contentHash(collection);
    },

    async scratchList() {
      return [...scratch.values()]
        .sort((a, b) => (scratchTouched.get(b.id) ?? 0) - (scratchTouched.get(a.id) ?? 0))
        .map(clone);
    },
    async scratchSave(request: RequestDraft) {
      calls.push("scratchSave");
      scratch.set(request.id, clone(request));
      scratchTouched.set(request.id, Date.now());
    },
    async scratchDelete(id: ItemId) {
      scratch.delete(id);
      scratchTouched.delete(id);
      delete scratchLayout.placement[id];
    },
    async scratchSaveToWorkspace(request: RequestDraft, parent: ItemId) {
      calls.push("scratchSaveToWorkspace");
      // Like the backend, the workspace copy gets a new ID.
      const node = insertRequest(parent, { ...request, id: mockId() }, null);
      scratch.delete(request.id);
      delete scratchLayout.placement[request.id];
      return node;
    },
    async scratchLayout() {
      return clone(scratchLayout);
    },
    async scratchSaveLayout(layout: ScratchLayout) {
      calls.push("scratchSaveLayout");
      scratchLayout = clone(layout);
      for (const id of Object.keys(scratchLayout.placement)) {
        if (!scratch.has(id)) delete scratchLayout.placement[id];
      }
    },
    async scratchFolderCreate(name: string, parent: ItemId | null) {
      const known = parent !== null && scratchLayout.folders.some((f) => f.id === parent);
      const folder = {
        id: mockId(),
        name: name.trim() || "New folder",
        parent: known ? parent : null,
      };
      scratchLayout.folders.push(folder);
      return clone(folder);
    },
    async sessionLoad() {
      return clone(session);
    },
    async sessionSave(next: Session) {
      session = clone(next);
    },
    async onWorkspaceChanged(cb: (c: WorkspaceChanged) => void) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },

    async pickFolder(_title: string) {
      return dialog.folder;
    },
    async confirm(_message: string) {
      calls.push("confirm");
      return dialog.confirm;
    },
    async choose(_message: string) {
      calls.push("choose");
      return dialog.choose;
    },
    async openUrl(url: string) {
      calls.push(`openUrl:${url}`);
    },
    async onCloseRequested(allow: () => Promise<boolean>) {
      closeHandlers.add(allow);
      return () => closeHandlers.delete(allow);
    },
  };

  /** Test helpers acting as "another program" changing files on disk. */
  const external = {
    dialog,
    calls,
    /** Simulates the user closing the window; resolves whether it would close. */
    async requestClose() {
      for (const allow of closeHandlers) if (!(await allow())) return false;
      return true;
    },
    get session() {
      return session;
    },
    scratch,
    get scratchLayout() {
      return scratchLayout;
    },
    /** Edits a request file on disk and emits a change event. */
    externalEdit(id: ItemId, patch: Partial<RequestDraft>) {
      const n = node(id);
      if (!n.request) throw new Error("not a request");
      n.request = { ...n.request, ...patch };
      emit({ changed: [id], tree: treeOf(ws()) });
    },
    /** Deletes a request file on disk and emits a change event. */
    externalDelete(id: ItemId) {
      const w = ws();
      removeFromOrder(w, id);
      w.nodes.delete(id);
      emit({ removed: [id], tree: treeOf(w) });
    },
    /** The request as stored on disk. */
    diskRequest(id: ItemId): RequestDraft | undefined {
      return current?.nodes.get(id)?.request;
    },
  };

  /** Read access for the mock variables layer (`mockVars.ts`). */
  const internals = {
    /** The open workspace's root, or null. */
    root: () => current?.root ?? null,
    collection: () => current?.collection ?? null,
    setCollection(c: CollectionDraft) {
      if (current) current.collection = clone(c);
    },
    /** True if `id` is a request in the open workspace (not a scratch request). */
    hasRequest: (id: ItemId) => current?.nodes.get(id)?.kind === "request",
    /** Folders containing `id`, outermost first ([] for scratch or top-level requests). */
    folderChain(id: ItemId): FolderDraft[] {
      const w = current;
      if (!w) return [];
      const chain: FolderDraft[] = [];
      let parent = w.nodes.get(id)?.parent;
      while (parent && parent !== w.collection.id) {
        const n = w.nodes.get(parent);
        if (!n) break;
        if (n.folder) chain.unshift(n.folder);
        parent = n.parent;
      }
      return chain;
    },
    folders(): FolderDraft[] {
      return current
        ? [...current.nodes.values()].flatMap((n) => (n.folder ? [n.folder] : []))
        : [];
    },
    emit,
  };

  return { api, external, internals };
}
