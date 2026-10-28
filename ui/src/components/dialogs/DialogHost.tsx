import { createResource, createSignal, For, Match, Show, Switch } from "solid-js";
import { api, type CollectionDraft, type FolderDraft, type ItemId } from "../../ipc/api";
import { type DiffLine, diffLines, hasChanges } from "../../model/diff";
import { allContainers } from "../../model/tree";
import type { AppStore } from "../../stores/app";
import { closeDialog, dialog, type PromptRequest, promptRequest } from "../../stores/dialogs";
import { attempt, notify } from "../../stores/notify";
import { MarkdownEditor } from "../common/Markdown";
import { Modal } from "../common/Modal";
import { HistorySettingsDialog } from "../history/HistorySettingsDialog";
import { CopyCurlDialog } from "../import/CopyCurlDialog";
import { ImportDialog } from "../import/ImportDialog";
import { ContainerSettingsDialog } from "../network/ContainerSettingsDialog";
import { CookiesDialog } from "../network/CookiesDialog";
import { NetworkSettingsDialog } from "../network/NetworkSettingsDialog";
import { QuickOpen } from "../quickopen/QuickOpen";
import { RunnerDialog } from "../runner/RunnerDialog";
import { SettingsDialog } from "../settings/SettingsDialog";
import { EnvironmentManager } from "../vars/EnvironmentManager";
import { FillSecretsDialog } from "../vars/FillSecretsDialog";
import { PreviewDialog } from "../vars/PreviewDialog";
import { SecretsSettings } from "../vars/SecretsSettings";
import { VariablesDialog } from "../vars/VariablesDialog";
import styles from "./DialogHost.module.css";

/** Renders whichever in-app dialog is open (see `stores/dialogs.ts`). */
export function DialogHost(props: { app: AppStore }) {
  const of = <K extends NonNullable<ReturnType<typeof dialog>>["kind"]>(kind: K) => {
    const d = dialog();
    return d?.kind === kind ? (d as Extract<typeof d, { kind: K }>) : undefined;
  };
  return (
    <>
      <MainDialog app={props.app} of={of} />
      {/* Prompts stack above the main dialog. */}
      <Show when={promptRequest()} keyed>
        {(p) => <PromptDialog {...p} />}
      </Show>
    </>
  );
}

function MainDialog(props: {
  app: AppStore;
  of: <K extends NonNullable<ReturnType<typeof dialog>>["kind"]>(
    kind: K,
  ) => Extract<NonNullable<ReturnType<typeof dialog>>, { kind: K }> | undefined;
}) {
  return (
    <Switch>
      <Match when={props.of("environments")} keyed>
        {(d) => <EnvironmentManager vars={props.app.vars} select={d.select} />}
      </Match>
      <Match when={props.of("variables")} keyed>
        {(d) => <VariablesDialog vars={props.app.vars} target={d.target} />}
      </Match>
      <Match when={props.of("secrets")}>
        <SecretsSettings vars={props.app.vars} />
      </Match>
      <Match when={props.of("fillSecrets")}>
        <FillSecretsDialog vars={props.app.vars} />
      </Match>
      <Match when={props.of("containerSettings")} keyed>
        {(d) => <ContainerSettingsDialog target={d.target} />}
      </Match>
      <Match when={props.of("cookies")}>
        <CookiesDialog />
      </Match>
      <Match when={props.of("history")}>
        <HistorySettingsDialog
          hasWorkspace={!!props.app.workspace.info()}
          onSaved={() => void props.app.history.reload()}
        />
      </Match>
      <Match when={props.of("import")} keyed>
        {(d) => <ImportDialog app={props.app} initial={d.tab} />}
      </Match>
      <Match when={props.of("copyCurl")} keyed>
        {(d) => <CopyCurlDialog app={props.app} source={d.source} />}
      </Match>
      <Match when={props.of("settings")} keyed>
        {(d) => <SettingsDialog section={d.section} />}
      </Match>
      <Match when={props.of("quickOpen")}>
        <QuickOpen app={props.app} />
      </Match>
      <Match when={props.of("runner")}>
        <RunnerDialog app={props.app} />
      </Match>
      <Match when={props.of("network")}>
        <NetworkSettingsDialog hasWorkspace={!!props.app.workspace.info()} />
      </Match>
      <Match when={props.of("preview")} keyed>
        {(d) => <PreviewDialog app={props.app} tabKey={d.tabKey} />}
      </Match>
      <Match
        when={(() => {
          const d = dialog();
          return d?.kind === "saveTo" ? d : undefined;
        })()}
      >
        {(d) => <SaveToDialog app={props.app} tabKey={d().tabKey} />}
      </Match>
      <Match
        when={(() => {
          const d = dialog();
          return d?.kind === "compare" ? d : undefined;
        })()}
      >
        {(d) => <CompareDialog app={props.app} tabKey={d().tabKey} />}
      </Match>
      <Match
        when={(() => {
          const d = dialog();
          return d?.kind === "description" ? d : undefined;
        })()}
      >
        {(d) => <DescriptionDialog target={d().target} />}
      </Match>
    </Switch>
  );
}

function PromptDialog(props: PromptRequest) {
  const [value, setValue] = createSignal(props.initial);
  const [again, setAgain] = createSignal("");
  // Passwords may be anything, including spaces; text prompts are trimmed.
  const result = () => (props.password ? value() : value().trim());
  const mismatch = () => !!props.confirm && again() !== value();
  const valid = () => result() !== "" && !mismatch();
  return (
    <Modal
      title={props.title}
      onClose={() => props.resolve(null)}
      footer={
        <>
          <button type="button" onClick={() => props.resolve(null)}>
            Cancel
          </button>
          <button type="submit" form="prompt-form" class="primary" disabled={!valid()}>
            {props.okLabel}
          </button>
        </>
      }
    >
      <form
        id="prompt-form"
        class={styles.form}
        onSubmit={(e) => {
          e.preventDefault();
          if (valid()) props.resolve(result());
        }}
      >
        <label>
          {props.label}
          <input
            type={props.password ? "password" : "text"}
            autocomplete="off"
            value={value()}
            onInput={(e) => setValue(e.currentTarget.value)}
            autofocus
          />
        </label>
        <Show when={props.confirm}>
          <label>
            Repeat
            <input
              type="password"
              autocomplete="off"
              value={again()}
              aria-invalid={mismatch() && again() !== ""}
              onInput={(e) => setAgain(e.currentTarget.value)}
            />
          </label>
          <Show when={mismatch() && again() !== ""}>
            <span class={styles.mismatch}>The entries don't match.</span>
          </Show>
        </Show>
      </form>
    </Modal>
  );
}

/** Picks a folder in the open workspace for a scratch request. */
function SaveToDialog(props: { app: AppStore; tabKey: string }) {
  // Captured once: props come from a <Match> that goes stale when the dialog closes.
  const { app, tabKey } = props;
  const tree = () => app.workspace.tree();
  const [parent, setParent] = createSignal<ItemId | undefined>(tree()?.id);
  const tab = () => app.tabs.tabs().find((t) => t.key === tabKey);

  const save = async () => {
    const p = parent();
    if (!p) return;
    const title = tab()?.title() ?? "request";
    if (await app.tabs.saveToWorkspace(tabKey, p)) {
      closeDialog();
      notify(`Saved "${title}" to the workspace`);
    }
  };

  return (
    <Modal
      title="Save to workspace"
      onClose={closeDialog}
      footer={
        <Show when={tree()}>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button type="button" class="primary" disabled={!parent()} onClick={() => void save()}>
            Save
          </button>
        </Show>
      }
    >
      <Show
        when={tree()}
        fallback={
          <div class={styles.form}>
            <p>Open or create a workspace first.</p>
            <div class={styles.row}>
              <button
                type="button"
                class="primary"
                onClick={() => {
                  closeDialog();
                  void props.app.openWorkspace();
                }}
              >
                Open workspace…
              </button>
              <button
                type="button"
                onClick={() => {
                  closeDialog();
                  void props.app.createWorkspace();
                }}
              >
                Create workspace…
              </button>
            </div>
          </div>
        }
      >
        {(t) => (
          <div class={styles.form}>
            <label>
              Name
              <input
                value={tab()?.request.draft().name ?? ""}
                onInput={(e) => tab()?.request.setName(e.currentTarget.value)}
              />
            </label>
            <fieldset class={styles.folders}>
              <legend>Folder</legend>
              <For each={allContainers(t())}>
                {(c) => (
                  <label style={{ "padding-left": `${c.depth * 16}px` }}>
                    <input
                      type="radio"
                      name="save-to-folder"
                      checked={parent() === c.node.id}
                      onChange={() => setParent(c.node.id)}
                    />
                    {c.node.kind === "collection" ? `${c.node.name} (top level)` : c.node.name}
                  </label>
                )}
              </For>
            </fieldset>
          </div>
        )}
      </Show>
    </Modal>
  );
}

/** Side-by-side diff of my unsaved version against the file on disk. */
function CompareDialog(props: { app: AppStore; tabKey: string }) {
  const { app, tabKey } = props;
  const texts = () => app.tabs.compareTexts(tabKey);
  const diff = () => {
    const t = texts();
    return t ? diffLines(t.theirs, t.mine) : [];
  };
  const resolve = async (keep: boolean) => {
    closeDialog();
    if (keep) await app.tabs.keepMine(tabKey);
    else await app.tabs.takeTheirs(tabKey);
  };
  const sign = (l: DiffLine) => (l.op === "added" ? "+" : l.op === "removed" ? "−" : " ");
  return (
    <Modal
      title="Compare with disk"
      wide
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Close
          </button>
          <button type="button" onClick={() => void resolve(false)}>
            Take theirs
          </button>
          <button type="button" class="primary" onClick={() => void resolve(true)}>
            Keep mine
          </button>
        </>
      }
    >
      <Show when={texts()} fallback={<p>The conflict is resolved.</p>}>
        <p class={styles.legend}>
          <span class={styles.removed}>− on disk</span> <span class={styles.added}>+ yours</span>
        </p>
        <Show when={hasChanges(diff())} fallback={<p>The versions are identical.</p>}>
          <section aria-label="Differences">
            <pre class={styles.diff}>
              <For each={diff()}>
                {(l) => (
                  <div
                    class={styles.line}
                    classList={{
                      [styles.added as string]: l.op === "added",
                      [styles.removed as string]: l.op === "removed",
                    }}
                  >
                    {sign(l)} {l.text}
                  </div>
                )}
              </For>
            </pre>
          </section>
        </Show>
      </Show>
    </Modal>
  );
}

/** Markdown description of the collection (`ironcall.yaml`) or a folder (`_folder.yaml`). */
function DescriptionDialog(props: {
  target: { kind: "collection" } | { kind: "folder"; id: ItemId };
}) {
  const target = props.target;
  const [loaded] = createResource(async () => {
    const t = target;
    if (t.kind === "collection") {
      const c = await api.collectionLoad();
      return { file: c.collection as FolderDraft | CollectionDraft, hash: c.hash as string | null };
    }
    const f = await api.folderLoad(t.id);
    return { file: f.folder, hash: f.hash };
  });
  const [text, setText] = createSignal<string>();
  const value = () => text() ?? loaded()?.file.description ?? "";

  const save = async () => {
    const l = loaded();
    if (!l) return;
    const file = { ...l.file, description: value() || null };
    const ok = await attempt(
      () =>
        target.kind === "collection"
          ? api.collectionSave(file, l.hash)
          : api.folderSave(file, l.hash),
      "Couldn't save the description",
    );
    if (ok !== undefined) closeDialog();
  };

  return (
    <Modal
      title={`Description · ${loaded()?.file.name ?? ""}`}
      wide
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button type="button" class="primary" disabled={!loaded()} onClick={() => void save()}>
            Save
          </button>
        </>
      }
    >
      <Show when={loaded()} fallback={<p>Loading…</p>}>
        <MarkdownEditor label="Description" value={value()} onChange={setText} />
      </Show>
    </Modal>
  );
}
