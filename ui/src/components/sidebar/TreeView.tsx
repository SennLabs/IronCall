import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type { ItemId, TreeNode } from "../../ipc/api";
import {
  type DropPosition,
  dropPosition,
  dropTarget,
  filterRows,
  findNode,
  flatten,
  type TreeRow,
} from "../../model/tree";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { ContextMenu, type MenuEntries } from "../common/ContextMenu";
import { MethodBadge } from "../common/MethodBadge";
import styles from "./TreeView.module.css";

export const ROW_HEIGHT = 24;
const OVERSCAN = 10;
/** Rows rendered when the container has no measurable height (e.g. in tests). */
const FALLBACK_ROWS = 200;

export interface TreeViewProps {
  app: AppStore;
  tree: TreeNode;
  filter: string;
}

/** Highlights matched characters of a name. */
function Highlighted(props: { text: string; indices?: number[] }) {
  const parts = createMemo(() => {
    const set = new Set(props.indices ?? []);
    if (set.size === 0) return [{ text: props.text, hit: false }];
    const out: { text: string; hit: boolean }[] = [];
    for (let i = 0; i < props.text.length; i++) {
      const hit = set.has(i);
      const last = out[out.length - 1];
      if (last && last.hit === hit) last.text += props.text[i];
      else out.push({ text: props.text[i] as string, hit });
    }
    return out;
  });
  return <For each={parts()}>{(p) => (p.hit ? <mark>{p.text}</mark> : p.text)}</For>;
}

/**
 * The workspace tree: virtualised rows, keyboard navigation (arrows, Enter, F2, Delete,
 * context-menu key), inline rename, drag and drop, and context menus.
 */
export function TreeView(props: TreeViewProps) {
  const ws = () => props.app.workspace;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(0);
  const [menu, setMenu] = createSignal<{ x: number; y: number; node: TreeNode }>();
  const [drag, setDrag] = createSignal<ItemId>();
  const [over, setOver] = createSignal<{ id: ItemId; position: DropPosition }>();
  let scroller!: HTMLDivElement;

  const urlOf = (id: ItemId) => props.app.tabs.findByRequest(id)?.request.draft().url;
  const rows = createMemo<TreeRow[]>(() =>
    props.filter.trim()
      ? filterRows(props.tree, props.filter, urlOf)
      : flatten(props.tree, ws().expanded()),
  );

  const range = createMemo(() => {
    const total = rows().length;
    if (height() === 0) return { start: 0, end: Math.min(total, FALLBACK_ROWS) };
    const start = Math.max(0, Math.floor(scrollTop() / ROW_HEIGHT) - OVERSCAN);
    const end = Math.min(total, Math.ceil((scrollTop() + height()) / ROW_HEIGHT) + OVERSCAN);
    return { start, end };
  });
  const visible = createMemo(() => rows().slice(range().start, range().end));

  onMount(() => {
    setHeight(scroller.clientHeight);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setHeight(scroller.clientHeight));
    ro.observe(scroller);
    onCleanup(() => ro.disconnect());
  });

  const scrollIntoView = (id: ItemId) => {
    const i = rows().findIndex((r) => r.node.id === id);
    if (i < 0 || height() === 0) return;
    const top = i * ROW_HEIGHT;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + ROW_HEIGHT > scroller.scrollTop + height()) {
      scroller.scrollTop = top + ROW_HEIGHT - height();
    }
  };

  const select = (id: ItemId) => {
    ws().setSelected(id);
    scrollIntoView(id);
  };

  const activate = (node: TreeNode, pin: boolean) => {
    ws().setSelected(node.id);
    if (node.kind === "request") void props.app.openRequest(node.id, pin);
    else if (node.kind === "folder") ws().toggle(node.id);
  };

  /** Folder or collection a "new …" action should create in. */
  const containerFor = (node: TreeNode): ItemId => {
    if (node.kind === "collection" || node.kind === "folder") return node.id;
    return rows().find((r) => r.node.id === node.id)?.parentId ?? props.tree.id;
  };

  const menuItems = (node: TreeNode): MenuEntries => {
    const w = ws();
    const newItems: MenuEntries = [
      { label: "New request", action: () => void w.createItem(containerFor(node), "request") },
      { label: "New folder", action: () => void w.createItem(containerFor(node), "folder") },
    ];
    const reveal = { label: "Reveal in file manager", action: () => void w.reveal(node.id) };
    const del = {
      label: "Delete",
      shortcut: "Del",
      danger: true,
      action: () => void w.remove(node.id),
    };
    switch (node.kind) {
      case "collection":
        return [
          ...newItems,
          null,
          {
            label: "Edit description…",
            action: () => openDialog({ kind: "description", target: { kind: "collection" } }),
          },
          {
            label: "Collection variables…",
            action: () => openDialog({ kind: "variables", target: { kind: "collection" } }),
          },
          {
            label: "Auth & default headers…",
            action: () => openDialog({ kind: "containerSettings", target: { kind: "collection" } }),
          },
          { label: "Environments…", action: () => openDialog({ kind: "environments" }) },
          null,
          { label: "Run collection…", action: () => void props.app.openRunner(null, node.name) },
          reveal,
        ];
      case "folder":
        return [
          ...newItems,
          null,
          { label: "Rename", shortcut: "F2", action: () => w.setRenaming(node.id) },
          {
            label: "Edit description…",
            action: () =>
              openDialog({ kind: "description", target: { kind: "folder", id: node.id } }),
          },
          {
            label: "Folder variables…",
            action: () =>
              openDialog({
                kind: "variables",
                target: { kind: "folder", id: node.id, name: node.name },
              }),
          },
          {
            label: "Auth & default headers…",
            action: () =>
              openDialog({
                kind: "containerSettings",
                target: { kind: "folder", id: node.id, name: node.name },
              }),
          },
          { label: "Rename folder to match name", action: () => void w.renameFileToMatch(node.id) },
          { label: "Duplicate", action: () => void w.duplicate(node.id) },
          null,
          { label: "Run folder…", action: () => void props.app.openRunner(node.id, node.name) },
          reveal,
          del,
        ];
      case "request":
        return [
          { label: "Open", shortcut: "Enter", action: () => activate(node, true) },
          {
            label: "Copy as cURL…",
            action: () => openDialog({ kind: "copyCurl", source: { requestId: node.id } }),
          },
          null,
          {
            label: "Rename",
            shortcut: "F2",
            action: () => w.setRenaming(node.id),
            disabled: node.readOnly,
          },
          { label: "Duplicate", action: () => void w.duplicate(node.id) },
          { label: "Rename file to match name", action: () => void w.renameFileToMatch(node.id) },
          null,
          ...newItems,
          null,
          reveal,
          del,
        ];
      case "error":
        return [
          { label: "Open in external editor", action: () => void w.openExternal(node.id) },
          reveal,
        ];
    }
  };

  const openMenu = (node: TreeNode, x: number, y: number) => {
    ws().setSelected(node.id);
    setMenu({ x, y, node });
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (ws().renaming()) return;
    const list = rows();
    const i = list.findIndex((r) => r.node.id === ws().selected());
    const row = list[i];
    const go = (j: number) => {
      const target = list[Math.max(0, Math.min(list.length - 1, j))];
      if (target) select(target.node.id);
    };
    switch (e.key) {
      case "ArrowDown":
        go(i + 1);
        break;
      case "ArrowUp":
        go(i < 0 ? 0 : i - 1);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(list.length - 1);
        break;
      case "ArrowRight":
        if (row?.node.kind === "folder" && !row.expanded) ws().expand(row.node.id);
        else if (row?.hasChildren) go(i + 1);
        break;
      case "ArrowLeft":
        if (row?.node.kind === "folder" && row.expanded) ws().expand(row.node.id, false);
        else if (row?.parentId) select(row.parentId);
        break;
      case "Enter":
        if (row) activate(row.node, true);
        break;
      case "F2":
        if (row && row.node.kind !== "collection" && row.node.kind !== "error")
          ws().setRenaming(row.node.id);
        break;
      case "Delete":
        if (row && row.node.kind !== "collection") void ws().remove(row.node.id);
        break;
      case "ContextMenu": {
        if (!row) return;
        const el = scroller.querySelector(`[data-id="${row.node.id}"]`)?.getBoundingClientRect();
        openMenu(row.node, el?.left ?? 0, el?.bottom ?? 0);
        break;
      }
      default:
        if (e.key === "F10" && e.shiftKey && row) {
          openMenu(row.node, 0, 0);
          break;
        }
        return;
    }
    e.preventDefault();
  };

  const onDragOver = (e: DragEvent, row: TreeRow) => {
    const dragged = drag();
    if (!dragged) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const container = row.node.kind === "folder" || row.node.kind === "collection";
    const position = dropPosition(e.clientY - rect.top, rect.height || ROW_HEIGHT, container);
    if (dropTarget(props.tree, dragged, row.node.id, position)) {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
      setOver({ id: row.node.id, position });
    } else {
      setOver(undefined);
    }
  };

  const onDrop = (e: DragEvent, row: TreeRow) => {
    e.preventDefault();
    const dragged = drag();
    const o = over();
    setDrag(undefined);
    setOver(undefined);
    if (!dragged || !o || o.id !== row.node.id) return;
    const target = dropTarget(props.tree, dragged, row.node.id, o.position);
    if (target) void ws().move(dragged, target.parent, target.index);
  };

  return (
    <>
      <div
        ref={scroller}
        class={styles.scroller}
        role="tree"
        aria-label="Workspace"
        tabIndex={0}
        aria-activedescendant={ws().selected() ? `tree-${ws().selected()}` : undefined}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onKeyDown={onKeyDown}
        onContextMenu={(e) => {
          // Empty space below the rows: actions for the workspace root.
          if ((e.target as Element).closest('[role="treeitem"]')) return;
          e.preventDefault();
          openMenu(props.tree, e.clientX, e.clientY);
        }}
        onFocus={() => {
          if (!ws().selected()) ws().setSelected(props.tree.id);
        }}
      >
        <div style={{ height: `${rows().length * ROW_HEIGHT}px`, position: "relative" }}>
          <div style={{ transform: `translateY(${range().start * ROW_HEIGHT}px)` }}>
            <For each={visible()}>
              {(row) => (
                // biome-ignore lint/a11y/useFocusableInteractive: focus stays on the tree (aria-activedescendant).
                <div
                  id={`tree-${row.node.id}`}
                  data-id={row.node.id}
                  data-kind={row.node.kind}
                  role="treeitem"
                  aria-level={row.depth + 1}
                  aria-expanded={row.node.kind === "folder" ? row.expanded : undefined}
                  aria-selected={ws().selected() === row.node.id}
                  aria-label={
                    row.node.kind === "error" ? `${row.node.name} (error)` : row.node.name
                  }
                  class={styles.row}
                  classList={{
                    [styles.selected as string]: ws().selected() === row.node.id,
                    [styles.error as string]: row.node.kind === "error",
                    [styles.collection as string]: row.node.kind === "collection",
                    [styles.open as string]: props.app.tabs.active()?.key === row.node.id,
                    [styles.dropBefore as string]:
                      over()?.id === row.node.id && over()?.position === "before",
                    [styles.dropAfter as string]:
                      over()?.id === row.node.id && over()?.position === "after",
                    [styles.dropInside as string]:
                      over()?.id === row.node.id && over()?.position === "inside",
                  }}
                  style={{ "padding-left": `${8 + row.depth * 14}px`, height: `${ROW_HEIGHT}px` }}
                  title={row.node.kind === "error" ? (row.node.error ?? "") : row.node.path}
                  draggable={
                    row.node.kind !== "collection" && row.node.kind !== "error" && !ws().renaming()
                  }
                  onDragStart={(e) => {
                    setDrag(row.node.id);
                    e.dataTransfer?.setData("text/plain", row.node.id);
                    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                  }}
                  onDragEnd={() => {
                    setDrag(undefined);
                    setOver(undefined);
                  }}
                  onDragOver={(e) => onDragOver(e, row)}
                  onDragLeave={() => over()?.id === row.node.id && setOver(undefined)}
                  onDrop={(e) => onDrop(e, row)}
                  onClick={() => activate(row.node, false)}
                  onDblClick={() => row.node.kind === "request" && activate(row.node, true)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    openMenu(row.node, e.clientX, e.clientY);
                  }}
                  onKeyDown={() => {}}
                >
                  <span class={styles.twisty} aria-hidden="true">
                    {row.node.kind === "folder" ? (row.expanded ? "▾" : "▸") : ""}
                  </span>
                  <Show when={row.node.kind === "request"}>
                    <MethodBadge method={row.node.method} />
                  </Show>
                  <Show when={row.node.kind === "error"}>
                    <span class={styles.warn} aria-hidden="true">
                      ⚠
                    </span>
                  </Show>
                  <Show
                    when={ws().renaming() === row.node.id}
                    fallback={
                      <span class={styles.name}>
                        <Highlighted text={row.node.name} indices={row.highlight} />
                      </span>
                    }
                  >
                    <RenameInput
                      initial={row.node.name}
                      onDone={(name) =>
                        name === null
                          ? ws().setRenaming(undefined)
                          : void ws().rename(row.node.id, name)
                      }
                    />
                  </Show>
                  <Show when={row.node.readOnly}>
                    <span class={styles.tag} title="Newer file format: read-only">
                      read-only
                    </span>
                  </Show>
                </div>
              )}
            </For>
          </div>
        </div>
      </div>
      <Show when={selectedError(props.tree, ws().selected())}>
        {(node) => (
          <section class={styles.inspector} aria-label="File error">
            <strong>{node().name} can't be read</strong>
            <p>{node().error}</p>
            <p class={styles.muted}>
              IronCall won't change this file. Fix it, then it reloads automatically.
            </p>
            <div class={styles.inspectorActions}>
              <button type="button" onClick={() => void ws().openExternal(node().id)}>
                Open in external editor
              </button>
              <button type="button" onClick={() => void ws().reveal(node().id)}>
                Reveal
              </button>
            </div>
          </section>
        )}
      </Show>
      <Show when={menu()}>
        {(m) => (
          <ContextMenu
            x={m().x}
            y={m().y}
            label={`${m().node.name} actions`}
            items={menuItems(m().node)}
            onClose={() => {
              setMenu(undefined);
              // Rename puts focus in its input; taking it back would blur (and end) it.
              if (!ws().renaming()) scroller.focus();
            }}
          />
        )}
      </Show>
    </>
  );
}

function selectedError(tree: TreeNode, id: ItemId | undefined) {
  const node = id ? findNode(tree, id) : undefined;
  return node?.kind === "error" ? node : undefined;
}

/** Inline name editor: Enter or blur commits, Esc cancels (`null`). */
export function RenameInput(props: { initial: string; onDone: (name: string | null) => void }) {
  let input!: HTMLInputElement;
  let done = false;
  const finish = (value: string | null) => {
    if (done) return;
    done = true;
    props.onDone(value);
  };
  onMount(() => {
    input.focus();
    input.select();
  });
  return (
    <input
      ref={input}
      class={styles.rename}
      aria-label="Name"
      value={props.initial}
      data-native-undo
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(e.currentTarget.value);
        else if (e.key === "Escape") finish(null);
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
  );
}
