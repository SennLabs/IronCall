import { Index, Show } from "solid-js";
import { api, type BodyFileStatus, type MultipartPartDraft, type PickedFile } from "../../ipc/api";
import { attempt } from "../../stores/notify";
import { formatBytes } from "../common/format";
import netStyles from "../network/network.module.css";
import { VarInput } from "../vars/VarInput";
import kvStyles from "./KeyValueTable.module.css";

/** Picks a body file through the backend (it allows the file for this session). */
export const pickFile = () => attempt(() => api.pickBodyFile(), "Couldn't open the file picker");

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

/** Explains why a file can't be sent, or `undefined` if it can. */
export function fileProblem(status: BodyFileStatus | undefined): string | undefined {
  if (!status) return undefined;
  if (!status.exists) return "File not found";
  if (!status.allowed) return "Outside the workspace: re-select this file to allow it this session";
  return undefined;
}

/** A file chip: name, size and badges; clicking it picks another file. */
export function FileChip(props: {
  path: string;
  status?: BodyFileStatus;
  onPick: (file: PickedFile) => void;
  label: string;
}) {
  const problem = () => fileProblem(props.status);
  const pick = async () => {
    const file = await pickFile();
    if (file) props.onPick(file);
  };
  return (
    <span class={netStyles.actions}>
      <Show
        when={props.path}
        fallback={
          <button type="button" aria-label={props.label} onClick={() => void pick()}>
            Choose file…
          </button>
        }
      >
        <button
          type="button"
          class={netStyles.chip}
          classList={{ [netStyles.chipBad as string]: !!problem() }}
          aria-label={`${props.label}: ${props.path}`}
          title={problem() ?? props.path}
          onClick={() => void pick()}
        >
          {fileName(props.path)}
          <Show when={props.status?.size != null}>
            <span class={netStyles.muted}>{formatBytes(props.status?.size ?? 0)}</span>
          </Show>
        </button>
        <Show when={props.status?.outsideWorkspace}>
          <span class={`${netStyles.badge} ${netStyles.badgeWarn}`}>outside workspace</span>
        </Show>
        <Show when={problem()}>
          <span class={netStyles.error} role="alert">
            {problem()}
          </span>
        </Show>
      </Show>
    </span>
  );
}

/** Multipart parts: text or file rows (PLAN.md Phase 4). Rows are updated by spreading. */
export function MultipartTable(props: {
  parts: readonly MultipartPartDraft[];
  statuses: readonly BodyFileStatus[];
  onChange: (parts: MultipartPartDraft[]) => void;
}) {
  const update = (i: number, patch: Partial<MultipartPartDraft>) =>
    props.onChange(props.parts.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  const statusOf = (path: string | null | undefined) => props.statuses.find((s) => s.path === path);

  return (
    <table class={kvStyles.table} aria-label="Multipart parts">
      <thead>
        <tr>
          <th class={kvStyles.check}>
            <span class={kvStyles.srOnly}>Enabled</span>
          </th>
          <th>Name</th>
          <th>Kind</th>
          <th>Value</th>
          <th>Content type</th>
          <th class={kvStyles.action}>
            <span class={kvStyles.srOnly}>Actions</span>
          </th>
        </tr>
      </thead>
      <tbody>
        <Index each={props.parts}>
          {(part, i) => (
            <tr classList={{ [kvStyles.disabled as string]: part().enabled === false }}>
              <td class={kvStyles.check}>
                <input
                  type="checkbox"
                  checked={part().enabled !== false}
                  aria-label={`Enable ${part().name || "part"}`}
                  onChange={(e) => update(i, { enabled: e.currentTarget.checked })}
                />
              </td>
              <td>
                <VarInput
                  class={kvStyles.cell}
                  value={part().name}
                  placeholder="Name"
                  aria-label={`Part name ${i + 1}`}
                  onInput={(v) => update(i, { name: v })}
                />
              </td>
              <td>
                <select
                  aria-label={`Part kind ${i + 1}`}
                  value={part().type ?? "text"}
                  onChange={(e) => update(i, { type: e.currentTarget.value as "text" | "file" })}
                >
                  <option value="text">Text</option>
                  <option value="file">File</option>
                </select>
              </td>
              <td>
                <Show
                  when={(part().type ?? "text") === "file"}
                  fallback={
                    <VarInput
                      class={kvStyles.cell}
                      value={part().value ?? ""}
                      placeholder="Value"
                      aria-label={`Part value ${i + 1}`}
                      onInput={(v) => update(i, { value: v })}
                    />
                  }
                >
                  <FileChip
                    label={`Part file ${i + 1}`}
                    path={part().path ?? ""}
                    status={statusOf(part().path)}
                    onPick={(file) =>
                      update(i, {
                        path: file.path,
                        contentType: part().contentType || file.contentType,
                      })
                    }
                  />
                </Show>
              </td>
              <td>
                <input
                  class={kvStyles.cell}
                  value={part().contentType ?? ""}
                  placeholder="auto"
                  aria-label={`Part content type ${i + 1}`}
                  onInput={(e) => update(i, { contentType: e.currentTarget.value || null })}
                />
              </td>
              <td class={kvStyles.action}>
                <button
                  type="button"
                  class="ghost"
                  aria-label={`Delete ${part().name || "part"}`}
                  onClick={() => props.onChange(props.parts.filter((_, j) => j !== i))}
                >
                  ✕
                </button>
              </td>
            </tr>
          )}
        </Index>
        <tr>
          <td colSpan={6}>
            <span class={netStyles.actions}>
              <button
                type="button"
                onClick={() =>
                  props.onChange([
                    ...props.parts,
                    { name: "", type: "text", value: "", enabled: true },
                  ])
                }
              >
                Add text part
              </button>
              <button
                type="button"
                onClick={async () => {
                  const file = await pickFile();
                  if (!file) return;
                  props.onChange([
                    ...props.parts,
                    {
                      name: file.fileName.replace(/\.[^.]+$/, "") || "file",
                      type: "file",
                      path: file.path,
                      contentType: file.contentType,
                      enabled: true,
                    },
                  ]);
                }}
              >
                Add file part…
              </button>
            </span>
          </td>
        </tr>
      </tbody>
    </table>
  );
}
