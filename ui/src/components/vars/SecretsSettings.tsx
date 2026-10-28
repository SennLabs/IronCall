import { createSignal, For, onMount, Show } from "solid-js";
import { api, type SecretBackend } from "../../ipc/api";
import { closeDialog, promptSecret } from "../../stores/dialogs";
import { notify } from "../../stores/notify";
import type { VarsStore } from "../../stores/vars";
import { Modal } from "../common/Modal";
import styles from "./vars.module.css";

export const BACKEND_LABELS: Record<SecretBackend, { title: string; text: string }> = {
  keychain: {
    title: "OS keychain",
    text: "Windows Credential Manager or the Linux Secret Service. No passphrase needed.",
  },
  encryptedFile: {
    title: "Encrypted file",
    text: ".ironcall/secrets.enc, encrypted with your passphrase (asked once per session). A forgotten passphrase can't be recovered.",
  },
  plaintextFile: {
    title: "Plaintext file",
    text: ".ironcall/secrets.json, unencrypted. Only as safe as this disk; git-ignored, but never share the folder.",
  },
};

const ORDER: SecretBackend[] = ["keychain", "encryptedFile", "plaintextFile"];

/** Choose where this workspace's secret values are stored, migrating existing ones. */
export function SecretsSettings(props: { vars: VarsStore }) {
  const { vars } = props;
  const [choice, setChoice] = createSignal<SecretBackend | undefined>(vars.status()?.current);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string>();
  const status = () => vars.status();

  onMount(() => void vars.refresh());

  const disabledReason = (b: SecretBackend) =>
    b === "keychain" && status() && !status()?.keychainAvailable
      ? "No OS keychain is reachable on this machine (e.g. no Secret Service running)."
      : undefined;

  const apply = async () => {
    const target = choice();
    const s = status();
    if (!target || !s || target === s.current) return;
    setError(undefined);
    let passphrase: string | null = null;
    if (target === "encryptedFile") {
      passphrase = s.encryptedFileExists
        ? await promptSecret("Unlock encrypted file", "Passphrase", "Unlock")
        : await promptSecret(
            "Create encrypted file",
            "New passphrase (it can't be recovered if forgotten)",
            "Create",
            true,
          );
      if (passphrase === null) return;
    }
    if (target === "plaintextFile") {
      const ok = await api.confirm(
        "Secret values will be stored unencrypted in .ironcall/secrets.json. Anyone who can read this folder can read them. Continue?",
        { title: "Store secrets in plain text", okLabel: "Use plaintext file", kind: "warning" },
      );
      if (!ok) return;
    }
    setBusy(true);
    try {
      await api.secretBackendSet(target, passphrase);
      await vars.refresh();
      notify(`Secrets moved to the ${BACKEND_LABELS[target].title.toLowerCase()}.`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="Secret storage"
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Close
          </button>
          <button
            type="button"
            class="primary"
            disabled={
              busy() ||
              !choice() ||
              choice() === status()?.current ||
              !!disabledReason(choice() ?? "keychain")
            }
            onClick={() => void apply()}
          >
            {busy() ? "Migrating…" : "Switch and migrate"}
          </button>
        </>
      }
    >
      <div class={styles.form}>
        <p class={styles.muted}>
          Secret variables are declared in the workspace files, but their values are stored only on
          this machine. Switching moves every stored value to the new place.
        </p>
        <Show when={status()} fallback={<p class={styles.muted}>Open a workspace first.</p>}>
          {(s) => (
            <>
              <Show when={s().folderBackend}>
                {(b) => (
                  <p class={styles.warningText}>
                    This workspace's .ironcall/local.yaml asks for the{" "}
                    {BACKEND_LABELS[b()].title.toLowerCase()}. It isn't used, because a shared
                    folder could choose it for you. Switch here if you chose it yourself.
                  </p>
                )}
              </Show>
              <For each={ORDER}>
                {(b) => (
                  <label
                    class={styles.choice}
                    data-current={b === s().current || undefined}
                    data-disabled={disabledReason(b) ? "" : undefined}
                  >
                    <input
                      type="radio"
                      name="secret-backend"
                      checked={choice() === b}
                      disabled={!!disabledReason(b)}
                      onChange={() => setChoice(b)}
                    />
                    <span>
                      <strong>{BACKEND_LABELS[b].title}</strong>
                      <Show when={b === s().current}> (current)</Show>
                      <p>{BACKEND_LABELS[b].text}</p>
                      <Show when={disabledReason(b)}>
                        <p class={styles.warningText}>{disabledReason(b)}</p>
                      </Show>
                      <Show when={b === "plaintextFile"}>
                        <p class={styles.warningText}>Not encrypted.</p>
                      </Show>
                      <Show when={b === "encryptedFile" && b === s().current}>
                        <p>{s().unlocked ? "Unlocked for this session." : "Locked."}</p>
                      </Show>
                    </span>
                  </label>
                )}
              </For>
              <Show when={s().current === "encryptedFile" && !s().unlocked}>
                <button type="button" onClick={() => void vars.unlock()}>
                  Unlock now…
                </button>
              </Show>
            </>
          )}
        </Show>
        <Show when={error()}>
          <p class={styles.errorText} role="alert">
            {error()}
          </p>
        </Show>
      </div>
    </Modal>
  );
}
