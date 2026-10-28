import type { RequestStore } from "../../stores/request";
import { MarkdownEditor } from "../common/Markdown";
import styles from "./panels.module.css";

/** The request's Markdown description (saved in the request file). */
export function DocsPanel(props: { request: RequestStore; readOnly?: boolean }) {
  return (
    <div class={styles.panel}>
      <MarkdownEditor
        label="Request description"
        value={props.request.draft().description ?? ""}
        onChange={(v) => props.request.setDescription(v)}
        readOnly={props.readOnly}
        placeholder="What this request does, required setup, example responses… (Markdown)"
      />
    </div>
  );
}
