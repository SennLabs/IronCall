import { createEffect, createResource, createSignal, For, on, Show } from "solid-js";
import {
  api,
  type CurlDialect,
  type CurlExport,
  type CurlExportMode,
  type RequestDraft,
} from "../../ipc/api";
import { loadExportPrefs, saveExportPrefs } from "../../model/curl";
import type { AppStore } from "../../stores/app";
import { type CopyCurlSource, closeDialog } from "../../stores/dialogs";
import { notify } from "../../stores/notify";
import { Modal } from "../common/Modal";
import styles from "./import.module.css";

const MODES: { mode: CurlExportMode; label: string; hint: string }[] = [
  {
    mode: "variables",
    label: "With variables",
    hint: "{{variables}} left as they are. Safe to share.",
  },
  {
    mode: "masked",
    label: "Resolved, secrets masked",
    hint: "Variables filled in; secret values and sensitive headers shown as ••••.",
  },
  {
    mode: "resolved",
    label: "Resolved with secrets",
    hint: "Everything filled in, including secret values.",
  },
];

/** "Copy as cURL" for a tab's request or a request in the tree (PLAN.md Phase 6). */
export function CopyCurlDialog(props: { app: AppStore; source: CopyCurlSource }) {
  // Captured once: props come from a <Match> that goes stale when the dialog closes.
  const { app, source } = props;
  const prefs = loadExportPrefs();
  const [dialect, setDialect] = createSignal<CurlDialect>(prefs.dialect);
  const [mode, setMode] = createSignal<CurlExportMode>(prefs.mode);
  const [confirmed, setConfirmed] = createSignal(false);
  const [error, setError] = createSignal<string>();

  const [request] = createResource<RequestDraft | undefined>(async () => {
    if ("tabKey" in source)
      return app.tabs
        .tabs()
        .find((t) => t.key === source.tabKey)
        ?.request.draft();
    const open = app.tabs.findByRequest(source.requestId);
    if (open) return open.request.draft();
    return (await api.requestLoad(source.requestId)).request;
  });

  // Secrets are only fetched once the user has confirmed.
  const ready = () => mode() !== "resolved" || confirmed();
  const [exported, setExported] = createSignal<CurlExport>();
  let generation = 0;
  createEffect(
    on([request, dialect, mode, confirmed], ([r, d, m]) => {
      const g = ++generation;
      setExported(undefined);
      setError(undefined);
      if (!r || !ready()) return;
      api.curlExport(r, d, m).then(
        (x) => g === generation && setExported(x),
        (e: unknown) => g === generation && setError(e instanceof Error ? e.message : String(e)),
      );
    }),
  );

  const choose = (m: CurlExportMode) => {
    setMode(m);
    if (m !== "resolved") setConfirmed(false);
    saveExportPrefs({ dialect: dialect(), mode: m });
  };
  const chooseDialect = (d: CurlDialect) => {
    setDialect(d);
    saveExportPrefs({ dialect: d, mode: mode() });
  };

  const copy = async () => {
    const x = exported();
    if (!x) return;
    try {
      await navigator.clipboard.writeText(x.command);
      notify(mode() === "resolved" ? "Copied (includes secret values)" : "Copied cURL command");
      closeDialog();
    } catch (e) {
      notify(`Couldn't copy: ${e instanceof Error ? e.message : String(e)}`, "error");
    }
  };

  return (
    <Modal
      title="Copy as cURL"
      wide
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button type="button" class="primary" disabled={!exported()} onClick={() => void copy()}>
            Copy
          </button>
        </>
      }
    >
      <div class={styles.pane}>
        <fieldset class={styles.segmented} aria-label="Shell">
          <label class={styles.check}>
            <input
              type="radio"
              name="curl-dialect"
              checked={dialect() === "posix"}
              onChange={() => chooseDialect("posix")}
            />
            bash
          </label>
          <label class={styles.check}>
            <input
              type="radio"
              name="curl-dialect"
              checked={dialect() === "powerShell"}
              onChange={() => chooseDialect("powerShell")}
            />
            PowerShell
          </label>
        </fieldset>
        <fieldset class={styles.options} aria-label="Values">
          <For each={MODES}>
            {(m) => (
              <label class={styles.check} title={m.hint}>
                <input
                  type="radio"
                  name="curl-mode"
                  checked={mode() === m.mode}
                  onChange={() => choose(m.mode)}
                />
                {m.label}
                <span class={styles.muted}> — {m.hint}</span>
              </label>
            )}
          </For>
        </fieldset>
        <Show when={mode() === "resolved"}>
          <label class={`${styles.check} ${styles.danger}`}>
            <input
              type="checkbox"
              checked={confirmed()}
              onChange={(e) => setConfirmed(e.currentTarget.checked)}
            />
            This copies secret values to the clipboard
          </label>
        </Show>
        <Show when={error()}>
          <p class={styles.error} role="alert">
            {error()}
          </p>
        </Show>
        <Show
          when={exported()}
          fallback={
            <p class={styles.muted}>
              {request.loading || (ready() && !error())
                ? "Building…"
                : "Confirm to include secrets."}
            </p>
          }
        >
          {(x) => (
            <>
              <textarea
                class={styles.code}
                readOnly
                rows={Math.min(14, x().command.split("\n").length + 1)}
                aria-label="cURL command"
                value={x().command}
              />
              <Show when={x().warnings.length > 0}>
                <ul class={styles.list} aria-label="Export warnings">
                  <For each={x().warnings}>
                    {(w) => (
                      <li>
                        <Show when={w.location}>
                          <code>{w.location}</code>{" "}
                        </Show>
                        {w.message}
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </>
          )}
        </Show>
      </div>
    </Modal>
  );
}
