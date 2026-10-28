import { createResource, createSignal, Show } from "solid-js";
import {
  type AuthDraft,
  api,
  type CollectionDraft,
  type FolderDraft,
  type KeyValueRow,
  WorkspaceFailure,
} from "../../ipc/api";
import { COMMON_HEADERS, valueSuggestions } from "../../model/headers";
import { type ContainerTarget, closeDialog } from "../../stores/dialogs";
import { notify } from "../../stores/notify";
import { Modal } from "../common/Modal";
import { KeyValueTable } from "../request/KeyValueTable";
import { AuthEditor } from "./AuthEditor";
import styles from "./network.module.css";

/**
 * Auth and default headers of the collection or a folder (PLAN.md Phase 4). Requests
 * inside inherit both; a request header with the same name overrides a default one.
 */
export function ContainerSettingsDialog(props: { target: ContainerTarget }) {
  const target = props.target;
  const [loaded, { refetch }] = createResource(async () => {
    if (target.kind === "collection") {
      const c = await api.collectionLoad();
      return { file: c.collection as FolderDraft | CollectionDraft, hash: c.hash as string | null };
    }
    const f = await api.folderLoad(target.id);
    return { file: f.folder, hash: f.hash };
  });
  const [auth, setAuth] = createSignal<AuthDraft>();
  const [headers, setHeaders] = createSignal<KeyValueRow[]>();
  const [conflict, setConflict] = createSignal(false);

  const currentAuth = () => auth() ?? loaded()?.file.auth;
  const currentHeaders = () => headers() ?? loaded()?.file.headers ?? [];

  const save = async () => {
    const l = loaded();
    if (!l) return;
    const { auth: _a, headers: _h, ...rest } = l.file;
    const a = currentAuth();
    const h = currentHeaders().filter((r) => r.name.trim() !== "" || (r.value ?? "") !== "");
    // A bare `inherit` is the default, so it isn't written (keeps the file minimal).
    const bareInherit = !a || (a.type === "inherit" && Object.keys(a).length === 1);
    const file = {
      ...rest,
      ...(bareInherit ? {} : { auth: a }),
      ...(h.length ? { headers: h } : {}),
    } as FolderDraft;
    try {
      if (target.kind === "collection") await api.collectionSave(file, l.hash);
      else await api.folderSave(file, l.hash);
      closeDialog();
    } catch (e) {
      if (e instanceof WorkspaceFailure && e.conflict) {
        setConflict(true);
        return;
      }
      notify(`Couldn't save the settings: ${e instanceof Error ? e.message : String(e)}`, "error");
    }
  };

  const reload = async () => {
    setAuth(undefined);
    setHeaders(undefined);
    setConflict(false);
    await refetch();
  };

  const title = () =>
    target.kind === "collection"
      ? "Collection auth & headers"
      : `Folder auth & headers · ${target.name}`;

  return (
    <Modal
      title={title()}
      wide
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button
            type="button"
            class="primary"
            disabled={!loaded() || conflict()}
            onClick={() => void save()}
          >
            Save
          </button>
        </>
      }
    >
      <Show when={loaded()} fallback={<p>Loading…</p>}>
        <div class={styles.section}>
          <Show when={conflict()}>
            <div class={styles.card} role="alert">
              <span>
                <strong>This file changed on disk</strong> since you opened it.
              </span>
              <span>
                <button type="button" onClick={() => void reload()}>
                  Reload
                </button>
              </span>
            </div>
          </Show>
          <h3 class={styles.heading}>Auth</h3>
          <AuthEditor
            auth={currentAuth()}
            onChange={setAuth}
            inheritLabel={
              target.kind === "collection" ? "No auth (nothing to inherit)" : "Inherit from parent"
            }
          />
          <p class={styles.muted}>
            Requests set to <em>Inherit</em> use the nearest folder's auth, then the collection's.
          </p>
          <h3 class={styles.heading}>Default headers</h3>
          <KeyValueTable<KeyValueRow>
            label="Default headers"
            rows={currentHeaders()}
            toggle
            varsInNames
            nameSuggestions={COMMON_HEADERS}
            valueSuggestions={valueSuggestions}
            onChange={setHeaders}
          />
          <p class={styles.muted}>
            Sent with every request inside. Inner folders override outer ones; a request's own
            header with the same name wins.
          </p>
        </div>
      </Show>
    </Modal>
  );
}
