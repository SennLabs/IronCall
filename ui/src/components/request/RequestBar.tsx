import { createEffect, For, Show } from "solid-js";
import { api } from "../../ipc/api";
import { looksLikeCurl } from "../../model/curl";
import { prompt } from "../../stores/dialogs";
import { notify } from "../../stores/notify";
import type { RequestStore } from "../../stores/request";
import type { SendStore } from "../../stores/send";
import { VarInput } from "../vars/VarInput";
import styles from "./RequestBar.module.css";

export const COMMON_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] as const;

/** The dropdown's "Custom…" entry (not a valid method, so it can't clash). */
const CUSTOM = " custom";

const isCommon = (m: string) => (COMMON_METHODS as readonly string[]).includes(m);

/** An HTTP method is a token (RFC 9110 §5.6.2): letters, digits and ``!#$%&'*+-.^_`|~``. */
export const isMethodToken = (m: string) => /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(m);

/** `get`, `post`, … for colouring; `other` for custom methods. */
const methodClass = (m: string) =>
  isCommon(m) && m !== "HEAD" && m !== "OPTIONS" ? m.toLowerCase() : "other";

export interface RequestBarProps {
  request: RequestStore;
  sender: SendStore;
  onSend: () => void;
  /** Shows the request with variables resolved. */
  onPreview?: () => void;
  /** Called when a cURL command is pasted into the URL bar (PLAN.md Phase 6). */
  onPasteCurl?: (command: string) => void;
}

/** Method + URL + Send/Cancel. Ctrl+Enter sends and Esc cancels (handled in App). */
export function RequestBar(props: RequestBarProps) {
  let methodSelect!: HTMLSelectElement;
  // Set the value after the options render (a custom method adds one), so the select
  // never falls back to another option.
  createEffect(() => {
    methodSelect.value = props.request.draft().method;
  });
  return (
    <form
      class={styles.bar}
      onSubmit={(e) => {
        e.preventDefault();
        if (props.sender.isSending()) void props.sender.cancel();
        else props.onSend();
      }}
    >
      <select
        ref={methodSelect}
        class={styles.method}
        aria-label="Method"
        data-method={methodClass(props.request.draft().method)}
        onChange={async (e) => {
          const select = e.currentTarget;
          const current = props.request.draft().method;
          if (select.value !== CUSTOM) {
            props.request.setMethod(select.value);
            return;
          }
          // Keep showing the current method while asking (and if cancelled).
          select.value = current;
          const answer = await prompt(
            "Custom method",
            "Method (e.g. PROPFIND, PURGE)",
            isCommon(current) ? "" : current,
            "Use method",
          );
          const method = answer?.trim().toUpperCase();
          if (!method) return;
          if (!isMethodToken(method)) {
            notify(`"${method}" isn't a valid HTTP method`, "error");
            return;
          }
          props.request.setMethod(method);
        }}
      >
        <For each={COMMON_METHODS}>
          {(m) => (
            <option value={m} selected={props.request.draft().method === m}>
              {m}
            </option>
          )}
        </For>
        <Show when={!isCommon(props.request.draft().method)}>
          <option value={props.request.draft().method} selected>
            {props.request.draft().method}
          </option>
        </Show>
        <option value={CUSTOM}>Custom…</option>
      </select>
      <VarInput
        wrapClass={styles.urlWrap}
        class={styles.url}
        aria-label="URL"
        placeholder="https://api.example.com/users/:id?page=1"
        value={props.request.urlText()}
        onInput={(v) => props.request.setUrlText(v)}
        onPaste={(e) => {
          const text = e.clipboardData?.getData("text/plain") ?? "";
          if (!props.onPasteCurl || !looksLikeCurl(text)) return;
          // Decide synchronously (the paste can't be deferred), then confirm with the
          // backend; if it disagrees, insert the text as a normal paste would.
          e.preventDefault();
          const input = e.currentTarget;
          const start = input.selectionStart ?? input.value.length;
          const end = input.selectionEnd ?? start;
          // Insert into the text as it is when detection answers (the user may have
          // typed meanwhile), and as a plain paste if detection fails.
          const plainPaste = () => {
            const now = input.value;
            const at = Math.min(start, now.length);
            props.request.setUrlText(
              now.slice(0, at) + text + now.slice(Math.min(end, now.length)),
            );
          };
          void api
            .curlDetect(text)
            .then((isCurl) => (isCurl ? props.onPasteCurl?.(text) : plainPaste()))
            .catch(plainPaste);
        }}
      />
      <Show when={props.onPreview}>
        <button
          type="button"
          title="Show the request with variables resolved"
          onClick={() => props.onPreview?.()}
        >
          Preview
        </button>
      </Show>
      <button
        type="submit"
        class={props.sender.isSending() ? undefined : "primary"}
        classList={{ [styles.send as string]: true }}
        title={props.sender.isSending() ? "Cancel (Esc)" : "Send (Ctrl+Enter)"}
      >
        {props.sender.isSending() ? "Cancel" : "Send"}
      </button>
    </form>
  );
}
