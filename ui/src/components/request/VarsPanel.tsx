import { createEffect, createSignal, on } from "solid-js";
import { rowsToVars, type VarRow, varsToRows } from "../../model/variables";
import type { RequestStore } from "../../stores/request";
import { VarsTable } from "../vars/VarsTable";
import styles from "./panels.module.css";

/**
 * Request-scope variables (highest precedence below runtime, PLAN.md §5). They're saved
 * with the request, so they can't be secret; use an environment for secrets.
 */
export function VarsPanel(props: { request: RequestStore; readOnly?: boolean }) {
  const [rows, setRows] = createSignal<VarRow[]>(varsToRows(props.request.draft().vars));
  // Follow outside changes (undo, reload from disk) without fighting local edits.
  createEffect(
    on(
      () => props.request.draft().vars,
      (vars) => {
        if (JSON.stringify(vars ?? {}) !== JSON.stringify(rowsToVars(rows()))) {
          setRows(varsToRows(vars));
        }
      },
      { defer: true },
    ),
  );
  return (
    <div class={styles.panel}>
      <VarsTable
        label="Request variables"
        rows={rows()}
        readOnly={props.readOnly}
        onChange={(next) => {
          setRows(next);
          props.request.setVars(rowsToVars(next));
        }}
      />
      <p class={styles.hint}>
        Request variables override folder, environment, collection and global ones. They're saved in
        the request file, so keep secrets in an environment instead.
      </p>
    </div>
  );
}
