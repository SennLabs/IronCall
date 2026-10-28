import { For } from "solid-js";
import { dismiss, notices } from "../../stores/notify";
import styles from "./Notices.module.css";

/** Transient messages, bottom right. */
export function Notices() {
  return (
    <div class={styles.stack} aria-live="polite">
      <For each={notices()}>
        {(n) => (
          <div
            class={styles.notice}
            classList={{ [styles.error as string]: n.kind === "error" }}
            role={n.kind === "error" ? "alert" : "status"}
          >
            <span>{n.text}</span>
            <button type="button" class="ghost" aria-label="Dismiss" onClick={() => dismiss(n.id)}>
              ✕
            </button>
          </div>
        )}
      </For>
    </div>
  );
}
