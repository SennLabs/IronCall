import { createSignal, For, Show } from "solid-js";
import type { RecentSend } from "../../ipc/bindings";
import { formatMs, statusClass } from "../common/format";
import styles from "./response.module.css";

/** Toolbar popover with the last sends (redacted by the backend). */
export function RecentSends(props: { recent: readonly RecentSend[] }) {
  const [open, setOpen] = createSignal(false);
  return (
    <span class={styles.popoverHost}>
      <button type="button" class="ghost" aria-expanded={open()} onClick={() => setOpen((v) => !v)}>
        Recent ({props.recent.length})
      </button>
      <Show when={open()}>
        <div
          class={`${styles.popover} ${styles.popoverRight}`}
          role="dialog"
          aria-label="Recent sends"
        >
          <Show when={props.recent.length > 0} fallback={<p class={styles.muted}>No sends yet.</p>}>
            <ol class={styles.recent}>
              <For each={props.recent}>
                {(r) => (
                  <li>
                    <Show
                      when={r.status !== null}
                      fallback={
                        <span class={styles.status} data-class="server">
                          {r.error ?? "error"}
                        </span>
                      }
                    >
                      <span class={styles.status} data-class={statusClass(r.status ?? 0)}>
                        {r.status}
                      </span>
                    </Show>{" "}
                    <strong>{r.method}</strong> <code title={r.url}>{r.url}</code>{" "}
                    <span class={styles.muted}>{formatMs(r.totalMs)}</span>
                  </li>
                )}
              </For>
            </ol>
          </Show>
        </div>
      </Show>
    </span>
  );
}
