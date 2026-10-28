import { Show } from "solid-js";
import type { RequestPreview } from "../../ipc/bindings";
import { formatBytes } from "../common/format";
import { HeadersTable } from "./HeadersTable";
import styles from "./response.module.css";

/** The request exactly as it was sent, including engine-added headers. */
export function SentRequest(props: { request: RequestPreview; onCopyCurl?: () => void }) {
  return (
    <div class={styles.section}>
      <Show when={props.onCopyCurl}>
        <div>
          <button type="button" onClick={() => props.onCopyCurl?.()}>
            Copy as cURL…
          </button>
        </div>
      </Show>
      <p class={styles.requestLine}>
        <strong>{props.request.method}</strong> <code>{props.request.url}</code>
      </p>
      <HeadersTable
        label="Request headers"
        headers={props.request.headers.filter((h) => h.effective)}
      />
      <p class={styles.muted}>Body: {formatBytes(props.request.bodySize)}</p>
    </div>
  );
}
