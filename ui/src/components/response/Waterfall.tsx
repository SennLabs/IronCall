import { For, Show } from "solid-js";
import type { ResponseSummary } from "../../ipc/bindings";
import { buildWaterfall, PHASE_LABEL, type Segment } from "../../model/waterfall";
import { formatMs, statusClass } from "../common/format";
import styles from "./waterfall.module.css";

function segmentTitle(s: Segment): string {
  const label = PHASE_LABEL[s.phase];
  if (s.state === "reused") return `${label}: reused connection`;
  if (s.state === "notApplicable") return `${label}: not applicable`;
  return `${label}: ${formatMs(s.ms)}`;
}

/**
 * Timing waterfall (PLAN.md Phase 5): DNS / connect / TLS / TTFB / download bars for
 * each redirect hop and the final response, on one shared time scale.
 */
export function Waterfall(props: { response: ResponseSummary }) {
  const w = () =>
    buildWaterfall(
      props.response.redirects,
      {
        status: props.response.status,
        url: props.response.request.url,
        timings: props.response.timings,
      },
      props.response.timings.totalMs,
      props.response.informational,
    );
  return (
    <div class={styles.waterfall}>
      <div class={styles.legend} aria-hidden="true">
        <For each={["dns", "connect", "tls", "ttfb", "download"] as const}>
          {(p) => (
            <span>
              <i class={styles.swatch} data-phase={p} /> {PHASE_LABEL[p]}
            </span>
          )}
        </For>
      </div>
      <table class={styles.table}>
        <caption class={styles.caption}>
          Total {formatMs(props.response.timings.totalMs)}
          <Show when={props.response.connectionReused}> · reused connection</Show>
        </caption>
        <tbody>
          <For each={w().rows}>
            {(row) => (
              <tr>
                <th scope="row" class={styles.label}>
                  <span class={styles.status} data-class={statusClass(row.status)}>
                    {row.status}
                  </span>{" "}
                  <span title={row.url}>{row.label}</span>
                </th>
                <td class={styles.track}>
                  <div class={styles.lane}>
                    <For each={w().ticks}>
                      {(t) => <span class={styles.tick} style={{ left: `${t.at * 100}%` }} />}
                    </For>
                    <For each={row.segments.filter((s) => s.state === "measured" && s.width > 0)}>
                      {(s) => (
                        <span
                          class={styles.bar}
                          data-phase={s.phase}
                          title={segmentTitle(s)}
                          style={{
                            left: `${s.start * 100}%`,
                            width: `max(${s.width * 100}%, 2px)`,
                          }}
                        />
                      )}
                    </For>
                    <Show when={row.label === "Response"}>
                      <For each={w().markers}>
                        {(m) => (
                          <span
                            class={styles.marker}
                            style={{ left: `${m.at * 100}%` }}
                            title={`${m.label} informational response`}
                          >
                            {m.label}
                          </span>
                        )}
                      </For>
                    </Show>
                  </div>
                  <div class={styles.phases}>
                    <For each={row.segments}>
                      {(s) => (
                        <span data-state={s.state}>
                          {PHASE_LABEL[s.phase]}{" "}
                          {s.state === "measured"
                            ? formatMs(s.ms)
                            : s.state === "reused"
                              ? "reused"
                              : "n/a"}
                        </span>
                      )}
                    </For>
                  </div>
                </td>
                <td class={styles.total}>{formatMs(row.totalMs)}</td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
      <div class={styles.axis} aria-hidden="true">
        <For each={w().ticks}>
          {(t) => (
            <span class={styles.axisLabel} style={{ left: `${t.at * 100}%` }}>
              {t.label}
            </span>
          )}
        </For>
      </div>
      <Show when={props.response.informational.length > 0}>
        <ul class={styles.informational}>
          <For each={props.response.informational}>
            {(r) => (
              <li>
                <span class={styles.status} data-class="info">
                  {r.status}
                </span>{" "}
                {r.status === 103 ? "Early Hints" : "Informational"} at {formatMs(r.atMs)} after
                sending · {r.headers.length} header{r.headers.length === 1 ? "" : "s"}
              </li>
            )}
          </For>
        </ul>
        <p class={styles.note}>TTFB is measured to the final response's headers.</p>
      </Show>
    </div>
  );
}
