import DOMPurify from "dompurify";
import { marked } from "marked";
import { createMemo, createSignal, Show } from "solid-js";
import { api } from "../../ipc/api";
import styles from "./Markdown.module.css";

/** Renders Markdown to sanitised HTML (no scripts, styles or event handlers). */
export function renderMarkdown(text: string): string {
  const html = marked.parse(text, { async: false, gfm: true, breaks: false }) as string;
  // No forms or form controls: a workspace's description could otherwise submit (and
  // navigate the app window) to a remote page.
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["form", "input", "button", "textarea", "select", "option"],
    FORBID_ATTR: ["formaction", "action"],
  });
}

/** Opens links in the system browser instead of navigating the app. */
function onPreviewClick(e: MouseEvent) {
  const link = (e.target as HTMLElement).closest("a");
  if (!link) return;
  e.preventDefault();
  const href = link.getAttribute("href");
  if (href && /^(https?:|mailto:)/i.test(href)) void api.openUrl(href);
}

export interface MarkdownEditorProps {
  value: string;
  onChange: (value: string) => void;
  label: string;
  placeholder?: string;
  readOnly?: boolean;
}

/** Markdown text with an Edit / Preview toggle. Starts in preview when there's text. */
export function MarkdownEditor(props: MarkdownEditorProps) {
  const [editing, setEditing] = createSignal(!props.value);
  const html = createMemo(() => renderMarkdown(props.value));
  return (
    <div class={styles.editor}>
      <div class={styles.toolbar} role="toolbar" aria-label={`${props.label} mode`}>
        <button
          type="button"
          aria-pressed={editing()}
          classList={{ primary: editing() }}
          onClick={() => setEditing(true)}
          disabled={props.readOnly}
        >
          Edit
        </button>
        <button
          type="button"
          aria-pressed={!editing()}
          classList={{ primary: !editing() }}
          onClick={() => setEditing(false)}
        >
          Preview
        </button>
        <span class={styles.hint}>Markdown</span>
      </div>
      <Show
        when={editing() && !props.readOnly}
        fallback={
          <Show when={props.value.trim()} fallback={<p class={styles.empty}>No description.</p>}>
            {/* biome-ignore lint/a11y/useKeyWithClickEvents: links inside are focusable and open with Enter natively. */}
            <section
              class={styles.preview}
              aria-label={`${props.label} preview`}
              innerHTML={html()}
              onClick={onPreviewClick}
            />
          </Show>
        }
      >
        <textarea
          class={styles.text}
          aria-label={props.label}
          value={props.value}
          placeholder={props.placeholder ?? "Describe this… (Markdown)"}
          onInput={(e) => props.onChange(e.currentTarget.value)}
        />
      </Show>
    </div>
  );
}
