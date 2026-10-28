import { createResource, For, Show } from "solid-js";
import { schemeUrl } from "../../ipc/scheme";
import { formatBytes } from "../common/format";
import styles from "./response.module.css";

export const HEX_PREVIEW_BYTES = 4096;

/** Formats bytes as `offset  hex…  ascii` rows of 16. */
export function hexRows(bytes: Uint8Array): string[] {
  const rows: string[] = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = bytes.subarray(off, off + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(chunk, (b) =>
      b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".",
    ).join("");
    rows.push(`${off.toString(16).padStart(8, "0")}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return rows;
}

async function fetchPrefix(bodyId: number): Promise<Uint8Array> {
  const res = await fetch(schemeUrl(`body/${bodyId}`), {
    headers: { Range: `bytes=0-${HEX_PREVIEW_BYTES - 1}` },
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return new Uint8Array(await res.arrayBuffer()).subarray(0, HEX_PREVIEW_BYTES);
}

/** Binary body: size plus a hex dump of the first 4KB (served by the `ironcall` scheme). */
export function HexView(props: { bodyId: number; size: number; contentType: string | null }) {
  const [bytes] = createResource(() => props.bodyId, fetchPrefix);
  return (
    <div class={styles.section}>
      <p class={styles.muted}>
        Binary body, {formatBytes(props.size)}
        {props.contentType ? ` (${props.contentType})` : ""}.
        {props.size > HEX_PREVIEW_BYTES
          ? ` Showing the first ${formatBytes(HEX_PREVIEW_BYTES)}.`
          : ""}
      </p>
      <Show when={!bytes.error} fallback={<p class={styles.muted}>Preview unavailable.</p>}>
        <pre class={styles.hex}>
          <For each={hexRows(bytes() ?? new Uint8Array())}>{(row) => `${row}\n`}</For>
        </pre>
      </Show>
    </div>
  );
}
