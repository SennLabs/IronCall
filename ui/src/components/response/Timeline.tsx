import { createSignal, For, Show } from "solid-js";
import type { TimelineEntry } from "../../ipc/bindings";
import { firstLine, formatAt, isBlock, KIND_PREFIX, timelineText } from "../../model/timeline";
import { CopyButton } from "../common/CopyButton";
import styles from "./timeline.module.css";

/** Per-send console, like `curl -v` (PLAN.md Phase 5). Header lists collapse. */
export function Timeline(props: { entries: TimelineEntry[] }) {
  return (
    <div class={styles.timeline}>
      <div class={styles.toolbar}>
        <span class={styles.muted}>
          {props.entries.length} event{props.entries.length === 1 ? "" : "s"} · sensitive values
          masked
        </span>
        <span class={styles.spacer} />
        <CopyButton label="Copy timeline" text={() => timelineText(props.entries)} />
      </div>
      <Show
        when={props.entries.length > 0}
        fallback={<p class={styles.muted}>Nothing was sent.</p>}
      >
        <ol class={styles.log} aria-label="Timeline">
          <For each={props.entries}>{(e) => <Entry entry={e} />}</For>
        </ol>
      </Show>
    </div>
  );
}

function Entry(props: { entry: TimelineEntry }) {
  const [open, setOpen] = createSignal(true);
  const prefix = () => KIND_PREFIX[props.entry.kind];
  return (
    <li class={styles.entry} data-kind={props.entry.kind}>
      <span class={styles.at}>{formatAt(props.entry.atMs)}</span>
      <span class={styles.prefix} aria-hidden="true">
        {prefix()}
      </span>
      <Show
        when={isBlock(props.entry)}
        fallback={<pre class={styles.text}>{props.entry.text}</pre>}
      >
        <div class={styles.block}>
          <button
            type="button"
            class={styles.toggle}
            aria-expanded={open()}
            onClick={() => setOpen(!open())}
          >
            <span aria-hidden="true">{open() ? "▾" : "▸"}</span> {firstLine(props.entry)}
          </button>
          <Show when={open()}>
            <pre class={styles.text}>{props.entry.text.split("\n").slice(1).join("\n")}</pre>
          </Show>
        </div>
      </Show>
    </li>
  );
}
