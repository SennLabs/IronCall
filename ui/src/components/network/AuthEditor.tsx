import { createUniqueId, For, type JSX, Match, Show, Switch } from "solid-js";
import type { AuthDraft, AuthType } from "../../ipc/api";
import {
  AUTH_LABELS,
  AUTH_TYPES,
  authOf,
  looksLikeLiteralSecret,
  patchAuth,
  withAuthType,
} from "../../model/auth";
import { VarInput } from "../vars/VarInput";
import styles from "./network.module.css";
import { SecretInput } from "./SecretInput";

export interface AuthEditorProps {
  auth: AuthDraft | undefined;
  onChange: (auth: AuthDraft) => void;
  /** Shown instead of fields when the type is `inherit` (e.g. the effective auth). */
  inherited?: JSX.Element;
  /** What `inherit` means here, e.g. "No auth" at the collection. */
  inheritLabel?: string;
  readOnly?: boolean;
}

/**
 * Auth type plus its fields (PLAN.md Phase 4). Used by requests, folders and the
 * collection. Every edit spreads the current object, so unknown keys survive.
 */
export function AuthEditor(props: AuthEditorProps) {
  const id = createUniqueId();
  const auth = () => authOf(props.auth);
  const set = (patch: Record<string, unknown>) => props.onChange(patchAuth(props.auth, patch));
  const field = (name: string) => {
    const v = (auth() as Record<string, unknown>)[name];
    return typeof v === "string" ? v : "";
  };
  const literalSecret = () => {
    const a = auth();
    const value =
      a.type === "basic"
        ? a.password
        : a.type === "bearer"
          ? a.token
          : a.type === "apiKey"
            ? a.value
            : "";
    return looksLikeLiteralSecret(value);
  };

  return (
    <fieldset class={`${styles.section} ${styles.fieldset}`} disabled={props.readOnly}>
      <div class={styles.form}>
        <label for={`${id}-type`}>Type</label>
        <select
          id={`${id}-type`}
          value={auth().type}
          onChange={(e) =>
            props.onChange(withAuthType(props.auth, e.currentTarget.value as AuthType))
          }
        >
          <For each={AUTH_TYPES}>
            {(t) => (
              <option value={t}>
                {t === "inherit" && props.inheritLabel ? props.inheritLabel : AUTH_LABELS[t]}
              </option>
            )}
          </For>
        </select>

        <Switch>
          <Match when={auth().type === "basic"}>
            <label for={`${id}-user`}>Username</label>
            <VarInput
              id={`${id}-user`}
              aria-label="Username"
              value={field("username")}
              onInput={(v) => set({ username: v })}
            />
            <label for={`${id}-pass`}>Password</label>
            <SecretInput
              id={`${id}-pass`}
              label="Password"
              value={field("password")}
              onInput={(v) => set({ password: v })}
            />
          </Match>
          <Match when={auth().type === "bearer"}>
            <label for={`${id}-token`}>Token</label>
            <SecretInput
              id={`${id}-token`}
              label="Token"
              placeholder="{{token}}"
              value={field("token")}
              onInput={(v) => set({ token: v })}
            />
          </Match>
          <Match when={auth().type === "apiKey"}>
            <label for={`${id}-key`}>Key</label>
            <VarInput
              id={`${id}-key`}
              aria-label="Key"
              placeholder="X-Api-Key"
              value={field("key")}
              onInput={(v) => set({ key: v })}
            />
            <label for={`${id}-value`}>Value</label>
            <SecretInput
              id={`${id}-value`}
              label="Value"
              placeholder="{{apiKey}}"
              value={field("value")}
              onInput={(v) => set({ value: v })}
            />
            <label for={`${id}-in`}>Add to</label>
            <select
              id={`${id}-in`}
              value={field("in") || "header"}
              onChange={(e) => set({ in: e.currentTarget.value })}
            >
              <option value="header">Header</option>
              <option value="query">Query params</option>
            </select>
          </Match>
        </Switch>
      </div>
      <Show when={auth().type === "inherit" && props.inherited}>{props.inherited}</Show>
      <Show when={auth().type === "none"}>
        <p class={styles.muted}>
          No auth is sent, even if a parent folder or the collection sets one.
        </p>
      </Show>
      <Show when={literalSecret()}>
        <p class={styles.hint} role="note">
          This looks like a real credential. Store it in a secret variable and write{" "}
          <code>{"{{name}}"}</code> here instead, so it never ends up in the file or in git.
        </p>
      </Show>
    </fieldset>
  );
}
