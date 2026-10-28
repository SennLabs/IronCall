import { onMount, Show } from "solid-js";
import type { VarsEditor } from "../../stores/varsEditor";
import { VarsTable } from "./VarsTable";
import styles from "./vars.module.css";

/** A scope's variable table with Save/Revert and the conflict bar. */
export function VarsEditorView(props: {
  editor: VarsEditor;
  label: string;
  allowSecrets?: boolean;
  readOnly?: boolean;
}) {
  const e = () => props.editor;
  onMount(() => {
    if (!e().loaded()) void e().reload();
  });
  return (
    <div class={styles.editor}>
      <Show when={e().conflict()}>
        <div class={styles.conflict} role="alert" aria-label="Variables conflict">
          <span>
            <strong>This file changed on disk</strong> since you opened it. Reload to see the latest
            version (your unsaved edits here are lost).
          </span>
          <button type="button" onClick={() => void e().reload()}>
            Reload
          </button>
        </div>
      </Show>
      <Show when={e().loaded()} fallback={<p class={styles.muted}>Loading…</p>}>
        <VarsTable
          label={props.label}
          rows={e().rows()}
          onChange={(rows) => e().setRows(rows)}
          allowSecrets={props.allowSecrets}
          readOnly={props.readOnly}
          secretState={(r) => e().secretState(r)}
          onToggleSecret={(i, on) => void e().toggleSecret(i, on)}
          onSetSecret={(i) => void e().setSecret(i)}
          onClearSecret={(i) => void e().clearSecret(i)}
          onReveal={(i) => void e().reveal(i)}
        />
      </Show>
      <div class={styles.actions}>
        <span class={styles.muted}>
          <Show when={props.allowSecrets}>
            Secret values are kept in the secret store on this machine, never in the file.
          </Show>
        </span>
        <span class={styles.spacer} />
        <button
          type="button"
          disabled={!e().dirty() || e().busy()}
          onClick={() => void e().reload()}
        >
          Revert
        </button>
        <button
          type="button"
          class="primary"
          disabled={!e().dirty() || e().busy() || props.readOnly}
          onClick={() => void e().save()}
        >
          Save
        </button>
      </div>
    </div>
  );
}
