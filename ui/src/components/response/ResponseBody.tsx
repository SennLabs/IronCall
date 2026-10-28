import { openSearchPanel } from "@codemirror/search";
import type { EditorView } from "@codemirror/view";
import {
  createEffect,
  createResource,
  createSignal,
  Match,
  on,
  onCleanup,
  Show,
  Switch,
} from "solid-js";
import { api, type DerivedBody } from "../../ipc/api";
import type { BodyHit, ResponseBodyInfo } from "../../ipc/bindings";
import { AUTO_PRETTY_LIMIT, bodyMode, suggestedFileName } from "../../model/bodyView";
import { attempt, notify } from "../../stores/notify";
import { LargeBodyViewer, type LargeBodyViewerApi } from "../body-viewer/LargeBodyViewer";
import { CopyButton } from "../common/CopyButton";
import { formatBytes } from "../common/format";
import { type EditorLanguage, SmallBodyEditor } from "../SmallBodyEditor";
import { BodySearch } from "./BodySearch";
import { HexView } from "./HexView";
import styles from "./response.module.css";

export { SMALL_BODY_LIMIT } from "../../model/bodyView";

function languageFor(kind: ResponseBodyInfo["kind"]): EditorLanguage {
  if (kind === "json") return "json";
  if (kind === "xml" || kind === "html") return "xml";
  return "text";
}

export interface ResponseBodyProps {
  body: ResponseBodyInfo;
  status: number;
  /** Final request URL, used to suggest a file name for "Save response…". */
  url?: string;
}

/** "Save response…": native Save-as dialog run by Rust. */
export function SaveButton(props: { bodyId: number; url?: string; contentType: string | null }) {
  const save = async () => {
    const path = await attempt(
      () => api.bodySaveAs(props.bodyId, suggestedFileName(props.url ?? "", props.contentType)),
      "Couldn't save the response",
    );
    if (path) notify(`Saved to ${path}`, "info");
  };
  return (
    <button type="button" onClick={() => void save()}>
      Save response…
    </button>
  );
}

function PrettyToggle(props: { pretty: boolean; onChange: (pretty: boolean) => void }) {
  return (
    <fieldset class={styles.segmented} aria-label="Body view">
      <button
        type="button"
        aria-pressed={props.pretty}
        class={props.pretty ? "primary" : undefined}
        onClick={() => props.onChange(true)}
      >
        Pretty
      </button>
      <button
        type="button"
        aria-pressed={!props.pretty}
        class={!props.pretty ? "primary" : undefined}
        onClick={() => props.onChange(false)}
      >
        Raw
      </button>
    </fieldset>
  );
}

export function ResponseBody(props: ResponseBodyProps) {
  const mode = () => bodyMode(props.body);
  let searchInput: HTMLInputElement | undefined;
  let editor: EditorView | undefined;

  const onKeyDown = (e: KeyboardEvent) => {
    if (!((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f")) return;
    if (mode().mode === "editor" && editor) {
      e.preventDefault();
      openSearchPanel(editor);
    } else if (searchInput) {
      e.preventDefault();
      searchInput.focus();
      searchInput.select();
    }
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: captures Ctrl+F for the body below.
    <div class={styles.body} onKeyDown={onKeyDown}>
      <Show when={props.body.charsetLossy}>
        <p class={styles.warningBanner} role="alert">
          Some bytes weren't valid {props.body.charset ?? "text"} and were replaced with �.
        </p>
      </Show>
      <Switch>
        <Match when={mode().mode === "empty"}>
          <p class={styles.emptyState}>
            No body
            {props.status === 204
              ? " (204 No Content)"
              : props.status === 304
                ? " (304 Not Modified)"
                : ""}
            .
          </p>
        </Match>
        <Match when={mode().mode === "binary" && props.body.bodyId}>
          {(id) => (
            <>
              <div class={styles.bodyToolbar}>
                <span class={styles.muted}>{props.body.contentType ?? "binary"}</span>
                <span class={styles.spacer} />
                <SaveButton bodyId={id()} url={props.url} contentType={props.body.contentType} />
              </div>
              <div class={styles.bodyContent}>
                <HexView
                  bodyId={id()}
                  size={props.body.size}
                  contentType={props.body.contentType}
                />
              </div>
            </>
          )}
        </Match>
        <Match when={mode().mode === "editor"}>
          <EditorBody
            body={props.body}
            url={props.url}
            onReady={(v) => {
              editor = v;
            }}
          />
        </Match>
        <Match when={mode().mode === "paged" && props.body.bodyId}>
          {(id) => (
            <PagedBody
              bodyId={id()}
              body={props.body}
              url={props.url}
              searchRef={(el) => {
                searchInput = el;
              }}
            />
          )}
        </Match>
      </Switch>
    </div>
  );
}

/** Up to 2 MB: the whole text in CodeMirror, pretty-printed by Rust. */
function EditorBody(props: {
  body: ResponseBodyInfo;
  url?: string;
  onReady: (v: EditorView) => void;
}) {
  const [pretty, setPretty] = createSignal(true);
  const usePretty = () => props.body.prettyAvailable && pretty();
  const [text] = createResource(
    () => (props.body.bodyId !== null ? { id: props.body.bodyId, pretty: usePretty() } : null),
    ({ id, pretty }) => api.getBodyText(id, pretty),
  );
  return (
    <>
      <div class={styles.bodyToolbar}>
        <Show when={props.body.prettyAvailable}>
          <PrettyToggle pretty={pretty()} onChange={setPretty} />
        </Show>
        <span class={styles.muted}>{props.body.contentType ?? ""}</span>
        <span class={styles.spacer} />
        <CopyButton
          label="Copy body"
          text={() => api.getBodyText(props.body.bodyId as number, false)}
        />
        <SaveButton
          bodyId={props.body.bodyId as number}
          url={props.url}
          contentType={props.body.contentType}
        />
      </div>
      <div class={styles.bodyContent}>
        <Show
          when={!text.error}
          fallback={<p class={styles.emptyState}>Couldn't load the body: {String(text.error)}</p>}
        >
          <Show when={text() !== undefined} fallback={<p class={styles.emptyState}>Loading…</p>}>
            <SmallBodyEditor
              value={text() ?? ""}
              language={languageFor(props.body.kind)}
              readOnly
              ariaLabel="Response body"
              onReady={props.onReady}
            />
          </Show>
        </Show>
      </div>
    </>
  );
}

/**
 * Over 2 MB: the paged viewer. JSON/XML up to 10 MB are pretty-printed into a derived
 * body by default (Rust streams it on a background thread); bigger bodies start raw.
 */
function PagedBody(props: {
  bodyId: number;
  body: ResponseBodyInfo;
  url?: string;
  searchRef: (el: HTMLInputElement) => void;
}) {
  const initial = bodyMode(props.body);
  const prettyAvailable = initial.mode === "paged" && initial.prettyAvailable;
  const [pretty, setPretty] = createSignal(initial.mode === "paged" && initial.prettyByDefault);
  const [derived, setDerived] = createSignal<DerivedBody>();
  const [formatting, setFormatting] = createSignal(false);
  const [formatError, setFormatError] = createSignal<string>();
  const [hit, setHit] = createSignal<BodyHit>();
  let viewer: LargeBodyViewerApi | undefined;
  let disposed = false;

  const release = () => {
    const d = derived();
    if (d) void api.closeBody(d.bodyId).catch(() => {});
    setDerived(undefined);
  };
  onCleanup(() => {
    disposed = true;
    release();
  });

  createEffect(
    on(pretty, async (p) => {
      if (!p || derived() || formatting()) return;
      setFormatting(true);
      setFormatError(undefined);
      try {
        const d = await api.bodyPretty(props.bodyId);
        if (disposed) {
          void api.closeBody(d.bodyId).catch(() => {});
          return;
        }
        setDerived(d);
      } catch (e) {
        setFormatError(e instanceof Error ? e.message : String(e));
        setPretty(false);
      } finally {
        setFormatting(false);
      }
    }),
  );

  /** The body shown in the viewer: the pretty-printed copy when ready, else the raw one. */
  const shown = () => {
    const d = derived();
    return pretty() && d
      ? { id: d.bodyId, rows: d.rowCount, size: d.size, label: "pretty" }
      : { id: props.bodyId, rows: props.body.rowCount, size: props.body.size, label: "raw" };
  };

  return (
    <>
      <div class={styles.bodyToolbar}>
        <Show when={prettyAvailable}>
          <PrettyToggle pretty={pretty()} onChange={setPretty} />
        </Show>
        <Show when={shown().id} keyed>
          {(id) => (
            <BodySearch
              bodyId={id}
              byteLen={shown().size}
              ref={props.searchRef}
              onHit={(h) => {
                setHit(h);
                if (h) viewer?.scrollToRow(Math.max(0, h.row - 3));
              }}
            />
          )}
        </Show>
        <span class={styles.spacer} />
        <span class={styles.muted} title="Bodies over 2 MB can't be copied; save them instead">
          {formatBytes(shown().size)} · {shown().rows.toLocaleString()} lines · {shown().label}
        </span>
        <SaveButton bodyId={props.bodyId} url={props.url} contentType={props.body.contentType} />
      </div>
      <Show
        when={prettyAvailable && !pretty() && props.body.size > AUTO_PRETTY_LIMIT && !formatError()}
      >
        <p class={styles.note}>
          This body is {formatBytes(props.body.size)}, so it's shown raw.{" "}
          <button type="button" class="ghost" onClick={() => setPretty(true)}>
            Pretty-print anyway
          </button>
        </p>
      </Show>
      <Show when={formatError()}>
        <p class={styles.warningBanner} role="alert">
          Couldn't pretty-print: {formatError()}. Showing it raw.
        </p>
      </Show>
      <div class={styles.bodyContent}>
        <Show
          when={!(pretty() && formatting())}
          fallback={
            <p class={styles.emptyState} role="status">
              Formatting {formatBytes(props.body.size)}…
            </p>
          }
        >
          <Show when={shown()} keyed>
            {(s) => (
              <LargeBodyViewer
                rowCount={s.rows}
                byteLen={s.size}
                fetchRows={(from, count) => api.getBodyLines(s.id, from, count)}
                highlightRow={hit()?.row}
                ref={(v) => {
                  viewer = v;
                }}
              />
            )}
          </Show>
        </Show>
      </div>
    </>
  );
}
