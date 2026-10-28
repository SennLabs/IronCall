import { createSignal, For, type JSX, Show } from "solid-js";
import styles from "./Tabs.module.css";

export interface TabDef<K extends string> {
  key: K;
  label: JSX.Element;
}

export interface TabsProps<K extends string> {
  tabs: readonly TabDef<K>[];
  initial?: K;
  /** Extra controls shown at the right end of the tab strip. */
  actions?: JSX.Element;
  children: (active: K) => JSX.Element;
  label: string;
}

/** Accessible tab strip (arrow keys move between tabs). Panels render lazily. */
export function Tabs<K extends string>(props: TabsProps<K>) {
  const [active, setActive] = createSignal<K>(props.initial ?? (props.tabs[0]?.key as K));
  const buttons: HTMLButtonElement[] = [];

  const onKeyDown = (e: KeyboardEvent, index: number) => {
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = (index + step + props.tabs.length) % props.tabs.length;
    const tab = props.tabs[next];
    if (tab) setActive(() => tab.key);
    buttons[next]?.focus();
  };

  return (
    <div class={styles.tabs}>
      <div class={styles.strip}>
        <div role="tablist" aria-label={props.label} class={styles.list}>
          <For each={props.tabs}>
            {(tab, i) => (
              <button
                ref={(el) => {
                  buttons[i()] = el;
                }}
                type="button"
                role="tab"
                aria-selected={active() === tab.key}
                tabIndex={active() === tab.key ? 0 : -1}
                class={styles.tab}
                onClick={() => setActive(() => tab.key)}
                onKeyDown={(e) => onKeyDown(e, i())}
              >
                {tab.label}
              </button>
            )}
          </For>
        </div>
        <Show when={props.actions}>
          <div class={styles.actions}>{props.actions}</div>
        </Show>
      </div>
      <div role="tabpanel" class={styles.panel}>
        {props.children(active())}
      </div>
    </div>
  );
}
