import { createMemo, createSignal, Index, onCleanup, onMount } from "solid-js";
import type { BodyRow } from "../../ipc/bindings";
import { columnWindow, contentWidth } from "../../model/columns";
import styles from "./LargeBodyViewer.module.css";
import { PageCache } from "./pageCache";
import {
  clampRealTop,
  type Geometry,
  realTopForRow,
  scrollHeight,
  toRealTop,
  toScrollTop,
  windowAt,
} from "./viewport";

export const ROW_HEIGHT = 18;
/** Gutter width in characters (8ch number + 1ch padding; see the CSS). */
const GUTTER_CHARS = 9;
/** Used until the real character width is measured (and in jsdom, which has no layout). */
const FALLBACK_CHAR_WIDTH = 7.2;
const OVERSCAN_ROWS = 100;
const PAGE_TARGET_BYTES = 64 * 1024;

/** Rows per page so a page is about PAGE_TARGET_BYTES. */
export function pageSizeFor(rowCount: number, byteLen: number): number {
  const avgRowBytes = rowCount > 0 ? byteLen / rowCount : 1;
  return Math.min(200, Math.max(20, Math.floor(PAGE_TARGET_BYTES / Math.max(avgRowBytes, 1))));
}

export interface LargeBodyViewerProps {
  rowCount: number;
  /** Body size, used to size pages at ~64KB so wide rows don't make huge IPC payloads. */
  byteLen: number;
  fetchRows: (from: number, count: number) => Promise<BodyRow[]>;
  /** Row to highlight, e.g. a search hit. */
  highlightRow?: number;
  /** Exposes imperative controls (scroll element, jump to row) to the parent. */
  ref?: (api: LargeBodyViewerApi) => void;
}

export interface LargeBodyViewerApi {
  scroller: HTMLElement;
  scrollToRow: (row: number) => void;
  /** Scrolls by true (unscaled) pixels, as a wheel would. */
  scrollByPixels: (dy: number) => void;
  /** Rows currently on screen whose page hasn't loaded yet. */
  blankRows: () => number;
}

/**
 * Virtualised, paged text viewer for bodies of any size (PLAN.md Phase 1/5).
 * Rows are fetched from Rust in pages; only the visible rows exist in the DOM.
 */
export function LargeBodyViewer(props: LargeBodyViewerProps) {
  let scroller!: HTMLElement;
  let windowEl!: HTMLDivElement;
  let probe!: HTMLSpanElement;
  // Horizontal virtualisation (ADR 0003): only the characters in view are rendered.
  const [scrollLeft, setScrollLeft] = createSignal(0);
  const [viewportWidth, setViewportWidth] = createSignal(800);
  const [charWidth, setCharWidth] = createSignal(FALLBACK_CHAR_WIDTH);
  // True scroll position in unscaled pixels; the scrollbar may be scaled (see viewport.ts).
  const [realTop, setRealTop] = createSignal(0);
  const [viewportHeight, setViewportHeight] = createSignal(600);
  const [version, setVersion] = createSignal(0);
  // Last scrollTop we set ourselves, so the resulting scroll event isn't re-interpreted.
  let ownScrollTop = -1;

  const geometry = (): Geometry => ({
    rowCount: props.rowCount,
    rowHeight: ROW_HEIGHT,
    viewportHeight: viewportHeight(),
  });

  const cache = new PageCache<BodyRow>(
    (from, count) => props.fetchRows(from, count),
    pageSizeFor(props.rowCount, props.byteLen),
    128,
    () => setVersion((v) => v + 1),
  );

  const win = createMemo(() => windowAt(geometry(), realTop()));

  const visible = createMemo(() => {
    version();
    const { first, count } = win();
    void cache.ensure(first - OVERSCAN_ROWS, first + count + OVERSCAN_ROWS);
    return Array.from({ length: count }, (_, i) => ({
      index: first + i,
      row: cache.row(first + i),
    }));
  });

  const rowsWidth = createMemo(() =>
    contentWidth(
      visible().map((v) => v.row?.text.length ?? 0),
      charWidth(),
      GUTTER_CHARS * charWidth(),
    ),
  );

  /** The slice of a row's text to render, plus the padding for skipped characters. */
  const slice = (text: string) => {
    const cw = charWidth();
    const w = columnWindow(text.length, cw, scrollLeft() - GUTTER_CHARS * cw, viewportWidth());
    return { text: text.slice(w.start, w.end), padLeft: w.padLeft, width: text.length * cw };
  };

  const moveTo = (top: number) => {
    const real = clampRealTop(geometry(), top);
    setRealTop(real);
    scroller.scrollTop = toScrollTop(geometry(), real);
    ownScrollTop = scroller.scrollTop;
  };
  const scrollByPixels = (dy: number) => moveTo(realTop() + dy);
  const scrollToRow = (row: number) => moveTo(realTopForRow(geometry(), row));

  const onScroll = () => {
    // Scrollbar drags land here; our own moves are already applied.
    if (Math.abs(scroller.scrollTop - ownScrollTop) < 1) return;
    ownScrollTop = -1;
    setRealTop(toRealTop(geometry(), scroller.scrollTop));
  };

  const onWheel = (e: WheelEvent) => {
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return; // horizontal: let the row window scroll
    e.preventDefault();
    const unit = e.deltaMode === 1 ? ROW_HEIGHT : e.deltaMode === 2 ? viewportHeight() : 1;
    scrollByPixels(e.deltaY * unit);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const page = viewportHeight() - ROW_HEIGHT;
    const moves: Record<string, () => void> = {
      ArrowDown: () => scrollByPixels(ROW_HEIGHT),
      ArrowUp: () => scrollByPixels(-ROW_HEIGHT),
      PageDown: () => scrollByPixels(page),
      PageUp: () => scrollByPixels(-page),
      Home: () => moveTo(0),
      End: () => moveTo(Number.MAX_SAFE_INTEGER),
    };
    const move = moves[e.key];
    if (move) {
      e.preventDefault();
      move();
    }
  };

  onMount(() => {
    const observer = new ResizeObserver(() => {
      setViewportHeight(scroller.clientHeight);
      setViewportWidth(windowEl.clientWidth);
    });
    observer.observe(scroller);
    setViewportHeight(scroller.clientHeight);
    setViewportWidth(windowEl.clientWidth || 800);
    const measured = probe.getBoundingClientRect().width / 100;
    if (measured > 0) setCharWidth(measured);
    // Scrollable regions must be keyboard-focusable (WCAG 2.1.1). Set here because the
    // a11y lint rejects tabIndex on a non-interactive element in JSX.
    scroller.tabIndex = 0;
    scroller.addEventListener("wheel", onWheel, { passive: false });
    onCleanup(() => {
      observer.disconnect();
      scroller.removeEventListener("wheel", onWheel);
    });
    props.ref?.({
      scroller,
      scrollToRow,
      scrollByPixels,
      blankRows: () => visible().filter((v) => !v.row).length,
    });
  });

  return (
    <section
      class={styles.scroller}
      ref={scroller}
      aria-label="Response body"
      onScroll={onScroll}
      onKeyDown={onKeyDown}
    >
      {/* Sticky window stays pinned by the compositor; the spacer provides scroll height. */}
      <div
        class={styles.window}
        ref={windowEl}
        style={{ height: `${viewportHeight()}px` }}
        onScroll={() => setScrollLeft(windowEl.scrollLeft)}
      >
        <span ref={probe} class={styles.probe} aria-hidden="true">
          {"0".repeat(100)}
        </span>
        <div
          class={styles.rows}
          style={{
            transform: `translateY(${win().offset}px)`,
            "min-width": `${rowsWidth()}px`,
          }}
        >
          {/* Index keeps one DOM node per screen slot and only swaps its text. */}
          <Index each={visible()}>
            {(v) => (
              <div
                class={styles.row}
                classList={{ [styles.highlight as string]: v().index === props.highlightRow }}
              >
                <span class={styles.gutter}>
                  {(() => {
                    const row = v().row;
                    return row ? (row.continuation ? "↪" : row.line + 1) : "";
                  })()}
                </span>
                {(() => {
                  const row = v().row;
                  // Placeholder bar while the page loads (e.g. during scrollbar drags),
                  // so the view never shows blank gaps.
                  if (!row) return <span class={styles.placeholder} data-placeholder="" />;
                  const s = slice(row.text);
                  return (
                    <span
                      class={styles.text}
                      style={
                        s.padLeft > 0 || s.text.length < row.text.length
                          ? { "padding-left": `${s.padLeft}px`, width: `${s.width}px` }
                          : undefined
                      }
                    >
                      {s.text}
                    </span>
                  );
                })()}
              </div>
            )}
          </Index>
        </div>
      </div>
      <div
        style={{
          height: `${Math.max(scrollHeight(props.rowCount, ROW_HEIGHT) - viewportHeight(), 0)}px`,
        }}
      />
    </section>
  );
}
