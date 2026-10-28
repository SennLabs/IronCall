import {
  createMemo,
  createSignal,
  For,
  type JSX,
  onCleanup,
  onMount,
  Show,
  splitProps,
} from "solid-js";
import {
  applyCompletion,
  type Completion,
  completionQuery,
  completions,
  hoverInfo,
  refAt,
  segments,
} from "../../model/variables";
import styles from "./VarInput.module.css";
import { useVars } from "./VarsContext";

export type VarInputProps = Omit<JSX.InputHTMLAttributes<HTMLInputElement>, "onInput" | "value"> & {
  value: string;
  onInput: (value: string) => void;
  /** Class for the wrapper (layout); `class` styles the input itself. */
  wrapClass?: string;
};

const MAX_SUGGESTIONS = 12;

/**
 * A single-line input with `{{variable}}` support (PLAN.md §5):
 * - references are highlighted by an overlay behind transparent text (defined,
 *   helper, unresolved, missing secret), so typing stays a plain `<input>`;
 * - typing `{{` opens an autocomplete list (↑/↓, Enter or Tab to insert, Esc to close);
 * - hovering a reference shows its value and source.
 * Outside a `VarsProvider` it renders a plain input.
 */
export function VarInput(props: VarInputProps) {
  const [local, rest] = splitProps(props, ["value", "onInput", "class", "wrapClass"]);
  const vars = useVars();
  const ctx = () => vars?.();
  let input!: HTMLInputElement;
  let mirror!: HTMLDivElement;

  const [caret, setCaret] = createSignal<number | null>(null);
  const [selected, setSelected] = createSignal(0);
  const [dismissed, setDismissed] = createSignal(false);
  const [hover, setHover] = createSignal<{ name: string; x: number } | undefined>();

  const query = createMemo(() => {
    const c = caret();
    return c === null || dismissed() ? undefined : completionQuery(local.value, c);
  });
  const items = createMemo<Completion[]>(() => {
    const q = query();
    return q ? completions(ctx(), q.query).slice(0, MAX_SUGGESTIONS) : [];
  });

  const syncScroll = () => {
    if (mirror) mirror.scrollLeft = input.scrollLeft;
  };

  const accept = (c: Completion) => {
    const q = query();
    const at = caret();
    if (!q || at === null) return;
    const next = applyCompletion(local.value, q.start, at, c.name);
    local.onInput(next.text);
    queueMicrotask(() => {
      input.value = next.text;
      input.setSelectionRange(next.caret, next.caret);
      setCaret(next.caret);
      setDismissed(true);
    });
  };

  const onKeyDown: JSX.EventHandler<HTMLInputElement, KeyboardEvent> = (e) => {
    const list = items();
    if (list.length === 0) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setSelected((i) => (i + step + list.length) % list.length);
    } else if (e.key === "Enter" || e.key === "Tab") {
      const c = list[selected()];
      if (!c) return;
      e.preventDefault();
      e.stopPropagation();
      accept(c);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setDismissed(true);
    }
  };

  /** Character offset under the mouse, measured with the input's own font. */
  const offsetAt = (clientX: number): number | undefined => {
    const rect = input.getBoundingClientRect();
    const style = getComputedStyle(input);
    const x = clientX - rect.left - Number.parseFloat(style.paddingLeft || "0") + input.scrollLeft;
    const canvas = offsetAt.canvas ?? document.createElement("canvas");
    offsetAt.canvas = canvas;
    const g = canvas.getContext?.("2d");
    if (!g) return undefined;
    g.font = style.font || `${style.fontSize} ${style.fontFamily}`;
    const text = local.value;
    for (let i = 0; i < text.length; i++) {
      if (g.measureText(text.slice(0, i + 1)).width > x) return i;
    }
    return undefined;
  };
  offsetAt.canvas = undefined as HTMLCanvasElement | undefined;

  const onMouseMove: JSX.EventHandler<HTMLInputElement, MouseEvent> = (e) => {
    if (!ctx()) return;
    const offset = offsetAt(e.clientX);
    const ref = offset === undefined ? undefined : refAt(local.value, offset);
    const left = e.clientX - input.getBoundingClientRect().left;
    setHover(ref ? { name: ref.name, x: left } : undefined);
  };

  const info = () => {
    const h = hover();
    return h ? hoverInfo(h.name, ctx()) : undefined;
  };

  // The mirror must lay text out exactly like the input.
  onMount(() => {
    if (!mirror) return;
    const s = getComputedStyle(input);
    for (const prop of [
      "padding-left",
      "padding-right",
      "font-family",
      "font-size",
      "font-weight",
      "letter-spacing",
      "border-left-width",
      "border-right-width",
    ]) {
      mirror.style.setProperty(prop, s.getPropertyValue(prop));
    }
  });

  const blurTimer: { id?: ReturnType<typeof setTimeout> } = {};
  onCleanup(() => clearTimeout(blurTimer.id));

  return (
    <div class={`${styles.wrap} ${local.wrapClass ?? ""}`}>
      <Show when={vars}>
        <div ref={mirror} class={styles.mirror} aria-hidden="true">
          <For each={segments(local.value, ctx())}>
            {(s) => (
              <span
                class={s.state === "plain" ? undefined : styles.ref}
                data-state={s.state === "plain" ? undefined : s.state}
              >
                {s.text}
              </span>
            )}
          </For>
        </div>
      </Show>
      <input
        {...rest}
        ref={input}
        class={`${styles.input} ${local.class ?? ""}`}
        classList={{ [styles.overlay as string]: !!vars }}
        value={local.value}
        spellcheck={false}
        autocomplete="off"
        role="combobox"
        aria-autocomplete={vars ? "list" : undefined}
        aria-expanded={items().length > 0}
        onInput={(e) => {
          setDismissed(false);
          setSelected(0);
          setCaret(e.currentTarget.selectionStart);
          local.onInput(e.currentTarget.value);
          syncScroll();
        }}
        onKeyDown={onKeyDown}
        onKeyUp={(e) => {
          if (e.key.startsWith("Arrow") && items().length > 0) return;
          setCaret(e.currentTarget.selectionStart);
          syncScroll();
        }}
        onClick={(e) => setCaret(e.currentTarget.selectionStart)}
        onScroll={syncScroll}
        onMouseMove={onMouseMove}
        onMouseLeave={() => setHover(undefined)}
        onBlur={(e) => {
          // Let a click on a suggestion land first.
          blurTimer.id = setTimeout(() => setCaret(null), 150);
          if (typeof rest.onBlur === "function") rest.onBlur(e);
        }}
        onFocus={() => clearTimeout(blurTimer.id)}
      />
      <Show when={items().length > 0}>
        <div class={styles.menu} role="listbox" aria-label="Variables">
          <For each={items()}>
            {(c, i) => (
              <div
                role="option"
                tabIndex={-1}
                aria-selected={i() === selected()}
                class={styles.option}
                classList={{ [styles.active as string]: i() === selected() }}
                onMouseDown={(e) => {
                  e.preventDefault();
                  accept(c);
                }}
              >
                <span class={styles.optName} data-helper={c.helper || undefined}>
                  {c.name}
                </span>
                <span class={styles.optDetail}>{c.detail}</span>
                <span class={styles.optSource}>{c.source}</span>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={info()}>
        {(h) => (
          <div class={styles.tooltip} role="tooltip" style={{ left: `${hover()?.x ?? 0}px` }}>
            <VarTooltip info={h()} />
          </div>
        )}
      </Show>
    </div>
  );
}

/** Content of the hover tooltip; also used by the body editor. */
export function VarTooltip(props: { info: ReturnType<typeof hoverInfo> }) {
  return (
    <div class={styles.tip}>
      <div class={styles.tipTitle}>{props.info.title}</div>
      <Show when={props.info.value}>
        <div class={styles.tipValue}>{props.info.value}</div>
      </Show>
      <Show when={props.info.source}>
        <div class={styles.tipSource}>from {props.info.source}</div>
      </Show>
      <Show when={props.info.overrides.length > 0}>
        <div class={styles.tipSource}>overrides {props.info.overrides.join(", ")}</div>
      </Show>
      <Show when={props.info.problem}>
        <div class={styles.tipProblem}>{props.info.problem}</div>
      </Show>
    </div>
  );
}
