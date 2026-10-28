import { createSignal, For, Index, Show } from "solid-js";
import type {
  AssertionDraft,
  AssertOp,
  AssertTarget,
  ExtractionDraft,
  ExtractSource,
} from "../../ipc/api";
import {
  ASSERT_OPS,
  ASSERT_TARGETS,
  alwaysSensitive,
  assertionField,
  EXTRACT_SOURCES,
  extractionField,
  FIELD_PLACEHOLDER,
  formatExpected,
  needsValue,
  newAssertion,
  newExtraction,
  parseExpected,
  VALUE_TYPES,
  withOp,
  withSource,
  withTarget,
} from "../../model/checks";
import type { RequestStore } from "../../stores/request";
import { VarInput } from "../vars/VarInput";
import kv from "./KeyValueTable.module.css";
import styles from "./panels.module.css";

/**
 * Declarative assertions and extractions (PLAN.md Phase 7). They run after every send
 * and in collection runs; results show in the response's Tests tab.
 */
export function TestsPanel(props: { request: RequestStore; readOnly?: boolean }) {
  const assertions = () => props.request.draft().assert ?? [];
  const extractions = () => props.request.draft().extract ?? [];
  const setAssertion = (i: number, next: AssertionDraft) =>
    props.request.setAssertions(assertions().map((a, j) => (j === i ? next : a)));
  const setExtraction = (i: number, next: ExtractionDraft) =>
    props.request.setExtractions(extractions().map((e, j) => (j === i ? next : e)));

  return (
    <div class={styles.panel}>
      <h3 class={styles.heading}>Assertions</h3>
      <Show
        when={assertions().length > 0}
        fallback={<p class={styles.hint}>No assertions. Add one to check every response.</p>}
      >
        <table class={kv.table} aria-label="Assertions">
          <colgroup>
            <col class={kv.check} />
            <col class={styles.selectCol} />
            <col />
            <col class={styles.selectCol} />
            <col />
            <col class={kv.action} />
          </colgroup>
          <thead>
            <tr>
              <th class={kv.check}>
                <span class={kv.srOnly}>Enabled</span>
              </th>
              <th>Target</th>
              <th>Path or header</th>
              <th>Check</th>
              <th>Expected</th>
              <th class={kv.action}>
                <span class={kv.srOnly}>Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            <Index each={assertions()}>
              {(a, i) => {
                const field = () => assertionField(a().target);
                // While typing, show the text as typed; reformat from the stored value on
                // blur. Re-rendering the parsed value on each key would turn "200," into
                // `[200,""]` and "1.0" into "1".
                const [typing, setTyping] = createSignal<string>();
                const label = () => `assertion ${i + 1}`;
                return (
                  <tr classList={{ [kv.disabled as string]: a().enabled === false }}>
                    <td class={kv.check}>
                      <input
                        type="checkbox"
                        checked={a().enabled !== false}
                        disabled={props.readOnly}
                        aria-label={`Enable ${label()}`}
                        onChange={(e) =>
                          setAssertion(i, { ...a(), enabled: e.currentTarget.checked })
                        }
                      />
                    </td>
                    <td>
                      <select
                        class={kv.cell}
                        aria-label={`Target of ${label()}`}
                        disabled={props.readOnly}
                        onChange={(e) =>
                          setAssertion(i, withTarget(a(), e.currentTarget.value as AssertTarget))
                        }
                      >
                        <For each={ASSERT_TARGETS}>
                          {(t) => (
                            <option value={t.key} selected={t.key === a().target}>
                              {t.label}
                            </option>
                          )}
                        </For>
                      </select>
                    </td>
                    <td>
                      <Show when={field()}>
                        {(f) => (
                          <VarInput
                            class={kv.cell}
                            value={(f() === "path" ? a().path : a().name) ?? ""}
                            placeholder={f() === "path" ? FIELD_PLACEHOLDER.path : "Header name"}
                            aria-label={`${f() === "path" ? "JSONPath" : "Header name"} of ${label()}`}
                            readOnly={props.readOnly}
                            onInput={(v) => setAssertion(i, { ...a(), [f()]: v })}
                          />
                        )}
                      </Show>
                    </td>
                    <td>
                      <select
                        class={kv.cell}
                        aria-label={`Check of ${label()}`}
                        disabled={props.readOnly}
                        onChange={(e) =>
                          setAssertion(i, withOp(a(), e.currentTarget.value as AssertOp))
                        }
                      >
                        <For each={ASSERT_OPS}>
                          {(o) => (
                            <option value={o.key} selected={o.key === a().op}>
                              {o.label}
                            </option>
                          )}
                        </For>
                      </select>
                    </td>
                    <td>
                      <Show when={needsValue(a().op)}>
                        <Show
                          when={a().op === "type"}
                          fallback={
                            <VarInput
                              class={kv.cell}
                              value={typing() ?? formatExpected(a().value, a().op)}
                              placeholder={a().op === "in" ? "200, 201" : "Expected value"}
                              aria-label={`Expected value of ${label()}`}
                              readOnly={props.readOnly}
                              onInput={(v) => {
                                setTyping(v);
                                setAssertion(i, { ...a(), value: parseExpected(v, a().op) });
                              }}
                              onBlur={() => setTyping(undefined)}
                            />
                          }
                        >
                          <select
                            class={kv.cell}
                            aria-label={`Expected type of ${label()}`}
                            disabled={props.readOnly}
                            onChange={(e) =>
                              setAssertion(i, { ...a(), value: e.currentTarget.value })
                            }
                          >
                            <For each={VALUE_TYPES}>
                              {(t) => (
                                <option value={t} selected={t === a().value}>
                                  {t}
                                </option>
                              )}
                            </For>
                          </select>
                        </Show>
                      </Show>
                    </td>
                    <td class={kv.action}>
                      <Show when={!props.readOnly}>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Delete ${label()}`}
                          onClick={() =>
                            props.request.setAssertions(assertions().filter((_, j) => j !== i))
                          }
                        >
                          ✕
                        </button>
                      </Show>
                    </td>
                  </tr>
                );
              }}
            </Index>
          </tbody>
        </table>
      </Show>
      <Show when={!props.readOnly}>
        <div>
          <button
            type="button"
            onClick={() => props.request.setAssertions([...assertions(), newAssertion()])}
          >
            Add assertion
          </button>
        </div>
      </Show>
      <p class={styles.hint}>
        Body values are typed: <code>201</code> and <code>"201"</code> differ. Expected values can
        use <code>{"{{variables}}"}</code>, compared as text.
      </p>

      <h3 class={styles.heading}>Extract into variables</h3>
      <Show
        when={extractions().length > 0}
        fallback={
          <p class={styles.hint}>
            No extractions. Save a value from the response (an ID, a token) to use in later
            requests.
          </p>
        }
      >
        <table class={kv.table} aria-label="Extractions">
          <colgroup>
            <col />
            <col class={styles.selectCol} />
            <col />
            <col class={styles.narrow} />
            <col class={kv.action} />
          </colgroup>
          <thead>
            <tr>
              <th>Variable</th>
              <th>From</th>
              <th>Path, name or pattern</th>
              <th class={styles.narrow}>Sensitive</th>
              <th class={kv.action}>
                <span class={kv.srOnly}>Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            <Index each={extractions()}>
              {(e, i) => {
                const field = () => extractionField(e().from);
                const label = () => `extraction ${i + 1}`;
                const forced = () => alwaysSensitive(e());
                return (
                  <tr>
                    <td>
                      <input
                        class={kv.cell}
                        value={e().var}
                        placeholder="userId"
                        aria-label={`Variable of ${label()}`}
                        readOnly={props.readOnly}
                        onInput={(ev) => setExtraction(i, { ...e(), var: ev.currentTarget.value })}
                      />
                    </td>
                    <td>
                      <select
                        class={kv.cell}
                        aria-label={`Source of ${label()}`}
                        disabled={props.readOnly}
                        onChange={(ev) =>
                          setExtraction(i, withSource(e(), ev.currentTarget.value as ExtractSource))
                        }
                      >
                        <For each={EXTRACT_SOURCES}>
                          {(s) => (
                            <option value={s.key} selected={s.key === e().from}>
                              {s.label}
                            </option>
                          )}
                        </For>
                      </select>
                    </td>
                    <td>
                      <input
                        class={kv.cell}
                        value={e()[field()] ?? ""}
                        placeholder={FIELD_PLACEHOLDER[field()]}
                        aria-label={`${field() === "path" ? "JSONPath" : field() === "name" ? "Name" : "Pattern"} of ${label()}`}
                        readOnly={props.readOnly}
                        onInput={(ev) =>
                          setExtraction(i, { ...e(), [field()]: ev.currentTarget.value })
                        }
                      />
                    </td>
                    <td class={styles.narrow}>
                      <input
                        type="checkbox"
                        checked={forced() || !!e().sensitive}
                        disabled={props.readOnly || forced()}
                        title={
                          forced()
                            ? "Always sensitive: cookies, auth headers and token-like names"
                            : "Mask in the UI, history and logs"
                        }
                        aria-label={`Sensitive ${label()}`}
                        onChange={(ev) =>
                          setExtraction(i, { ...e(), sensitive: ev.currentTarget.checked })
                        }
                      />
                    </td>
                    <td class={kv.action}>
                      <Show when={!props.readOnly}>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Delete ${label()}`}
                          onClick={() =>
                            props.request.setExtractions(extractions().filter((_, j) => j !== i))
                          }
                        >
                          ✕
                        </button>
                      </Show>
                    </td>
                  </tr>
                );
              }}
            </Index>
          </tbody>
        </table>
      </Show>
      <Show when={!props.readOnly}>
        <div>
          <button
            type="button"
            onClick={() => props.request.setExtractions([...extractions(), newExtraction()])}
          >
            Add extraction
          </button>
        </div>
      </Show>
      <p class={styles.hint}>
        Extracted values are runtime variables: they override every other scope until the app
        closes, and are never written to files. Save one to the environment from the variables quick
        look.
      </p>
    </div>
  );
}
