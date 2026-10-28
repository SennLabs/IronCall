import { createResource, createSignal, For, Show } from "solid-js";
import { api, SendFailure } from "../../ipc/api";
import type { RequestPreview } from "../../ipc/bindings";
import { HEADER_SOURCE_LABEL, headerSourceTitle } from "../../model/previewHeaders";
import type { AppStore } from "../../stores/app";
import { closeDialog, openDialog } from "../../stores/dialogs";
import { formatBytes } from "../common/format";
import { Modal } from "../common/Modal";
import styles from "./vars.module.css";

/**
 * The request exactly as it would be sent (variables resolved), with secrets masked
 * unless "Reveal secrets" is on (PLAN.md Phase 3).
 */
export function PreviewDialog(props: { app: AppStore; tabKey: string }) {
  const { app, tabKey } = props;
  const tab = () => app.tabs.tabs().find((t) => t.key === tabKey);
  const [reveal, setReveal] = createSignal(false);
  const [preview] = createResource(
    () => {
      const t = tab();
      return t ? { draft: t.request.draft(), reveal: reveal(), v: app.vars.version() } : undefined;
    },
    async ({ draft, reveal }): Promise<RequestPreview | { error: string }> => {
      try {
        return await api.previewRequest(draft, reveal);
      } catch (e) {
        return { error: e instanceof SendFailure ? e.error.message : String(e) };
      }
    },
  );
  const ok = () => {
    const p = preview();
    return p && !("error" in p) ? p : undefined;
  };
  const error = () => {
    const p = preview();
    return p && "error" in p ? p.error : undefined;
  };

  return (
    <Modal
      title="Resolved request"
      wide
      onClose={closeDialog}
      footer={
        <>
          <button
            type="button"
            onClick={() => openDialog({ kind: "copyCurl", source: { tabKey } })}
          >
            Copy as cURL…
          </button>
          <button type="button" onClick={closeDialog}>
            Close
          </button>
        </>
      }
    >
      <div class={styles.form}>
        <label class={styles.actions}>
          <input
            type="checkbox"
            checked={reveal()}
            onChange={(e) => setReveal(e.currentTarget.checked)}
          />
          Reveal secrets
        </label>
        <Show when={error()}>
          <p class={styles.errorText} role="alert">
            {error()}
          </p>
        </Show>
        <Show when={ok()}>
          {(p) => (
            <>
              <section class={styles.previewUrl} aria-label="Resolved URL">
                <strong>{p().method}</strong> {p().url}
              </section>
              <table class={styles.previewTable} aria-label="Resolved headers">
                <thead>
                  <tr>
                    <th>Header</th>
                    <th>Value</th>
                    <th>Source</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={p().headers}>
                    {(h) => (
                      <tr title={headerSourceTitle(h)}>
                        <td classList={{ [styles.overridden as string]: !h.effective }}>
                          {h.name}
                        </td>
                        <td classList={{ [styles.overridden as string]: !h.effective }}>
                          {h.value}
                        </td>
                        <td>
                          <span class={styles.sourceBadge}>{HEADER_SOURCE_LABEL[h.source]}</span>
                          <Show when={h.origin}>
                            {" "}
                            <span class={styles.muted}>{h.origin}</span>
                          </Show>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
              <p class={styles.muted}>Body: {formatBytes(p().bodySize)}</p>
            </>
          )}
        </Show>
      </div>
    </Modal>
  );
}
