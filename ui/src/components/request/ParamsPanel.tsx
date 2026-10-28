import { Show } from "solid-js";
import type { KeyValueRow, PathParamRow } from "../../ipc/api";
import type { RequestStore } from "../../stores/request";
import { KeyValueTable } from "./KeyValueTable";
import styles from "./panels.module.css";

export function ParamsPanel(props: { request: RequestStore }) {
  const draft = () => props.request.draft();
  return (
    <div class={styles.panel}>
      <h3 class={styles.heading}>Query parameters</h3>
      <KeyValueTable<KeyValueRow>
        label="Query parameters"
        rows={draft().params ?? []}
        toggle
        varsInNames
        onChange={(rows) => props.request.setParams(rows)}
      />
      <Show when={(draft().pathParams ?? []).length > 0}>
        <h3 class={styles.heading}>Path parameters</h3>
        <KeyValueTable<PathParamRow>
          label="Path parameters"
          rows={draft().pathParams ?? []}
          fixedNames
          onChange={(rows) => props.request.setPathParams(rows)}
        />
      </Show>
      <p class={styles.hint}>
        Use <code>:name</code> or <code>{"{name}"}</code> in the URL path to add a path parameter.
        Values are encoded when the request is sent; disabled rows aren't sent.
      </p>
    </div>
  );
}
