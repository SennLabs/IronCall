/**
 * Open tabs (PLAN.md Phase 2). Each tab owns its request draft, send state, preview,
 * undo history and save state, created in its own reactive root and disposed on close.
 *
 * - Workspace request tabs save explicitly (Ctrl+S) with the hash they were loaded with;
 *   a changed file on disk becomes a conflict instead of being overwritten.
 * - Scratch tabs autosave (debounced) to the app data dir.
 * - Single-clicking a request opens a *preview* tab that the next preview replaces;
 *   editing, double-clicking or saving pins it (as in VS Code).
 */

import { type Accessor, batch, createEffect, createRoot, createSignal, on } from "solid-js";
import {
  api,
  type ItemId,
  type LoadedRequest,
  type RequestDraft,
  type Session,
  type TabRef,
  type WorkspaceChanged,
} from "../ipc/api";
import { toDisplayText } from "../model/diff";
import { createHistory, type History } from "../model/history";
import { attempt, notify } from "./notify";
import { createPreview } from "./preview";
import { createRequestStore, type RequestStore } from "./request";
import { createSendStore, type SendStore } from "./send";
import { createVariables, type VariablesStore } from "./variables";

export const SCRATCH_AUTOSAVE_MS = 400;

export type TabStatus =
  | { kind: "ok" }
  /** The file changed on disk while this tab had unsaved edits (or a save was refused). */
  | { kind: "conflict"; disk: LoadedRequest | null }
  /** The file was deleted on disk. Saving recreates it. */
  | { kind: "deleted" };

export interface Tab {
  /**
   * Unique within the tab list: the current request or scratch ID. Follows `ref`, since
   * saving a scratch request into the workspace may give it a new ID.
   */
  readonly key: string;
  ref: Accessor<TabRef>;
  request: RequestStore;
  sender: SendStore;
  preview: ReturnType<typeof createPreview>;
  /** Variables visible to this request (for highlighting, autocomplete, hover). */
  vars: VariablesStore;
  status: Accessor<TabStatus>;
  readOnly: Accessor<boolean>;
  pinned: Accessor<boolean>;
  /** Unsaved changes (workspace requests only; scratch tabs autosave). */
  dirty: () => boolean;
  isScratch: () => boolean;
  title: () => string;
  /** @internal */ state: TabInternals;
}

interface TabInternals {
  history: History<RequestDraft>;
  saved: Accessor<RequestDraft>;
  setSaved(d: RequestDraft): void;
  baseHash: Accessor<string | null>;
  setBaseHash(h: string | null): void;
  setStatus(s: TabStatus): void;
  setReadOnly(v: boolean): void;
  setPinned(v: boolean): void;
  setRef(r: TabRef): void;
  /** Loads a draft without recording undo history or pinning. */
  apply(d: RequestDraft): void;
  flushAutosave(): Promise<void>;
  dispose(): void;
}

const same = (a: RequestDraft, b: RequestDraft) => JSON.stringify(a) === JSON.stringify(b);

function createTab(
  ref: TabRef,
  draft: RequestDraft,
  loaded: { hash: string | null; readOnly: boolean },
  pinned: boolean,
  version: Accessor<number>,
): Tab {
  return createRoot((dispose) => {
    const request = createRequestStore(draft);
    const sender = createSendStore();
    const preview = createPreview(request.draft, undefined, version);
    const vars = createVariables(request.draft, version);
    const [refSignal, setRef] = createSignal<TabRef>(ref);
    const [saved, setSaved] = createSignal<RequestDraft>(draft);
    const [baseHash, setBaseHash] = createSignal<string | null>(loaded.hash);
    const [status, setStatus] = createSignal<TabStatus>({ kind: "ok" });
    const [readOnly, setReadOnly] = createSignal(loaded.readOnly);
    const [isPinned, setPinned] = createSignal(pinned);
    const history = createHistory(draft);
    const isScratch = () => refSignal().kind === "scratch";

    let applying = false;
    const apply = (d: RequestDraft) => {
      applying = true;
      request.load(d);
      applying = false;
    };

    let timer: ReturnType<typeof setTimeout> | undefined;
    let pending: RequestDraft | undefined;
    // Saves run one after another, so an older draft can never land after a newer one.
    let saving: Promise<unknown> = Promise.resolve();
    const flushAutosave = () => {
      clearTimeout(timer);
      const d = pending;
      pending = undefined;
      if (d) {
        saving = saving.then(() =>
          attempt(() => api.scratchSave(d), "Couldn't save the scratch request"),
        );
      }
      return saving.then(() => undefined);
    };

    // Every user edit: record undo history, pin the tab, and autosave scratch requests.
    createEffect(
      on(
        request.draft,
        (d) => {
          if (applying) return;
          history.record(d);
          setPinned(true);
          if (isScratch()) {
            pending = d;
            clearTimeout(timer);
            timer = setTimeout(() => void flushAutosave(), SCRATCH_AUTOSAVE_MS);
          }
        },
        { defer: true },
      ),
    );

    const tab: Tab = {
      get key() {
        return refSignal().id;
      },
      ref: refSignal,
      request,
      sender,
      preview,
      vars,
      status,
      readOnly,
      pinned: isPinned,
      dirty: () => !isScratch() && !same(request.draft(), saved()),
      isScratch,
      title: () => request.draft().name || "Untitled request",
      state: {
        history,
        saved,
        setSaved,
        baseHash,
        setBaseHash,
        setStatus,
        setReadOnly,
        setPinned,
        setRef,
        apply,
        flushAutosave,
        dispose() {
          clearTimeout(timer);
          dispose();
        },
      },
    };
    return tab;
  });
}

export type CloseChoice = "save" | "discard" | "cancel";

export interface TabsStore {
  tabs: Accessor<Tab[]>;
  active: Accessor<Tab | undefined>;
  activate(key: string): void;
  next(step?: 1 | -1): void;
  /** Opens (or focuses) a workspace request. `pin: false` opens a preview tab. */
  openRequest(id: ItemId, options?: { pin?: boolean }): Promise<Tab | undefined>;
  newScratch(): Promise<Tab>;
  openScratch(draft: RequestDraft): Tab;
  pin(key: string): void;
  /** Closes a tab, asking about unsaved changes. Resolves false if cancelled. */
  close(key: string): Promise<boolean>;
  closeOthers(key: string): Promise<void>;
  closeAll(): Promise<boolean>;
  /**
   * Before the window closes: saves scratch requests and asks Save / Don't save / Cancel
   * for each unsaved workspace tab. Tabs stay open (the session restores them). False if
   * the user cancelled.
   */
  beforeQuit(): Promise<boolean>;
  /** Closes every workspace request tab (e.g. when switching workspace). */
  closeWorkspaceTabs(): Promise<boolean>;
  save(key?: string): Promise<boolean>;
  keepMine(key: string): Promise<void>;
  takeTheirs(key: string): Promise<void>;
  /** Display texts for the Compare view. */
  compareTexts(key: string): { mine: string; theirs: string } | undefined;
  saveToWorkspace(key: string, parent: ItemId): Promise<boolean>;
  /**
   * Replaces a tab's whole request as one undoable edit (e.g. pasting a cURL command);
   * Ctrl+Z restores the previous request.
   */
  replaceRequest(key: string, draft: RequestDraft): void;
  undo(key?: string): void;
  redo(key?: string): void;
  handleChange(change: WorkspaceChanged): Promise<void>;
  /**
   * IronCall renamed a request (the watcher ignores our own writes). A clean tab
   * reloads; a dirty one takes the new name and file hash and keeps its edits, so the
   * next save isn't a false conflict.
   */
  renamed(id: ItemId): Promise<void>;
  /** IronCall moved requests to the trash: their tabs show as deleted. */
  removed(ids: ItemId[]): void;
  session(workspace: string | null): Session;
  restore(session: Session): Promise<void>;
  findByRequest(id: ItemId): Tab | undefined;
}

export interface TabsOptions {
  /**
   * Called after IronCall writes a workspace file. The file watcher ignores our own
   * writes, so this is how the tree learns about e.g. a changed method.
   */
  onWrite?: () => void;
  /** Bumps when resolved variable values may have changed (see `stores/vars.ts`). */
  varsVersion?: Accessor<number>;
}

export function createTabsStore(options: TabsOptions = {}): TabsStore {
  const version = options.varsVersion ?? (() => 0);
  const [tabs, setTabs] = createSignal<Tab[]>([]);
  const [activeKey, setActiveKey] = createSignal<string>();

  const byKey = (key: string | undefined) => tabs().find((t) => t.key === key);
  const active = () => byKey(activeKey()) ?? tabs()[0];
  const resolve = (key?: string) => (key ? byKey(key) : active());

  const add = (tab: Tab, after?: Tab) => {
    batch(() => {
      setTabs((list) => {
        const i = after ? list.indexOf(after) : -1;
        return i >= 0 ? [...list.slice(0, i + 1), tab, ...list.slice(i + 1)] : [...list, tab];
      });
      setActiveKey(tab.key);
    });
    return tab;
  };

  const remove = (tab: Tab) => {
    const list = tabs();
    const i = list.indexOf(tab);
    batch(() => {
      setTabs(list.filter((t) => t !== tab));
      if (activeKey() === tab.key) setActiveKey((list[i + 1] ?? list[i - 1])?.key);
    });
    tab.state.dispose();
  };

  const markSaved = (tab: Tab, draft: RequestDraft, hash: string) => {
    batch(() => {
      tab.state.setSaved(draft);
      tab.state.setBaseHash(hash);
      tab.state.setStatus({ kind: "ok" });
      tab.state.setPinned(true);
    });
  };

  const reload = (tab: Tab, loaded: LoadedRequest) => {
    batch(() => {
      tab.state.apply(loaded.request);
      tab.state.history.reset(loaded.request);
      tab.state.setSaved(loaded.request);
      tab.state.setBaseHash(loaded.hash);
      tab.state.setReadOnly(loaded.readOnly);
      tab.state.setStatus({ kind: "ok" });
    });
  };

  const write = async (tab: Tab, force: boolean): Promise<boolean> => {
    if (tab.readOnly()) {
      notify("This file uses a newer IronCall format and is read-only.", "error");
      return false;
    }
    const draft = tab.request.draft();
    const deleted = tab.status().kind === "deleted";
    const outcome = await attempt(
      () => api.requestSave(draft, deleted ? null : tab.state.baseHash(), force),
      "Couldn't save",
    );
    if (!outcome) return false;
    if (outcome.outcome === "conflict") {
      tab.state.setStatus({ kind: "conflict", disk: outcome.disk });
      return false;
    }
    markSaved(tab, draft, outcome.hash);
    options.onWrite?.();
    return true;
  };

  const closeTab = async (tab: Tab): Promise<boolean> => {
    if (tab.isScratch()) {
      await tab.state.flushAutosave();
      remove(tab);
      return true;
    }
    if (tab.dirty()) {
      setActiveKey(tab.key);
      const choice = await api.choose(`Save changes to "${tab.title()}" before closing?`, {
        title: "Unsaved changes",
        yes: "Save",
        no: "Don't save",
        cancel: "Cancel",
      });
      if (choice === "cancel") return false;
      if (choice === "yes" && !(await write(tab, false))) return false;
    }
    remove(tab);
    return true;
  };

  const store: TabsStore = {
    tabs,
    active,
    activate: (key) => setActiveKey(key),
    next(step = 1) {
      const list = tabs();
      if (list.length < 2) return;
      const i = list.findIndex((t) => t.key === active()?.key);
      setActiveKey(list[(i + step + list.length) % list.length]?.key);
    },

    async openRequest(id, options = {}) {
      const pin = options.pin ?? true;
      const existing = byKey(id);
      if (existing) {
        setActiveKey(existing.key);
        if (pin) existing.state.setPinned(true);
        return existing;
      }
      const loaded = await attempt(() => api.requestLoad(id), "Couldn't open the request");
      if (!loaded) return undefined;
      // A second click may have opened it while we were loading.
      const raced = byKey(id);
      if (raced) {
        setActiveKey(raced.key);
        return raced;
      }
      const tab = createTab({ kind: "request", id }, loaded.request, loaded, pin, version);
      // A preview tab replaces the previous (clean, unpinned) preview tab in place.
      const previous = tabs().find((t) => !t.pinned() && !t.dirty() && !t.isScratch());
      if (!pin && previous) {
        setTabs((list) => list.map((t) => (t === previous ? tab : t)));
        setActiveKey(tab.key);
        previous.state.dispose();
        return tab;
      }
      return add(tab, active());
    },

    async newScratch() {
      const draft = await api.newScratchRequest();
      // Saved first, so the sidebar's scratch list (refetched when tabs change) has it.
      await attempt(() => api.scratchSave(draft), "Couldn't save the scratch request");
      return store.openScratch(draft);
    },

    openScratch(draft) {
      const existing = byKey(draft.id);
      if (existing) {
        setActiveKey(existing.key);
        return existing;
      }
      return add(
        createTab(
          { kind: "scratch", id: draft.id },
          draft,
          { hash: null, readOnly: false },
          true,
          version,
        ),
      );
    },

    pin(key) {
      byKey(key)?.state.setPinned(true);
    },

    async close(key) {
      const tab = byKey(key);
      return tab ? closeTab(tab) : true;
    },
    async closeOthers(key) {
      for (const tab of tabs().filter((t) => t.key !== key)) {
        if (!(await closeTab(tab))) return;
      }
      setActiveKey(key);
    },
    async closeAll() {
      for (const tab of [...tabs()]) {
        if (!(await closeTab(tab))) return false;
      }
      return true;
    },
    async beforeQuit() {
      for (const tab of tabs()) {
        if (tab.isScratch()) {
          await tab.state.flushAutosave();
          continue;
        }
        if (!tab.dirty()) continue;
        setActiveKey(tab.key);
        const choice = await api.choose(`Save changes to "${tab.title()}" before quitting?`, {
          title: "Unsaved changes",
          yes: "Save",
          no: "Don't save",
          cancel: "Cancel",
        });
        if (choice === "cancel") return false;
        if (choice === "yes" && !(await write(tab, false))) return false;
      }
      return true;
    },
    async closeWorkspaceTabs() {
      for (const tab of tabs().filter((t) => !t.isScratch())) {
        if (!(await closeTab(tab))) return false;
      }
      return true;
    },

    async save(key) {
      const tab = resolve(key);
      if (!tab) return false;
      if (tab.isScratch()) {
        await tab.state.flushAutosave();
        return true;
      }
      return write(tab, false);
    },
    async keepMine(key) {
      const tab = byKey(key);
      if (tab) await write(tab, true);
    },
    async takeTheirs(key) {
      const tab = byKey(key);
      if (!tab) return;
      const status = tab.status();
      const disk =
        status.kind === "conflict" && status.disk
          ? status.disk
          : await attempt(() => api.requestLoad(tab.key), "Couldn't reload");
      if (disk) reload(tab, disk);
    },
    compareTexts(key) {
      const tab = byKey(key);
      const status = tab?.status();
      if (!tab || status?.kind !== "conflict") return undefined;
      return {
        mine: toDisplayText(tab.request.draft()),
        theirs: status.disk ? toDisplayText(status.disk.request) : "(deleted on disk)",
      };
    },

    async saveToWorkspace(key, parent) {
      const tab = byKey(key);
      if (!tab?.isScratch()) return false;
      await tab.state.flushAutosave();
      const draft = tab.request.draft();
      const node = await attempt(
        () => api.scratchSaveToWorkspace(draft, parent),
        "Couldn't save to the workspace",
      );
      if (!node) return false;
      const loaded = await attempt(() => api.requestLoad(node.id), "Couldn't reload");
      const wasActive = active() === tab;
      batch(() => {
        tab.state.setRef({ kind: "request", id: node.id });
        if (wasActive) setActiveKey(node.id);
        if (loaded) reload(tab, loaded);
        else markSaved(tab, draft, "");
      });
      options.onWrite?.();
      return true;
    },

    replaceRequest(key, draft) {
      const tab = byKey(key);
      if (!tab) return;
      // Its own undo step: don't merge with typing just before or after.
      tab.state.history.breakBurst();
      tab.request.load(draft);
      tab.state.history.breakBurst();
    },

    undo(key) {
      const tab = resolve(key);
      const prev = tab?.state.history.undo();
      if (tab && prev) tab.state.apply(prev);
    },
    redo(key) {
      const tab = resolve(key);
      const next = tab?.state.history.redo();
      if (tab && next) tab.state.apply(next);
    },

    async handleChange(change) {
      for (const id of change.removed) {
        const tab = byKey(id);
        if (tab && !tab.isScratch()) tab.state.setStatus({ kind: "deleted" });
      }
      for (const id of change.changed) {
        const tab = byKey(id);
        if (!tab || tab.isScratch()) continue;
        const disk = await attempt(() => api.requestLoad(id), "Couldn't reload a changed file");
        if (!disk) continue;
        // Our own save can echo back as a change; nothing to do if the content matches.
        if (disk.hash === tab.state.baseHash()) continue;
        if (tab.dirty()) tab.state.setStatus({ kind: "conflict", disk });
        else reload(tab, disk);
      }
    },

    async renamed(id) {
      const tab = byKey(id);
      if (!tab || tab.isScratch()) return;
      const disk = await attempt(() => api.requestLoad(id), "Couldn't reload the renamed request");
      if (!disk) return;
      if (!tab.dirty()) {
        reload(tab, disk);
        return;
      }
      tab.state.apply({ ...tab.request.draft(), name: disk.request.name });
      tab.state.setSaved(disk.request);
      tab.state.setBaseHash(disk.hash);
    },

    removed(ids) {
      for (const id of ids) {
        const tab = byKey(id);
        if (tab && !tab.isScratch()) tab.state.setStatus({ kind: "deleted" });
      }
    },

    session(workspace) {
      // Preview tabs are transient and not restored.
      const kept = tabs().filter((t) => t.pinned() || t.isScratch());
      const i = kept.findIndex((t) => t.key === active()?.key);
      return { workspace, tabs: kept.map((t) => t.ref()), active: i >= 0 ? i : null };
    },

    async restore(session) {
      const scratch = new Map(
        ((await attempt(() => api.scratchList(), "")) ?? []).map((r) => [r.id, r]),
      );
      for (const ref of session.tabs) {
        if (ref.kind === "scratch") {
          const draft = scratch.get(ref.id);
          if (draft) add(createTab(ref, draft, { hash: null, readOnly: false }, true, version));
          continue;
        }
        // Missing requests (deleted or renamed away) are skipped quietly.
        const loaded = await api.requestLoad(ref.id).catch(() => undefined);
        if (loaded) add(createTab(ref, loaded.request, loaded, true, version));
      }
      const target = session.active !== null ? session.tabs[session.active] : undefined;
      if (target && byKey(target.id)) setActiveKey(target.id);
    },

    findByRequest: (id) => byKey(id),
  };
  return store;
}
