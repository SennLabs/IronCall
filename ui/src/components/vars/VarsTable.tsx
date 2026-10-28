import { Index, type JSX, Show } from "solid-js";
import { isValidVarName, MASK, type VarRow } from "../../model/variables";
import styles from "./VarsTable.module.css";

export interface SecretCellState {
  /** Value typed but not stored yet, or revealed from the store. */
  shown?: string;
  /** Known to have no stored value. */
  missing?: boolean;
  pending?: boolean;
}

export interface VarsTableProps {
  label: string;
  rows: readonly VarRow[];
  onChange: (rows: VarRow[]) => void;
  /** Show the secret column (not for request variables). */
  allowSecrets?: boolean;
  secretState?: (row: VarRow) => SecretCellState;
  onToggleSecret?: (index: number, secret: boolean) => void;
  onSetSecret?: (index: number) => void;
  onClearSecret?: (index: number) => void;
  onReveal?: (index: number) => void;
  readOnly?: boolean;
}

const blank = (name: string, value: string): VarRow => ({
  name,
  value,
  secret: false,
  description: "",
  extra: {},
});

/**
 * Editable variable rows: name, value, secret toggle, description (PLAN.md §5, §6).
 * Secret rows never show their value inline; they offer set/replace, clear and reveal.
 */
export function VarsTable(props: VarsTableProps) {
  const update = (i: number, patch: Partial<VarRow>) =>
    props.onChange(props.rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const remove = (i: number) => props.onChange(props.rows.filter((_, j) => j !== i));
  const add = (name: string, value: string) => props.onChange([...props.rows, blank(name, value)]);
  const duplicates = () => {
    const seen = new Map<string, number>();
    for (const r of props.rows) seen.set(r.name.trim(), (seen.get(r.name.trim()) ?? 0) + 1);
    return seen;
  };

  const secretCell = (row: () => VarRow, i: number): JSX.Element => {
    const state = () => props.secretState?.(row()) ?? {};
    return (
      <div class={styles.secret}>
        <output
          class={styles.secretValue}
          classList={{ [styles.missing as string]: !!state().missing && !state().pending }}
          aria-label={`Value of ${row().name || "secret"}`}
        >
          {state().shown ??
            (state().pending ? `${MASK} (not stored yet)` : state().missing ? "not set" : MASK)}
        </output>
        <Show when={!props.readOnly}>
          <button type="button" class="ghost" onClick={() => props.onSetSecret?.(i)}>
            {state().missing && !state().pending ? "Set" : "Replace"}
          </button>
          <Show when={!state().missing || state().pending}>
            <button type="button" class="ghost" onClick={() => props.onClearSecret?.(i)}>
              Clear
            </button>
          </Show>
        </Show>
        <Show when={!state().pending && !state().missing}>
          <button
            type="button"
            class="ghost"
            aria-pressed={state().shown !== undefined}
            onClick={() => props.onReveal?.(i)}
          >
            {state().shown !== undefined ? "Hide" : "Reveal"}
          </button>
        </Show>
      </div>
    );
  };

  return (
    <table class={styles.table} aria-label={props.label}>
      <thead>
        <tr>
          <th>Name</th>
          <th>Value</th>
          <Show when={props.allowSecrets}>
            <th class={styles.check}>Secret</th>
          </Show>
          <th>Description</th>
          <th class={styles.action}>
            <span class={styles.srOnly}>Actions</span>
          </th>
        </tr>
      </thead>
      <tbody>
        <Index each={props.rows}>
          {(row, i) => {
            const name = () => row().name.trim();
            const invalid = () =>
              name() !== "" && !isValidVarName(name())
                ? "Use letters, digits, _ . or -"
                : (duplicates().get(name()) ?? 0) > 1
                  ? "Duplicate name; the last one wins"
                  : undefined;
            return (
              <tr>
                <td>
                  <input
                    class={styles.cell}
                    classList={{ [styles.invalid as string]: !!invalid() }}
                    value={row().name}
                    readOnly={props.readOnly}
                    placeholder="name"
                    title={invalid()}
                    aria-invalid={!!invalid()}
                    aria-label={`Variable name ${i + 1}`}
                    spellcheck={false}
                    onInput={(e) => update(i, { name: e.currentTarget.value })}
                  />
                </td>
                <td>
                  <Show
                    when={row().secret}
                    fallback={
                      <input
                        class={styles.cell}
                        value={row().value}
                        readOnly={props.readOnly}
                        placeholder="value"
                        aria-label={`Value of ${row().name || `variable ${i + 1}`}`}
                        spellcheck={false}
                        onInput={(e) => update(i, { value: e.currentTarget.value })}
                      />
                    }
                  >
                    {secretCell(row, i)}
                  </Show>
                </td>
                <Show when={props.allowSecrets}>
                  <td class={styles.check}>
                    <input
                      type="checkbox"
                      checked={row().secret}
                      disabled={props.readOnly}
                      aria-label={`Secret ${row().name || `variable ${i + 1}`}`}
                      onChange={(e) => {
                        const on = e.currentTarget.checked;
                        // The parent decides (it may ask first); keep the box in sync.
                        e.currentTarget.checked = row().secret;
                        props.onToggleSecret?.(i, on);
                      }}
                    />
                  </td>
                </Show>
                <td>
                  <input
                    class={styles.cell}
                    value={row().description}
                    readOnly={props.readOnly}
                    placeholder="description"
                    aria-label={`Description of ${row().name || `variable ${i + 1}`}`}
                    onInput={(e) => update(i, { description: e.currentTarget.value })}
                  />
                </td>
                <td class={styles.action}>
                  <Show when={!props.readOnly}>
                    <button
                      type="button"
                      class="ghost"
                      aria-label={`Delete ${row().name || "variable"}`}
                      onClick={() => remove(i)}
                    >
                      ✕
                    </button>
                  </Show>
                </td>
              </tr>
            );
          }}
        </Index>
        <Show when={!props.readOnly}>
          <tr class={styles.ghostRow}>
            <td>
              <input
                class={styles.cell}
                value=""
                placeholder="Add variable"
                aria-label="New variable name"
                spellcheck={false}
                onInput={(e) => {
                  const v = e.currentTarget.value;
                  e.currentTarget.value = "";
                  add(v, "");
                  focusLast(e.currentTarget, 0);
                }}
              />
            </td>
            <td />
            <Show when={props.allowSecrets}>
              <td />
            </Show>
            <td />
            <td />
          </tr>
        </Show>
      </tbody>
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
