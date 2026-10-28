import { onCleanup, Show } from "solid-js";
import type { BodyHit } from "../../ipc/bindings";
import { createBodySearch } from "../../stores/bodySearch";
import styles from "./response.module.css";

export interface BodySearchProps {
  bodyId: number;
  byteLen: number;
  onHit: (hit: BodyHit | undefined) => void;
  ref?: (input: HTMLInputElement) => void;
}

/**
 * Search for the paged viewer, run in Rust in both directions. Typing searches as you
 * go; Enter / Shift+Enter move to the next / previous match (wrapping around).
 */
export function BodySearch(props: BodySearchProps) {
  const search = createBodySearch({
    bodyId: props.bodyId,
    byteLen: props.byteLen,
    onHit: (h) => props.onHit(h),
  });
  onCleanup(() => search.dispose());

  return (
    <search class={styles.search}>
      <input
        ref={props.ref}
        type="search"
        aria-label="Search body"
        placeholder="Search body"
        value={search.query()}
        onInput={(e) => search.setQuery(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (e.shiftKey) void search.previous();
            else void search.next();
          }
        }}
      />
      <button
        type="button"
        onClick={() => void search.previous()}
        disabled={!search.query()}
        aria-label="Previous match"
      >
        ↑
      </button>
      <button
        type="button"
        onClick={() => void search.next()}
        disabled={!search.query()}
        aria-label="Next match"
      >
        ↓
      </button>
      <label class={styles.muted}>
        <input
          type="checkbox"
          checked={!search.ignoreCase()}
          onChange={(e) => search.setIgnoreCase(!e.currentTarget.checked)}
        />{" "}
        Match case
      </label>
      <Show when={search.message()}>
        <span class={styles.muted} role="status">
          {search.message()}
        </span>
      </Show>
    </search>
  );
}
