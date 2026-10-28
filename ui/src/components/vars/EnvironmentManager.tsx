import { createSignal, For, Show } from "solid-js";
import { api, type EnvironmentDraft, type ItemId } from "../../ipc/api";
import { closeDialog, prompt } from "../../stores/dialogs";
import { attempt } from "../../stores/notify";
import type { VarsStore } from "../../stores/vars";
import { createVarsEditor } from "../../stores/varsEditor";
import { Modal } from "../common/Modal";
import { VarsEditorView } from "./VarsEditorView";
import styles from "./vars.module.css";

/** Create, rename, duplicate and delete environments, and edit their variables. */
export function EnvironmentManager(props: { vars: VarsStore; select?: ItemId }) {
  const { vars } = props;
  const [selected, setSelected] = createSignal<ItemId | undefined>(
    props.select ?? vars.active() ?? vars.environments()[0]?.id,
  );
  const current = () => vars.environments().find((e) => e.id === selected());

  const create = async () => {
    const name = await prompt("New environment", "Name", "", "Create");
    if (!name?.trim()) return;
    const env = await vars.withUnlock(() => api.environmentCreate(name.trim()), "Couldn't create");
    await vars.refresh();
    if (env) setSelected(env.id);
  };
  const duplicate = async () => {
    const id = selected();
    if (!id) return;
    const env = await vars.withUnlock(() => api.environmentDuplicate(id), "Couldn't duplicate");
    await vars.refresh();
    if (env) setSelected(env.id);
  };
  const rename = async () => {
    const env = current();
    if (!env) return;
    const name = await prompt("Rename environment", "Name", env.name, "Rename");
    if (!name?.trim() || name.trim() === env.name) return;
    await attempt(() => api.environmentRename(env.id, name.trim()), "Couldn't rename");
    await vars.refresh();
  };
  const remove = async () => {
    const env = current();
    if (!env) return;
    const ok = await api.confirm(
      `Delete the environment "${env.name}"? The file goes to the trash and its stored secrets are deleted from this machine.`,
      { title: "Delete environment", okLabel: "Delete", kind: "warning" },
    );
    if (!ok) return;
    await vars.withUnlock(() => api.environmentDelete(env.id), "Couldn't delete");
    await vars.refresh();
    setSelected(vars.environments()[0]?.id);
  };

  return (
    <Modal title="Environments" wide onClose={closeDialog}>
      <div class={styles.manager}>
        <div class={styles.envList}>
          <Show when={vars.environments().length === 0}>
            <p class={styles.muted}>No environments yet.</p>
          </Show>
          <div role="listbox" aria-label="Environments">
            <For each={vars.environments()}>
              {(env) => (
                <button
                  type="button"
                  role="option"
                  class={styles.envItem}
                  aria-selected={env.id === selected()}
                  title={env.error ?? env.path}
                  onClick={() => setSelected(env.id)}
                >
                  <span class={styles.envName}>{env.name}</span>
                  <Show when={env.id === vars.active()}>
                    <span class={styles.activeDot} title="Active">
                      ●
                    </span>
                  </Show>
                  <Show when={env.missingSecrets > 0}>
                    <span class={styles.count} title={`${env.missingSecrets} secrets missing`}>
                      {env.missingSecrets}
                    </span>
                  </Show>
                  <Show when={env.error}>
                    <span class={styles.errorText}>⚠</span>
                  </Show>
                </button>
              )}
            </For>
          </div>
          <div class={styles.envTools}>
            <button type="button" onClick={() => void create()}>
              New
            </button>
            <button type="button" disabled={!current()} onClick={() => void duplicate()}>
              Duplicate
            </button>
            <button type="button" disabled={!current()} onClick={() => void rename()}>
              Rename
            </button>
            <button type="button" disabled={!current()} onClick={() => void remove()}>
              Delete
            </button>
          </div>
        </div>
        <Show
          when={current()}
          keyed
          fallback={<p class={styles.muted}>Create an environment to get started.</p>}
        >
          {(env) => (
            <Show
              when={!env.error}
              fallback={<p class={styles.errorText}>This file can't be read: {env.error}</p>}
            >
              <EnvironmentVars vars={vars} id={env.id} name={env.name} />
            </Show>
          )}
        </Show>
      </div>
    </Modal>
  );
}

function EnvironmentVars(props: { vars: VarsStore; id: ItemId; name: string }) {
  const { vars, id } = props;
  let file: EnvironmentDraft | undefined;
  const editor = createVarsEditor(
    {
      async load() {
        const loaded = await api.environmentLoad(id);
        file = loaded.environment;
        return { vars: loaded.environment.vars ?? {}, hash: loaded.hash };
      },
      save: (v, hash) => {
        if (!file) throw new Error("not loaded");
        return api.environmentSave({ ...file, vars: v }, hash);
      },
    },
    { kind: "environment", id },
    vars,
  );
  return (
    <div class={styles.editor}>
      <div class={styles.actions}>
        <strong>{props.name}</strong>
        <span class={styles.spacer} />
        <Show
          when={vars.active() === id}
          fallback={
            <button type="button" onClick={() => void vars.setActive(id)}>
              Make active
            </button>
          }
        >
          <span class={styles.muted}>Active environment</span>
        </Show>
      </div>
      <VarsEditorView editor={editor} label={`Variables of ${props.name}`} allowSecrets />
    </div>
  );
}
