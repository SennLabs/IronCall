import { createResource, createSignal, For, Show } from "solid-js";
import { api, type HistorySettings } from "../../ipc/api";
import { closeDialog } from "../../stores/dialogs";
import { attempt, notify } from "../../stores/notify";
import { Modal } from "../common/Modal";
import styles from "./settings.module.css";

const FIELDS: { key: keyof HistorySettings; label: string; unit: string; min: number }[] = [
  { key: "maxEntries", label: "Keep at most", unit: "entries", min: 1 },
  { key: "maxAgeDays", label: "Delete entries older than", unit: "days", min: 1 },
  { key: "maxTotalMb", label: "Total size of stored bodies", unit: "MB", min: 1 },
  { key: "maxBodyKb", label: "Store at most", unit: "KB of each response body", min: 0 },
];

/** History retention (app-wide) and body storage (per workspace). PLAN.md Phase 5. */
export function HistorySettingsDialog(props: { hasWorkspace: boolean; onSaved?: () => void }) {
  const [settings, { mutate }] = createResource(() => api.historySettingsGet());
  const [storeBodies, setStoreBodies] = createSignal<boolean>();
  void (props.hasWorkspace
    ? attempt(() => api.workspaceHistoryBodiesGet(), "Couldn't load the workspace setting").then(
        (v) => setStoreBodies(v ?? true),
      )
    : Promise.resolve());

  const save = async () => {
    const s = settings();
    if (!s) return;
    try {
      await api.historySettingsSet(s);
      if (props.hasWorkspace && storeBodies() !== undefined) {
        await api.workspaceHistoryBodiesSet(storeBodies() as boolean);
      }
    } catch (e) {
      notify(`Couldn't save the history settings: ${e instanceof Error ? e.message : e}`, "error");
      return;
    }
    notify("History settings saved", "info");
    props.onSaved?.();
    closeDialog();
  };

  return (
    <Modal
      title="History settings"
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button type="button" class="primary" onClick={() => void save()} disabled={!settings()}>
            Save
          </button>
        </>
      }
    >
      <Show when={settings()} fallback={<p>Loading…</p>}>
        {(s) => (
          <form
            class={styles.form}
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <p class={styles.muted}>
              History is stored on this computer only (not in the workspace). Secrets and sensitive
              headers are masked. Older entries are removed when any limit is reached.
            </p>
            <For each={FIELDS}>
              {(f) => (
                <label class={styles.field}>
                  <span>{f.label}</span>
                  <input
                    type="number"
                    min={f.min}
                    step={1}
                    aria-label={`${f.label} (${f.unit})`}
                    value={s()[f.key]}
                    onInput={(e) => {
                      const v = Math.max(f.min, Math.floor(Number(e.currentTarget.value) || 0));
                      mutate({ ...s(), [f.key]: v });
                    }}
                  />
                  <span class={styles.muted}>{f.unit}</span>
                </label>
              )}
            </For>
            <Show when={props.hasWorkspace}>
              <label class={styles.check}>
                <input
                  type="checkbox"
                  checked={storeBodies() ?? true}
                  disabled={storeBodies() === undefined}
                  onChange={(e) => setStoreBodies(e.currentTarget.checked)}
                />{" "}
                Store response bodies in history for this workspace
              </label>
              <p class={styles.muted}>
                Turn this off for workspaces whose responses contain sensitive data. Metadata
                (status, headers, timings) is still recorded.
              </p>
            </Show>
          </form>
        )}
      </Show>
    </Modal>
  );
}
