import { For } from "solid-js";
import type { HeaderRow } from "../../ipc/bindings";
import { CopyButton } from "../common/CopyButton";
import styles from "./response.module.css";

export function HeadersTable(props: { headers: readonly HeaderRow[]; label: string }) {
  return (
    <table class={styles.headers} aria-label={props.label}>
      <tbody>
        <For each={props.headers}>
          {(h) => (
            <tr>
              <th scope="row">{h.name}</th>
              <td>{h.value}</td>
              <td class={styles.copyCell}>
                <CopyButton text={h.value} label="Copy" />
              </td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}
