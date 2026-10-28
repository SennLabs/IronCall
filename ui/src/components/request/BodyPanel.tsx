import { createMemo, createResource, For, Match, Show, Switch } from "solid-js";
import { api, type KeyValueRow } from "../../ipc/api";
import { bodyFilePaths } from "../../model/bodyFiles";
import { formatJson, lintJson } from "../../model/json";
import { formatXml, lintXml } from "../../model/xml";
import {
  bodyContent,
  bodyFields,
  bodyFile,
  bodyParts,
  bodyType,
  EDITABLE_BODY_TYPES,
  type EditableBodyType,
  type RequestStore,
} from "../../stores/request";
import { type EditorIssue, SmallBodyEditor } from "../SmallBodyEditor";
import { useVars } from "../vars/VarsContext";
import { FileChip, MultipartTable } from "./BodyFiles";
import { KeyValueTable } from "./KeyValueTable";
import styles from "./panels.module.css";

const TYPE_LABELS: Record<EditableBodyType, string> = {
  none: "No body",
  json: "JSON",
  text: "Text",
  xml: "XML",
  formUrlencoded: "Form (urlencoded)",
  multipart: "Multipart form",
  binaryFile: "Binary file",
};

const TEXT_TYPES = new Set(["json", "text", "xml"]);

const jsonIssues = (text: string): EditorIssue[] => {
  const issue = lintJson(text);
  return issue
    ? [{ from: issue.offset, to: Math.min(issue.offset + 1, text.length), message: issue.message }]
    : [];
};

export function BodyPanel(props: { request: RequestStore }) {
  const vars = useVars();
  const type = () => bodyType(props.request.draft());
  const content = () => bodyContent(props.request.draft());
  const editable = () => (EDITABLE_BODY_TYPES as readonly string[]).includes(type());
  const textual = () => TEXT_TYPES.has(type());
  // Status of uploaded files (missing, outside the workspace and not picked this session).
  const [files] = createResource(
    () => {
      const paths = bodyFilePaths(props.request.draft());
      return paths.length ? JSON.stringify(paths) : null;
    },
    () => api.bodyFilesCheck(props.request.draft()),
  );
  const statuses = () => (files.error ? [] : (files() ?? []));

  const problem = createMemo(() => {
    if (type() === "json") {
      const issue = lintJson(content());
      return issue ? `Line ${issue.line}, column ${issue.column}: ${issue.message}` : null;
    }
    if (type() === "xml") return lintXml(content());
    return null;
  });

  const beautify = () => {
    const formatted = type() === "json" ? formatJson(content()) : formatXml(content());
    if (formatted !== null) props.request.setBodyContent(formatted);
  };

  return (
    <div class={styles.fill}>
      <div class={styles.toolbar}>
        <label>
          <span class={styles.hint}>Body </span>
          <select
            aria-label="Body type"
            value={editable() ? type() : ""}
            onChange={(e) => props.request.setBodyType(e.currentTarget.value as EditableBodyType)}
          >
            <Show when={!editable()}>
              <option value="" disabled>
                {type()} (not supported)
              </option>
            </Show>
            <For each={EDITABLE_BODY_TYPES}>
              {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
            </For>
          </select>
        </label>
        <span class={styles.spacer} />
        <Show when={type() === "json" || type() === "xml"}>
          <button
            type="button"
            onClick={beautify}
            disabled={problem() !== null || content().trim() === ""}
            title={problem() ? "Fix the syntax error first" : "Re-indent the body"}
          >
            Beautify
          </button>
        </Show>
      </div>
      <Switch>
        <Match when={type() === "none"}>
          <p class={styles.empty}>This request has no body.</p>
        </Match>
        <Match when={type() === "formUrlencoded"}>
          <div class={styles.panel}>
            <KeyValueTable<KeyValueRow>
              label="Form fields"
              rows={bodyFields(props.request.draft())}
              toggle
              varsInNames
              onChange={(rows) => props.request.setBodyFields(rows)}
            />
            <p class={styles.hint}>Sent as application/x-www-form-urlencoded.</p>
          </div>
        </Match>
        <Match when={type() === "multipart"}>
          <div class={styles.panel}>
            <MultipartTable
              parts={bodyParts(props.request.draft())}
              statuses={statuses()}
              onChange={(parts) => props.request.setMultipartParts(parts)}
            />
            <p class={styles.hint}>
              Sent as multipart/form-data. Files are streamed from disk when the request is sent.
            </p>
          </div>
        </Match>
        <Match when={type() === "binaryFile"}>
          <div class={styles.panel}>
            <div class={styles.form}>
              <span>File</span>
              <FileChip
                label="Body file"
                path={bodyFile(props.request.draft()).path}
                status={statuses().find((f) => f.path === bodyFile(props.request.draft()).path)}
                onPick={(file) =>
                  props.request.setBinaryFile({
                    path: file.path,
                    contentType: bodyFile(props.request.draft()).contentType ?? file.contentType,
                  })
                }
              />
              <label for="body-file-type">Content type</label>
              <input
                id="body-file-type"
                placeholder="application/octet-stream"
                value={bodyFile(props.request.draft()).contentType ?? ""}
                onInput={(e) =>
                  props.request.setBinaryFile({ contentType: e.currentTarget.value || null })
                }
              />
            </div>
            <p class={styles.hint}>
              The file's bytes are streamed as the body when the request is sent.
            </p>
          </div>
        </Match>
        <Match when={textual()}>
          <div class={styles.editor}>
            <SmallBodyEditor
              value={content()}
              language={type() === "json" ? "json" : type() === "xml" ? "xml" : "text"}
              lint={type() === "json" ? jsonIssues : undefined}
              ariaLabel="Request body"
              variables={vars}
              onChange={(v) => props.request.setBodyContent(v)}
            />
          </div>
          <div
            class={styles.status}
            classList={{ [styles.statusError as string]: problem() !== null }}
            role="status"
          >
            {problem() ??
              (type() === "json"
                ? "Valid JSON ({{variables}} allowed)"
                : `${content().length} characters`)}
          </div>
        </Match>
      </Switch>
    </div>
  );
}
