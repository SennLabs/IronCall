import { createMemo, createResource, createSignal, For, onMount, Show } from "solid-js";
import { api, type ItemId, type RequestDraft, type ScratchLayout } from "../../ipc/api";
import {
  moveScratchItem,
  removeScratchFolder,
  requestsInFolder,
  type ScratchItem,
  type ScratchRow,
  scratchRows,
} from "../../model/scratchTree";
import type { AppStore } from "../../stores/app";
import { attempt } from "../../stores/notify";
import { ContextMenu, type MenuEntries } from "../common/ContextMenu";
import { MethodBadge } from "../common/MethodBadge";
import styles from "./Sidebar.module.css";
import { RenameInput } from "./TreeView";

/**
 * Saved scratch requests (PLAN.md §4.6), grouped into folders: reopen, delete, create
 * requests and folders from the context menu, and drag items into folders.
 */
export function ScratchList(props: { app: AppStore }) {
  const [open, setOpen] = createSignal(true);
  const [menu, setMenu] = createSignal<{
    x: number;
    y: number;
    label: string;
    items: MenuEntries;
  }>();
  const [layout, setLayout] = createSignal<ScratchLayout>({ folders: [], placement: {} });
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<ItemId>>(new Set());
  const [renaming, setRenaming] = createSignal<ItemId>();
  const [drag, setDrag] = createSignal<ScratchItem>();
  /** Drop target under the pointer: a folder ID, or `null` for the top level. */
  const [over, setOver] = createSignal<ItemId | null>();
  // Refetch whenever the set or names of open tabs change (autosave keeps the list fresh).
  const [items, { refetch }] = createResource(
    () =>
      props.app.tabs
        .tabs()
        .map((t) => `${t.key}:${t.title()}:${t.isScratch()}`)
        .join("|"),
    async () => (await attempt(() => api.scratchList(), "Couldn't list scratch requests")) ?? [],
  );
  const rows = createMemo(() => scratchRows(items() ?? [], layout(), collapsed()));

  onMount(async () => {
    const saved = await attempt(() => api.scratchLayout(), "Couldn't load scratch folders");
    if (saved) setLayout(saved);
  });

  const saveLayout = (next: ScratchLayout) => {
    setLayout(next);
    return attempt(() => api.scratchSaveLayout(next), "Couldn't save scratch folders");
  };

  const expand = (id: ItemId, value = true) => {
    const next = new Set(collapsed());
    if (value) next.delete(id);
    else next.add(id);
    setCollapsed(next);
  };

  const newRequest = async (folder: ItemId | null) => {
    setOpen(true);
    const tab = await props.app.tabs.newScratch();
    if (!folder) return;
    expand(folder);
    await saveLayout({ ...layout(), placement: { ...layout().placement, [tab.key]: folder } });
  };

  const newFolder = async (parent: ItemId | null) => {
    setOpen(true);
    const folder = await attempt(
      () => api.scratchFolderCreate("New folder", parent),
      "Couldn't create the folder",
    );
    if (!folder) return;
    setLayout({ ...layout(), folders: [...layout().folders, folder] });
    if (parent) expand(parent);
    setRenaming(folder.id);
  };

  const renameFolder = (id: ItemId, name: string | null) => {
    setRenaming(undefined);
    const trimmed = name?.trim();
    const current = layout().folders.find((f) => f.id === id);
    if (!trimmed || !current || current.name === trimmed) return;
    void saveLayout({
      ...layout(),
      folders: layout().folders.map((f) => (f.id === id ? { ...f, name: trimmed } : f)),
    });
  };

  /** Closes the request's tab (if open) and deletes it. Resolves false if kept. */
  const deleteRequest = async (id: ItemId) => {
    if (props.app.tabs.tabs().some((t) => t.key === id) && !(await props.app.tabs.close(id))) {
      return false;
    }
    await attempt(() => api.scratchDelete(id), "Couldn't delete it");
    return true;
  };

  const removeRequest = async (id: ItemId) => {
    if (!(await deleteRequest(id))) return;
    const placement = { ...layout().placement };
    delete placement[id];
    setLayout({ ...layout(), placement });
    void refetch();
  };

  const removeFolder = async (id: ItemId, name: string) => {
    const inside = requestsInFolder(layout(), id).filter((r) =>
      (items() ?? []).some((i) => i.id === r),
    );
    if (inside.length > 0) {
      const ok = await api.confirm(
        `Delete the folder "${name}" and the ${inside.length === 1 ? "scratch request" : `${inside.length} scratch requests`} in it? This can't be undone.`,
        { title: "Delete", okLabel: "Delete", kind: "warning" },
      );
      if (!ok) return;
    }
    for (const r of inside) {
      if (!(await deleteRequest(r))) {
        // A tab refused to close: keep the folder so nothing ends up stranded.
        void refetch();
        return;
      }
    }
    await saveLayout(removeScratchFolder(layout(), id));
    void refetch();
  };

  const move = (item: ScratchItem, target: ItemId | null) => {
    const next = moveScratchItem(layout(), item, target);
    if (!next) return;
    if (target) expand(target);
    void saveLayout(next);
  };

  const newEntries = (folder: ItemId | null): MenuEntries => [
    { label: "New request", action: () => void newRequest(folder) },
    { label: "New folder", action: () => void newFolder(folder) },
  ];

  const menuFor = (row: ScratchRow): { label: string; items: MenuEntries } => {
    if (row.kind === "folder") {
      const f = row.folder;
      return {
        label: `${f.name} actions`,
        items: [
          ...newEntries(f.id),
          null,
          { label: "Rename", shortcut: "F2", action: () => setRenaming(f.id) },
          ...(f.parent
            ? [
                {
                  label: "Move to top level",
                  action: () => move({ kind: "folder", id: f.id }, null),
                },
              ]
            : []),
          null,
          { label: "Delete", danger: true, action: () => void removeFolder(f.id, f.name) },
        ],
      };
    }
    const r = row.request;
    return {
      label: "Scratch request actions",
      items: [
        { label: "Open", action: () => props.app.tabs.openScratch(current(r)) },
        ...(row.folder
          ? [
              {
                label: "Move to top level",
                action: () => move({ kind: "request", id: r.id }, null),
              },
            ]
          : []),
        null,
        ...newEntries(row.folder),
        null,
        { label: "Delete", danger: true, action: () => void removeRequest(r.id) },
      ],
    };
  };

  const openMenu = (e: MouseEvent, entry: { label: string; items: MenuEntries }) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, ...entry });
  };

  const sectionMenu = (e: MouseEvent) => {
    // Leave the native menu for the rename input.
    if ((e.target as Element).closest("input")) return;
    openMenu(e, { label: "Scratch actions", items: newEntries(null) });
  };

  /** An open scratch request shows its live draft; the list only refetches on tab changes. */
  const current = (saved: RequestDraft) =>
    props.app.tabs
      .tabs()
      .find((t) => t.key === saved.id)
      ?.request.draft() ?? saved;

  const dropHandlers = (target: ItemId | null) => ({
    onDragOver: (e: DragEvent) => {
      const item = drag();
      if (!item || !moveScratchItem(layout(), item, target)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
      setOver(target);
    },
    onDragLeave: () => over() === target && setOver(undefined),
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const item = drag();
      setDrag(undefined);
      setOver(undefined);
      if (item) move(item, target);
    },
  });

  const dragHandlers = (item: ScratchItem) => ({
    draggable: renaming() !== item.id,
    onDragStart: (e: DragEvent) => {
      setDrag(item);
      e.dataTransfer?.setData("text/plain", item.id);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
    },
    onDragEnd: () => {
      setDrag(undefined);
      setOver(undefined);
    },
  });

  const indent = (depth: number) => ({ "padding-left": `calc(var(--space-4) + ${depth * 14}px)` });

  // Rows are rebuilt whenever the list or layout changes, so they needn't be reactive.
  const requestRow = (row: Extract<ScratchRow, { kind: "request" }>) => {
    const r = () => current(row.request);
    return (
      <button
        type="button"
        class={styles.scratchItem}
        classList={{ [styles.active as string]: props.app.tabs.active()?.key === row.request.id }}
        style={indent(row.depth)}
        onClick={() => props.app.tabs.openScratch(r())}
        onContextMenu={(e) => openMenu(e, menuFor(row))}
        {...dragHandlers({ kind: "request", id: row.request.id })}
      >
        <MethodBadge method={r().method} />
        <span class={styles.scratchName}>{r().url || r().name}</span>
      </button>
    );
  };

  const folderRow = (row: Extract<ScratchRow, { kind: "folder" }>) => {
    const f = row.folder;
    return (
      <Show
        when={renaming() !== f.id}
        fallback={
          <div class={styles.scratchItem} style={indent(row.depth)}>
            <span class={styles.twisty} aria-hidden="true">
              ▸
            </span>
            <RenameInput initial={f.name} onDone={(name) => renameFolder(f.id, name)} />
          </div>
        }
      >
        <button
          type="button"
          class={styles.scratchItem}
          classList={{ [styles.dropInside as string]: over() === f.id }}
          style={indent(row.depth)}
          aria-expanded={row.expanded}
          aria-label={f.name}
          onClick={() => expand(f.id, !row.expanded)}
          onKeyDown={(e) => {
            if (e.key === "F2") {
              e.preventDefault();
              setRenaming(f.id);
            }
          }}
          onContextMenu={(e) => openMenu(e, menuFor(row))}
          {...dragHandlers({ kind: "folder", id: f.id })}
          {...dropHandlers(f.id)}
        >
          <span class={styles.twisty} aria-hidden="true">
            {row.expanded ? "▾" : "▸"}
          </span>
          <span class={styles.scratchName}>{f.name}</span>
          <span class={styles.count}>{row.count}</span>
        </button>
      </Show>
    );
  };

  return (
    <section class={styles.scratch} aria-label="Scratch requests" onContextMenu={sectionMenu}>
      <button
        type="button"
        class={styles.sectionHeader}
        classList={{ [styles.dropInside as string]: over() === null }}
        aria-expanded={open()}
        onClick={() => setOpen(!open())}
        {...dropHandlers(null)}
      >
        <span aria-hidden="true">{open() ? "▾" : "▸"}</span> Scratch
        <span class={styles.count}>{items()?.length ?? 0}</span>
      </button>
      <Show when={open()}>
        <ul class={styles.scratchList}>
          <For each={rows()}>
            {(row) => <li>{row.kind === "folder" ? folderRow(row) : requestRow(row)}</li>}
          </For>
          <li>
            <button
              type="button"
              class={`ghost ${styles.newScratch}`}
              onClick={() => void props.app.tabs.newScratch()}
            >
              + New scratch request <kbd>Ctrl+N</kbd>
            </button>
          </li>
        </ul>
      </Show>
      <Show when={menu()}>
        {(m) => (
          <ContextMenu
            x={m().x}
            y={m().y}
            label={m().label}
            items={m().items}
            onClose={() => setMenu(undefined)}
          />
        )}
      </Show>
    </section>
  );
}
