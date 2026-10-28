import { createSignal, For, Show } from "solid-js";
import type { ResponseSummary } from "../../ipc/bindings";
import { formatBytes, formatMs, statusClass } from "../common/format";
import styles from "./response.module.css";
import { TimingBreakdown } from "./TimingBreakdown";

/** Status, time (with phase popover), size, connection reuse and redirects. */
export function ResponseMeta(props: { response: ResponseSummary }) {
  const [showTimings, setShowTimings] = createSignal(false);
  const r = () => props.response;
  return (
    <div class={styles.meta}>
      <span class={styles.status} data-class={statusClass(r().status)} data-testid="status">
        {r().status} {r().statusText}
      </span>
      <span class={styles.popoverHost}>
        <button
          type="button"
          class="ghost"
          aria-expanded={showTimings()}
          onClick={() => setShowTimings((v) => !v)}
          onBlur={() => setShowTimings(false)}
          title="Timing breakdown"
        >
          {formatMs(r().timings.totalMs)}
        </button>
        <Show when={showTimings()}>
          <div class={styles.popover} role="dialog" aria-label="Timing breakdown">
            <TimingBreakdown timings={r().timings} />
          </div>
        </Show>
      </span>
      <span title={`${r().body.size.toLocaleString()} bytes`}>
        {formatBytes(r().body.size)}
        <Show when={r().body.wireSize !== r().body.size}>
          <span class={styles.muted}> ({formatBytes(r().body.wireSize)} on the wire)</span>
        </Show>
      </span>
      <span class={styles.muted}>{r().httpVersion}</span>
      <Show when={r().connectionReused}>
        <span
          class={styles.muted}
          title="Sent on a pooled connection, so DNS, connect and TLS were skipped"
        >
          reused connection
        </span>
      </Show>
      <Show when={r().redirects.length > 0}>
        <details class={styles.redirects}>
          <summary>
            {r().redirects.length} redirect{r().redirects.length > 1 ? "s" : ""}
          </summary>
          <ol>
            <For each={r().redirects}>
              {(hop) => (
                <li>
                  <span class={styles.status} data-class={statusClass(hop.status)}>
                    {hop.status}
                  </span>{" "}
                  <code>{hop.url}</code>{" "}
                  <span class={styles.muted}>{formatMs(hop.timings.totalMs)}</span>
                </li>
              )}
            </For>
          </ol>
        </details>
      </Show>
    </div>
  );
}
