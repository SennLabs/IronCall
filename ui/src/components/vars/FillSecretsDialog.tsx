import { createSignal, For, Show } from "solid-js";
import { api } from "../../ipc/api";
import { closeDialog } from "../../stores/dialogs";
import { notify } from "../../stores/notify";
import type { VarsStore } from "../../stores/vars";
import { Modal } from "../common/Modal";
import styles from "./vars.module.css";

/**
 * Enter values for declared secrets that have none on this machine, e.g. after cloning
 * a teammate's workspace (PLAN.md §4.5).
 */
export function FillSecretsDialog(props: { vars: VarsStore }) {
  const { vars } = props;
  const list = vars.missing();
  const [values, setValues] = createSignal<Record<number, string>>({});
  const [busy, setBusy] = createSignal(false);
  const filled = () => Object.values(values()).filter((v) => v !== "").length;

  const save = async () => {
    setBusy(true);
    let stored = 0;
    for (const [i, value] of Object.entries(values())) {
      const m = list[Number(i)];
      if (!m || value === "") continue;
      const ok = await vars.withUnlock(
        () => api.secretSet(m.scope, m.name, value).then(() => true),
        `Couldn't store ${m.name}`,
      );
      if (ok) stored++;
    }
    setBusy(false);
    await vars.refresh();
    if (stored) notify(`Stored ${stored} secret${stored === 1 ? "" : "s"}.`);
    closeDialog();
  };

  return (
    <Modal
      title="Missing secrets"
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button
            type="submit"
            form="fill-secrets"
            class="primary"
            disabled={busy() || filled() === 0}
          >
            Save
          </button>
        </>
      }
    >
      <form
        id="fill-secrets"
        class={styles.form}
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <p class={styles.muted}>
          These secrets are declared in the workspace but have no value on this machine. Values go
          to the secret store, never into the files.
        </p>
        <Show when={list.length > 0} fallback={<p>Nothing is missing.</p>}>
          <For each={list}>
            {(m, i) => (
              <label>
                <span>
                  <code>{m.name}</code> <span class={styles.sourceBadge}>{m.scopeLabel}</span>
                </span>
                <input
                  type="password"
                  autocomplete="off"
                  aria-label={`${m.name} (${m.scopeLabel})`}
                  value={values()[i()] ?? ""}
                  onInput={(e) => {
                    const v = e.currentTarget.value;
                    setValues((all) => ({ ...all, [i()]: v }));
                  }}
                />
              </label>
            )}
          </For>
        </Show>
      </form>
    </Modal>
  );
}
