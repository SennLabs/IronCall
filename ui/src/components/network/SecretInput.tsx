import { createSignal, Show } from "solid-js";
import { VarInput } from "../vars/VarInput";
import styles from "./network.module.css";

/**
 * A masked field with a show toggle. While masked it's a plain password input (the
 * `{{variable}}` overlay would reveal the text); shown, it's a full `VarInput`.
 */
export function SecretInput(props: {
  value: string;
  onInput: (value: string) => void;
  label: string;
  id?: string;
  placeholder?: string;
}) {
  const [shown, setShown] = createSignal(false);
  return (
    <span class={styles.secret}>
      <Show
        when={shown()}
        fallback={
          <input
            id={props.id}
            type="password"
            autocomplete="off"
            value={props.value}
            placeholder={props.placeholder}
            aria-label={props.label}
            onInput={(e) => props.onInput(e.currentTarget.value)}
          />
        }
      >
        <VarInput
          id={props.id}
          value={props.value}
          placeholder={props.placeholder}
          aria-label={props.label}
          onInput={props.onInput}
        />
      </Show>
      <button
        type="button"
        class="ghost"
        aria-pressed={shown()}
        aria-label={shown() ? `Hide ${props.label}` : `Show ${props.label}`}
        onClick={() => setShown(!shown())}
      >
        {shown() ? "Hide" : "Show"}
      </button>
    </span>
  );
}
