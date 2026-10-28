import { createUniqueId, For, Index, type JSX, Show } from "solid-js";
import { VarInput } from "../vars/VarInput";
import styles from "./KeyValueTable.module.css";

export interface KvRow {
  name: string;
  value?: string;
  enabled?: boolean;
}

export interface KeyValueTableProps<T extends KvRow> {
  rows: readonly T[];
  onChange: (rows: T[]) => void;
  /** Builds a new row when the user types into the empty last row. */
  newRow?: (name: string, value: string) => T;
  /** Show the enable checkbox (query params, headers). */
  toggle?: boolean;
  /** Names are fixed (path params come from the URL). Disables adding and deleting. */
  fixedNames?: boolean;
  nameLabel?: string;
  valueLabel?: string;
  nameSuggestions?: readonly string[];
  valueSuggestions?: (name: string) => readonly string[];
  /** Allow `{{variables}}` in names too (headers, query params). Values always allow them. */
  varsInNames?: boolean;
  /** Extra read-only rows after the editable ones (e.g. auto headers). */
  footer?: JSX.Element;
  label: string;
}

/**
 * Editable name/value table. Rows are updated by spreading, so unknown keys on row
 * objects survive edits. `<Index>` keeps inputs (and focus) stable while typing.
 */
export function KeyValueTable<T extends KvRow>(props: KeyValueTableProps<T>) {
  const id = createUniqueId();
  const update = (index: number, patch: Partial<KvRow>) =>
    props.onChange(props.rows.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  const remove = (index: number) => props.onChange(props.rows.filter((_, i) => i !== index));
  const add = (name: string, value: string) => {
    const row = props.newRow?.(name, value) ?? ({ name, value, enabled: true } as T);
    props.onChange([...props.rows, row]);
  };

  const valueListId = (name: string) => {
    const options = props.valueSuggestions?.(name) ?? [];
    return options.length ? `${id}-v-${name.toLowerCase()}` : undefined;
  };

  return (
    <table class={styles.table} aria-label={props.label}>
      <thead>
        <tr>
          <Show when={props.toggle}>
            <th class={styles.check}>
              <span class={styles.srOnly}>Enabled</span>
            </th>
          </Show>
          <th>{props.nameLabel ?? "Name"}</th>
          <th>{props.valueLabel ?? "Value"}</th>
          <Show when={!props.fixedNames}>
            <th class={styles.action}>
              <span class={styles.srOnly}>Actions</span>
            </th>
          </Show>
        </tr>
      </thead>
      <tbody>
        <Index each={props.rows}>
          {(row, i) => (
            <tr classList={{ [styles.disabled as string]: row().enabled === false }}>
              <Show when={props.toggle}>
                <td class={styles.check}>
                  <input
                    type="checkbox"
                    checked={row().enabled !== false}
                    aria-label={`Enable ${row().name || "row"}`}
                    onChange={(e) => update(i, { enabled: e.currentTarget.checked })}
                  />
                </td>
              </Show>
              <td>
                <Show
                  when={props.varsInNames && !props.fixedNames}
                  fallback={
                    <input
                      class={styles.cell}
                      value={row().name}
                      readOnly={props.fixedNames}
                      placeholder={props.nameLabel ?? "Name"}
                      aria-label={`${props.nameLabel ?? "Name"} ${i + 1}`}
                      list={props.nameSuggestions ? `${id}-names` : undefined}
                      onInput={(e) => update(i, { name: e.currentTarget.value })}
                    />
                  }
                >
                  <VarInput
                    class={styles.cell}
                    value={row().name}
                    placeholder={props.nameLabel ?? "Name"}
                    aria-label={`${props.nameLabel ?? "Name"} ${i + 1}`}
                    list={props.nameSuggestions ? `${id}-names` : undefined}
                    onInput={(v) => update(i, { name: v })}
                  />
                </Show>
              </td>
              <td>
                <VarInput
                  class={styles.cell}
                  value={row().value ?? ""}
                  placeholder={props.valueLabel ?? "Value"}
                  aria-label={`${props.valueLabel ?? "Value"} ${i + 1}`}
                  list={valueListId(row().name)}
                  onInput={(v) => update(i, { value: v })}
                />
              </td>
              <Show when={!props.fixedNames}>
                <td class={styles.action}>
                  <button
                    type="button"
                    class="ghost"
                    aria-label={`Delete ${row().name || "row"}`}
                    onClick={() => remove(i)}
                  >
                    ✕
                  </button>
                </td>
              </Show>
            </tr>
          )}
        </Index>
        <Show when={!props.fixedNames}>
          <tr class={styles.ghostRow}>
            <Show when={props.toggle}>
              <td class={styles.check} />
            </Show>
            <td>
              <input
                class={styles.cell}
                value=""
                placeholder={`Add ${(props.nameLabel ?? "name").toLowerCase()}`}
                aria-label={`New ${(props.nameLabel ?? "name").toLowerCase()}`}
                list={props.nameSuggestions ? `${id}-names` : undefined}
                onInput={(e) => {
                  const v = e.currentTarget.value;
                  e.currentTarget.value = "";
                  add(v, "");
                  focusLast(e.currentTarget, 0);
                }}
              />
            </td>
            <td>
              <input
                class={styles.cell}
                value=""
                placeholder={props.valueLabel ?? "Value"}
                aria-label={`New ${(props.valueLabel ?? "value").toLowerCase()}`}
                onInput={(e) => {
                  const v = e.currentTarget.value;
                  e.currentTarget.value = "";
                  add("", v);
                  focusLast(e.currentTarget, 1);
                }}
              />
            </td>
            <td class={styles.action} />
          </tr>
        </Show>
        {props.footer}
      </tbody>
      <Show when={props.nameSuggestions}>
        <datalist id={`${id}-names`}>
          <For each={props.nameSuggestions}>{(name) => <option value={name} />}</For>
        </datalist>
      </Show>
      <Show when={props.valueSuggestions}>
        <For each={[...new Set(props.rows.map((r) => r.name.toLowerCase()))]}>
          {(name) => (
            <Show when={valueListId(name)}>
              {(listId) => (
                <datalist id={listId()}>
                  <For each={props.valueSuggestions?.(name) ?? []}>
                    {(v) => <option value={v} />}
                  </For>
                </datalist>
              )}
            </Show>
          )}
        </For>
      </Show>
    </table>
  );
}

/** After the ghost row becomes a real row, move focus into it at the same column. */
function focusLast(from: HTMLElement, column: number) {
  const ghost = from.closest("tr");
  queueMicrotask(() => {
    const inputs = ghost?.previousElementSibling?.querySelectorAll<HTMLInputElement>(
      "input:not([type=checkbox])",
    );
    const input = inputs?.[column];
    if (!input) return;
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  });
}
