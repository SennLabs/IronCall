import { For } from "solid-js";
import type { TimingPhase, TimingsInfo } from "../../ipc/bindings";
import { formatMs } from "../common/format";
import styles from "./response.module.css";

function phaseText(phase: TimingPhase): string {
  if (phase.kind === "measured") return formatMs(phase.ms);
  return phase.kind === "reused" ? "reused" : "n/a";
}

export function timingRows(t: TimingsInfo): { label: string; value: string }[] {
  return [
    { label: "DNS lookup", value: phaseText(t.dns) },
    { label: "TCP connect", value: phaseText(t.connect) },
    { label: "TLS handshake", value: phaseText(t.tls) },
    { label: "Waiting (TTFB)", value: formatMs(t.ttfbMs) },
    { label: "Download", value: formatMs(t.downloadMs) },
    { label: "Total", value: formatMs(t.totalMs) },
  ];
}

/** Phase table shown in the timing popover. The full waterfall comes in Phase 5. */
export function TimingBreakdown(props: { timings: TimingsInfo }) {
  return (
    <table class={styles.timings}>
      <tbody>
        <For each={timingRows(props.timings)}>
          {(r) => (
            <tr>
              <th scope="row">{r.label}</th>
              <td>{r.value}</td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}
