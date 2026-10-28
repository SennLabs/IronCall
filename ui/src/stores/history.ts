/**
 * History panel state (PLAN.md Phase 5): a paged, filtered list of past sends, the
 * selected entry's detail, and actions (open in a new tab, resend, delete, clear).
 */

import { batch, createSignal } from "solid-js";
import {
  api,
  type HistoryEntryDraft,
  type HistoryQuery,
  type HistorySummary,
  type ItemId,
  type RequestDraft,
  type StatusClass,
} from "../ipc/api";
import { attempt, notify } from "./notify";

export const HISTORY_PAGE_SIZE = 50;
export const HISTORY_SEARCH_DEBOUNCE_MS = 250;

export type HistoryScope = "workspace" | "all";

export interface HistoryStoreOptions {
  /** ID of the open workspace (its collection ID), or `null`. */
  workspaceId: () => ItemId | null;
  pageSize?: number;
  debounceMs?: number;
}

/**
 * A copy of an entry's authored request with a fresh ID, for a new scratch tab. The
 * `{{variables}}` are kept, so it resolves against the current environment.
 */
export async function requestForNewTab(entry: HistoryEntryDraft): Promise<RequestDraft> {
  const fresh = await api.newScratchRequest();
  return { ...entry.request, id: fresh.id, schema: fresh.schema };
}

export function createHistoryStore(options: HistoryStoreOptions) {
  const pageSize = options.pageSize ?? HISTORY_PAGE_SIZE;
  const [open, setOpen] = createSignal(false);
  const [text, setTextRaw] = createSignal("");
  const [scope, setScopeRaw] = createSignal<HistoryScope>("workspace");
  const [methods, setMethods] = createSignal<string[]>([]);
  const [statuses, setStatuses] = createSignal<StatusClass[]>([]);
  const [entries, setEntries] = createSignal<HistorySummary[]>([]);
  const [next, setNext] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [selected, setSelected] = createSignal<string>();
  const [detail, setDetail] = createSignal<HistoryEntryDraft>();
  /** True if new sends happened while the panel was closed. */
  let stale = true;
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const query = (before: string | null): HistoryQuery => ({
    text: text(),
    workspace: scope() === "workspace" ? options.workspaceId() : null,
    methods: methods(),
    status: statuses(),
    before,
    limit: pageSize,
  });

  /** Loads the first page again (filters changed, or new entries). */
  const reload = async () => {
    clearTimeout(timer);
    const mine = ++generation;
    setLoading(true);
    const page = await attempt(() => api.historyList(query(null)), "Couldn't load history");
    if (mine !== generation) return;
    batch(() => {
      setLoading(false);
      if (!page) return;
      setEntries(page.entries);
      setNext(page.next);
      stale = false;
    });
  };

  const scheduleReload = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void reload(), options.debounceMs ?? HISTORY_SEARCH_DEBOUNCE_MS);
  };

  const store = {
    open,
    text,
    scope,
    methods,
    statuses,
    entries,
    next,
    loading,
    selected,
    detail,

    setOpen(v: boolean) {
      setOpen(v);
      if (v && stale) void reload();
    },
    toggle() {
      store.setOpen(!open());
    },
    reload,
    setText(v: string) {
      setTextRaw(v);
      scheduleReload();
    },
    setScope(v: HistoryScope) {
      setScopeRaw(v);
      void reload();
    },
    toggleMethod(m: string) {
      setMethods((list) => (list.includes(m) ? list.filter((x) => x !== m) : [...list, m]));
      void reload();
    },
    toggleStatus(s: StatusClass) {
      setStatuses((list) => (list.includes(s) ? list.filter((x) => x !== s) : [...list, s]));
      void reload();
    },

    /** Loads the next page (infinite scroll). */
    async loadMore() {
      const before = next();
      if (!before || loading()) return;
      const mine = generation;
      setLoading(true);
      const page = await attempt(() => api.historyList(query(before)), "Couldn't load history");
      if (mine !== generation) return;
      batch(() => {
        setLoading(false);
        if (!page) return;
        setEntries((list) => [
          ...list,
          ...page.entries.filter((e) => !list.some((x) => x.id === e.id)),
        ]);
        setNext(page.next);
      });
    },

    async select(id: string | undefined) {
      setSelected(id);
      setDetail(undefined);
      if (!id) return;
      const entry = await attempt(() => api.historyGet(id), "Couldn't load the history entry");
      if (entry && selected() === id) setDetail(entry);
    },

    async remove(id: string) {
      try {
        await api.historyDelete(id);
      } catch (e) {
        notify(`Couldn't delete the entry: ${e instanceof Error ? e.message : e}`, "error");
        return;
      }
      batch(() => {
        setEntries((list) => list.filter((e) => e.id !== id));
        if (selected() === id) {
          setSelected(undefined);
          setDetail(undefined);
        }
      });
    },

    /** Clears all history, or only the open workspace's (after asking). */
    async clear(which: HistoryScope) {
      const workspace = which === "workspace" ? options.workspaceId() : null;
      const ok = await api.confirm(
        workspace
          ? "Delete this workspace's request history? Other workspaces' history is kept."
          : "Delete all request history, for every workspace?",
        { title: "Clear history", okLabel: "Delete", kind: "warning" },
      );
      if (!ok) return;
      await attempt(() => api.historyClear(workspace), "Couldn't clear history");
      batch(() => {
        setSelected(undefined);
        setDetail(undefined);
      });
      await reload();
    },

    /** A send finished (and was recorded): show it at the top if the panel is open. */
    sendRecorded() {
      stale = true;
      if (open()) void reload();
    },

    /** The open workspace changed: "this workspace" means something else now. */
    workspaceChanged() {
      stale = true;
      if (open()) void reload();
    },
  };
  return store;
}

export type HistoryStore = ReturnType<typeof createHistoryStore>;
