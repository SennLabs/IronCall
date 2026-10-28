/**
 * App-level variables state (PLAN.md Phase 3): environments of the open workspace, the
 * active environment, missing secrets, and the secret backend's status.
 *
 * `version` changes whenever resolved values may have changed (environment switch,
 * saved variables, stored secrets); per-tab previews and variable contexts depend on
 * it so they re-resolve.
 */

import { batch, createSignal } from "solid-js";
import {
  api,
  type EnvironmentSummary,
  type ItemId,
  type MissingSecret,
  type SecretBackendStatus,
  VarsFailure,
} from "../ipc/api";
import { promptSecret } from "./dialogs";
import { attempt, notify } from "./notify";

export type VarsStore = ReturnType<typeof createVarsStore>;

const SUGGESTION_KEY = "ironcall.keychainSuggestionDismissed";

export function createVarsStore() {
  const [environments, setEnvironments] = createSignal<EnvironmentSummary[]>([]);
  const [active, setActiveSignal] = createSignal<ItemId | null>(null);
  const [missing, setMissing] = createSignal<MissingSecret[]>([]);
  const [status, setStatus] = createSignal<SecretBackendStatus | null>(null);
  const [version, setVersion] = createSignal(0);
  const [workspaceOpen, setWorkspaceOpen] = createSignal(false);
  const [suggestionDismissed, setSuggestionDismissed] = createSignal(readDismissed());

  const bump = () => setVersion((v) => v + 1);

  /** Prompts for the passphrase until it unlocks or the user cancels. */
  const unlock = async (): Promise<boolean> => {
    let message = "Enter the passphrase for this workspace's encrypted secrets file.";
    for (;;) {
      const passphrase = await promptSecret("Unlock secrets", message, "Unlock");
      if (passphrase === null) return false;
      try {
        await api.secretsUnlock(passphrase);
        await store.refresh();
        return true;
      } catch (e) {
        message = e instanceof Error ? `${e.message}. Try again.` : String(e);
      }
    }
  };

  /**
   * Runs a secrets operation; if the encrypted file is locked, asks for the passphrase
   * and retries once. Other failures are reported and resolve `undefined`.
   */
  const withUnlock = async <T>(
    action: () => Promise<T>,
    failure: string,
  ): Promise<T | undefined> => {
    try {
      return await action();
    } catch (e) {
      if (e instanceof VarsFailure && e.locked && (await unlock())) {
        return attempt(action, failure);
      }
      notify(`${failure}: ${e instanceof Error ? e.message : String(e)}`, "error");
      return undefined;
    }
  };

  const store = {
    environments,
    active,
    missing,
    status,
    version,
    bump,
    withUnlock,
    unlock,

    activeEnvironment: () => environments().find((e) => e.id === active()),

    /** Shows the one-time "keychain unavailable" suggestion (PLAN.md §6.1). */
    keychainSuggestion: () => {
      const s = status();
      return !!s && s.current === "keychain" && !s.keychainAvailable && !suggestionDismissed();
    },
    dismissKeychainSuggestion() {
      setSuggestionDismissed(true);
      try {
        localStorage.setItem(SUGGESTION_KEY, "1");
      } catch {
        // Not persisted; fine.
      }
    },

    /** Reloads everything; call when a workspace opens/closes or files change. */
    async refresh(open: boolean = workspaceOpen()) {
      setWorkspaceOpen(open);
      const [envs, current, backend, miss] = open
        ? await Promise.all([
            api.environmentsList().catch(() => [] as EnvironmentSummary[]),
            api.activeEnvironmentGet().catch(() => null),
            api.secretBackendStatus().catch(() => null),
            api.secretsMissing().catch(() => [] as MissingSecret[]),
          ])
        : [[], null, null, await api.secretsMissing().catch(() => [] as MissingSecret[])];
      batch(() => {
        setEnvironments(envs);
        setActiveSignal(current && envs.some((e) => e.id === current) ? current : null);
        setStatus(backend);
        setMissing(miss);
        bump();
      });
    },

    async setActive(id: ItemId | null) {
      // `attempt` resolves undefined on failure, so map the void result to `true`.
      const ok = await attempt(
        () => api.activeEnvironmentSet(id).then(() => true),
        "Couldn't switch environment",
      );
      if (!ok) return;
      setActiveSignal(id);
      setMissing(await api.secretsMissing().catch(() => missing()));
      bump();
    },

    /** Missing secrets for the banner, grouped label like "Environment › Dev". */
    missingSummary(): { count: number; label: string } | undefined {
      const list = missing();
      if (list.length === 0) return undefined;
      const labels = [...new Set(list.map((m) => m.scopeLabel))];
      return { count: list.length, label: labels.join(", ") };
    },
  };
  return store;
}

function readDismissed(): boolean {
  try {
    return localStorage.getItem(SUGGESTION_KEY) === "1";
  } catch {
    return false;
  }
}
