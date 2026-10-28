import { For, Show } from "solid-js";
import type { AssertionResult, ExtractedView } from "../../ipc/api";
import styles from "./tests.module.css";

/** "3/4" plus whether everything passed, for tab labels and run rows. */
export function checkSummary(assertions: AssertionResult[], extracted: ExtractedView[]) {
  const passed = assertions.filter((a) => a.passed).length;
  const ok = passed === assertions.length && extracted.every((e) => e.error === null);
  return { passed, total: assertions.length, ok };
}

/**
 * Assertion and extraction results for one response (PLAN.md Phase 7). Used by the
 * response's Tests tab and the runner's per-request detail.
 */
export function CheckResults(props: { assertions: AssertionResult[]; extracted: ExtractedView[] }) {
  const summary = () => checkSummary(props.assertions, props.extracted);
  return (
    <div class={styles.results}>
      <Show when={props.assertions.length > 0}>
        <p class={`${styles.summary} ${summary().ok ? styles.pass : styles.fail}`} role="status">
          {summary().passed} of {summary().total} assertions passed
        </p>
        <ul class={styles.list} aria-label="Assertion results">
          <For each={props.assertions}>
            {(a) => (
              <li class={styles.item} aria-label={`${a.passed ? "Passed" : "Failed"}: ${a.label}`}>
                <span
                  class={`${styles.icon} ${a.passed ? styles.pass : styles.fail}`}
                  aria-hidden="true"
                >
                  {a.passed ? "✓" : "✗"}
                </span>
                <span class={styles.label}>{a.label}</span>
                <Show when={a.message}>
                  <span class={styles.detail}>{a.message}</span>
                </Show>
                <Show when={a.actual !== null && !a.passed}>
                  <span class={styles.detail}>
                    Actual: <code>{a.actual}</code>
                  </span>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={props.extracted.length > 0}>
        <h3 class={styles.heading}>Extracted variables</h3>
        <ul class={styles.list} aria-label="Extracted variables">
          <For each={props.extracted}>
            {(e) => (
              <li class={styles.item} aria-label={`Extracted ${e.var}`}>
                <span
                  class={`${styles.icon} ${e.error ? styles.fail : styles.pass}`}
                  aria-hidden="true"
                >
                  {e.error ? "✗" : "✓"}
                </span>
                <span class={styles.label}>
                  {e.var} = {e.value ?? ""}
                </span>
                <Show when={e.error}>
                  <span class={styles.detail}>{e.error}</span>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
