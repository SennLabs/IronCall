import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import type { HistorySummary, StatusClass } from "../../ipc/api";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { formatBytes, formatMs, statusClass } from "../common/format";
import { MethodBadge } from "../common/MethodBadge";
import { HistoryDetail } from "./HistoryDetail";
import styles from "./history.module.css";

export const HISTORY_ROW_HEIGHT = 44;
const OVERSCAN = 8;

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"] as const;
const STATUSES: { key: StatusClass; label: string }[] = [
  { key: "success", label: "2xx" },
  { key: "redirect", label: "3xx" },
  { key: "clientError", label: "4xx" },
  { key: "serverError", label: "5xx" },
  { key: "failed", label: "Failed" },
];

/** Rows to render for a fixed-height virtual list. */
export function listWindow(count: number, scrollTop: number, height: number, rowHeight: number) {
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN);
  const last = Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + OVERSCAN);
  return { first, last };
}

function timeOf(iso: string): string {
  const d = new Date(iso);
  const today = new Date().toDateString() === d.toDateString();
  return today
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : d.toLocaleString([], { dateStyle: "short", timeStyle: "short" });
}

/** History drawer (PLAN.md Phase 5): search, filters, list, and the selected entry. */
export function HistoryPanel(props: { app: AppStore }) {
  const h = () => props.app.history;
  let list!: HTMLDivElement;
  let search!: HTMLInputElement;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(600);

  onMount(() => {
    search.focus();
    const observer = new ResizeObserver(() => setHeight(list.clientHeight || 600));
    observer.observe(list);
  });

  const win = createMemo(() =>
    listWindow(h().entries().length, scrollTop(), height(), HISTORY_ROW_HEIGHT),
  );

  const onScroll = () => {
    setScrollTop(list.scrollTop);
    // Infinite scroll: fetch the next page near the bottom.
    if (list.scrollTop + list.clientHeight > list.scrollHeight - HISTORY_ROW_HEIGHT * 5) {
      void h().loadMore();
    }
  };

  const onListKey = (e: KeyboardEvent) => {
    const items = h().entries();
    const at = items.findIndex((x) => x.id === h().selected());
    const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = items[Math.min(items.length - 1, Math.max(0, at + step))];
    if (next) void h().select(next.id);
  };

  return (
    <aside class={styles.panel} aria-label="History">
      <div class={styles.header}>
        <strong>History</strong>
        <span class={styles.spacer} />
        <button
          type="button"
          class="ghost"
          onClick={() => openDialog({ kind: "history" })}
          title="History settings"
        >
          Settings…
        </button>
        <button
          type="button"
          class="ghost"
          aria-label="Close history"
          onClick={() => h().setOpen(false)}
        >
          ✕
        </button>
      </div>
      <div class={styles.filters}>
        <input
          ref={search}
          type="search"
          aria-label="Search history"
          placeholder="Search URL, method, headers…"
          value={h().text()}
          onInput={(e) => h().setText(e.currentTarget.value)}
        />
        <div class={styles.chips}>
          <select
            aria-label="History scope"
            value={h().scope()}
            onChange={(e) => h().setScope(e.currentTarget.value as "workspace" | "all")}
          >
            <option value="workspace" disabled={!props.app.workspace.info()}>
              This workspace
            </option>
            <option value="all">All</option>
          </select>
          <For each={METHODS}>
            {(m) => (
              <button
                type="button"
                class={styles.chip}
                aria-pressed={h().methods().includes(m)}
                onClick={() => h().toggleMethod(m)}
              >
                {m}
              </button>
            )}
          </For>
          <For each={STATUSES}>
            {(s) => (
              <button
                type="button"
                class={styles.chip}
                aria-pressed={h().statuses().includes(s.key)}
                onClick={() => h().toggleStatus(s.key)}
              >
                {s.label}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class={styles.body}>
        <div
          ref={list}
          class={styles.list}
          role="listbox"
          aria-label="History entries"
          tabIndex={0}
          onScroll={onScroll}
          onKeyDown={onListKey}
        >
          <Show
            when={h().entries().length > 0}
            fallback={
              <p class={styles.empty}>
                {h().loading() ? "Loading…" : "No requests in history match these filters."}
              </p>
            }
          >
            <div
              style={{
                height: `${h().entries().length * HISTORY_ROW_HEIGHT}px`,
                position: "relative",
              }}
            >
              <For each={h().entries().slice(win().first, win().last)}>
                {(entry, i) => (
                  <Row
                    entry={entry}
                    top={(win().first + i()) * HISTORY_ROW_HEIGHT}
                    selected={h().selected() === entry.id}
                    onSelect={() => void h().select(entry.id)}
                  />
                )}
              </For>
            </div>
            <Show when={h().next()}>
              <p class={styles.more}>{h().loading() ? "Loading more…" : ""}</p>
            </Show>
          </Show>
        </div>
        <div class={styles.footer}>
          <span class={styles.muted}>
            {h().entries().length}
            {h().next() ? "+" : ""} entries
          </span>
          <span class={styles.spacer} />
          <Show when={props.app.workspace.info()}>
            <button type="button" class="ghost" onClick={() => void h().clear("workspace")}>
              Clear workspace history
            </button>
          </Show>
          <button type="button" class="ghost" onClick={() => void h().clear("all")}>
            Clear all
          </button>
        </div>
      </div>
      <Show when={h().selected()}>
        <div class={styles.detailHost}>
          <Show when={h().detail()} keyed fallback={<p class={styles.empty}>Loading…</p>}>
            {(entry) => <HistoryDetail app={props.app} entry={entry} />}
          </Show>
        </div>
      </Show>
    </aside>
  );
}

function Row(props: {
  entry: HistorySummary;
  top: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const e = () => props.entry;
  return (
    <div
      role="option"
      tabIndex={-1}
      aria-selected={props.selected}
      class={styles.row}
      classList={{ [styles.selected as string]: props.selected }}
      style={{ top: `${props.top}px`, height: `${HISTORY_ROW_HEIGHT}px` }}
      onClick={props.onSelect}
      onKeyDown={(ev) => {
        if (ev.key === "Enter") props.onSelect();
      }}
      data-history-id={e().id}
    >
      <div class={styles.rowTop}>
        <MethodBadge method={e().method} />
        <span class={styles.url} title={e().url}>
          {e().url}
        </span>
      </div>
      <div class={styles.rowBottom}>
        <Show
          when={e().status !== null}
          fallback={
            <span class={styles.status} data-class="failed">
              {e().error ?? "failed"}
            </span>
          }
        >
          <span class={styles.status} data-class={statusClass(e().status as number)}>
            {e().status}
          </span>
        </Show>
        <span class={styles.muted}>{formatMs(e().totalMs)}</span>
        <Show when={e().size !== null}>
          <span class={styles.muted}>{formatBytes(e().size as number)}</span>
        </Show>
        <span class={styles.spacer} />
        <span class={styles.muted} title={e().at}>
          {timeOf(e().at)}
        </span>
      </div>
    </div>
  );
}
