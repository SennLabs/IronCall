import { createSignal, Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { BACKEND_LABELS } from "./SecretsSettings";
import styles from "./vars.module.css";

/**
 * App-level banners: "N secrets missing" (PLAN.md §4.5), a locked encrypted file, and
 * the one-time suggestion when the keychain isn't available (§6.1), and a secret backend
 * named by the workspace folder itself, which is not used (a repo could pick plaintext).
 */
export function SecretsBanner(props: { app: AppStore }) {
  const vars = () => props.app.vars;
  const [folderDismissed, setFolderDismissed] = createSignal(false);
  const folderBackend = () => (folderDismissed() ? null : vars().status()?.folderBackend);
  const locked = () => {
    const s = vars().status();
    return !!s && s.current === "encryptedFile" && !s.unlocked;
  };
  return (
    <>
      <Show when={locked()}>
        <div class={styles.banner} role="status" aria-label="Secrets locked">
          <span class={styles.text}>
            This workspace's secrets are in an encrypted file that is locked.
          </span>
          <button type="button" onClick={() => void vars().unlock()}>
            Unlock…
          </button>
        </div>
      </Show>
      <Show when={!locked() && vars().missingSummary()}>
        {(m) => (
          <div class={styles.banner} role="status" aria-label="Missing secrets">
            <span class={styles.text}>
              <strong>
                {m().count} secret{m().count === 1 ? "" : "s"} missing
              </strong>{" "}
              for {m().label}
            </span>
            <button
              type="button"
              class="primary"
              onClick={() => openDialog({ kind: "fillSecrets" })}
            >
              Enter now
            </button>
          </div>
        )}
      </Show>
      <Show when={folderBackend()}>
        {(b) => (
          <div class={styles.banner} role="status" aria-label="Secret storage not changed">
            <span class={styles.text}>
              This workspace's folder asks for secrets to be stored in the{" "}
              {BACKEND_LABELS[b()].title.toLowerCase()}. IronCall doesn't take that choice from the
              folder, so it uses the{" "}
              {BACKEND_LABELS[vars().status()?.current ?? "keychain"].title.toLowerCase()}. If you
              chose it yourself, switch to it.
            </span>
            <button type="button" onClick={() => openDialog({ kind: "secrets" })}>
              Choose storage…
            </button>
            <button type="button" class="ghost" onClick={() => setFolderDismissed(true)}>
              Dismiss
            </button>
          </div>
        )}
      </Show>
      <Show when={vars().keychainSuggestion()}>
        <div class={styles.banner} role="status" aria-label="Keychain unavailable">
          <span class={styles.text}>
            No OS keychain is available on this machine, so secrets can't be stored there. Switch
            this workspace to an encrypted file?
          </span>
          <button type="button" onClick={() => openDialog({ kind: "secrets" })}>
            Choose storage…
          </button>
          <button type="button" class="ghost" onClick={() => vars().dismissKeychainSuggestion()}>
            Dismiss
          </button>
        </div>
      </Show>
    </>
  );
}
