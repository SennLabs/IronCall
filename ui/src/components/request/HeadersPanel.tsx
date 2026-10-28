import { type Accessor, For, Show } from "solid-js";
import type { KeyValueRow } from "../../ipc/api";
import type { RequestPreview } from "../../ipc/bindings";
import { COMMON_HEADERS, valueSuggestions } from "../../model/headers";
import { HEADER_SOURCE_LABEL, headerSourceTitle } from "../../model/previewHeaders";
import type { RequestStore } from "../../stores/request";
import { KeyValueTable } from "./KeyValueTable";
import styles from "./panels.module.css";

export interface HeadersPanelProps {
  request: RequestStore;
  preview: Accessor<RequestPreview | undefined>;
}

/**
 * User headers plus read-only rows for headers from the body type, the auth, default
 * headers of folders and the collection, and the engine.
 */
export function HeadersPanel(props: HeadersPanelProps) {
  const autoRows = () => (props.preview()?.headers ?? []).filter((h) => h.source !== "user");
  return (
    <div class={styles.panel}>
      <KeyValueTable<KeyValueRow>
        label="Headers"
        rows={props.request.draft().headers ?? []}
        toggle
        varsInNames
        nameSuggestions={COMMON_HEADERS}
        valueSuggestions={valueSuggestions}
        onChange={(rows) => props.request.setHeaders(rows)}
        footer={
          <For each={autoRows()}>
            {(h) => (
              <tr class={styles.autoRow} title={headerSourceTitle(h)}>
                {/* The first column is the narrow checkbox column; keep it empty. */}
                <td />
                <td>
                  <span class={styles.badge}>{HEADER_SOURCE_LABEL[h.source]}</span>{" "}
                  <span classList={{ [styles.overridden as string]: !h.effective }}>{h.name}</span>
                </td>
                <td classList={{ [styles.overridden as string]: !h.effective }}>{h.value}</td>
                <td class={styles.origin}>{h.origin ?? ""}</td>
              </tr>
            )}
          </For>
        }
      />
      <Show when={autoRows().length > 0}>
        <p class={styles.hint}>
          Auto, auth and default headers are added for you. Add a header with the same name to
          override one.
        </p>
      </Show>
    </div>
  );
}
