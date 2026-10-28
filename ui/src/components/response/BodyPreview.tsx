import { createSignal, lazy, Match, Show, Suspense, Switch } from "solid-js";
import type { ResponseBodyInfo } from "../../ipc/bindings";
import { bodyUrl, previewUrl } from "../../ipc/scheme";
import { type PreviewKind, previewSandbox } from "../../model/bodyView";
import styles from "./preview.module.css";

const PdfView = lazy(() => import("./PdfView").then((m) => ({ default: m.PdfView })));

/** Rendered preview of an image, PDF or HTML body (PLAN.md Phase 5). */
export function BodyPreview(props: { body: ResponseBodyInfo; kind: PreviewKind }) {
  const id = () => props.body.bodyId as number;
  return (
    <Switch>
      <Match when={props.kind === "image"}>
        <ImagePreview src={bodyUrl(id())} />
      </Match>
      <Match when={props.kind === "pdf"}>
        <Suspense fallback={<p class={styles.muted}>Loading PDF viewer…</p>}>
          <PdfView bodyId={id()} size={props.body.size} />
        </Suspense>
      </Match>
      <Match when={props.kind === "html"}>
        <HtmlPreview bodyId={id()} />
      </Match>
      <Match when={props.kind === "media"}>
        <p class={styles.message}>
          Audio and video previews aren't supported yet. Use “Save response…” and open the file
          instead.
        </p>
      </Match>
    </Switch>
  );
}

function ImagePreview(props: { src: string }) {
  const [fitted, setFit] = createSignal(true);
  const [zoom, setZoom] = createSignal(1);
  const [size, setSize] = createSignal<{ w: number; h: number }>();
  const [failed, setFailed] = createSignal(false);
  return (
    <div class={styles.preview}>
      <div class={styles.toolbar}>
        <fieldset class={styles.segmented} aria-label="Image size">
          <button
            type="button"
            aria-pressed={fitted()}
            class={fitted() ? "primary" : undefined}
            onClick={() => setFit(true)}
          >
            Fit
          </button>
          <button
            type="button"
            aria-pressed={!fitted()}
            class={!fitted() ? "primary" : undefined}
            onClick={() => {
              setFit(false);
              setZoom(1);
            }}
          >
            Actual size
          </button>
        </fieldset>
        <Show when={!fitted()}>
          <button
            type="button"
            onClick={() => setZoom((z) => Math.max(0.1, z / 1.25))}
            aria-label="Zoom out"
          >
            −
          </button>
          <span class={styles.muted}>{Math.round(zoom() * 100)}%</span>
          <button
            type="button"
            onClick={() => setZoom((z) => Math.min(8, z * 1.25))}
            aria-label="Zoom in"
          >
            +
          </button>
        </Show>
        <span class={styles.spacer} />
        <Show when={size()}>
          {(s) => (
            <span class={styles.muted} data-testid="image-size">
              {s().w} × {s().h} px
            </span>
          )}
        </Show>
      </div>
      <Show
        when={!failed()}
        fallback={<p class={styles.message}>This image couldn't be displayed.</p>}
      >
        <div class={styles.imageHost} classList={{ [styles.checker as string]: true }}>
          <img
            src={props.src}
            alt="Response body"
            class={fitted() ? styles.fit : undefined}
            style={
              fitted() || !size()
                ? undefined
                : { width: `${(size()?.w ?? 0) * zoom()}px`, height: "auto" }
            }
            onLoad={(e) =>
              setSize({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })
            }
            onError={() => setFailed(true)}
          />
        </div>
      </Show>
    </div>
  );
}

/**
 * HTML in a sandboxed iframe on the isolated `ironcall-preview` origin (served with a
 * `default-src 'none'` CSP, so nothing loads from the network). Scripts are off unless
 * the user opts in; `allow-same-origin` is never set.
 */
export function HtmlPreview(props: { bodyId: number }) {
  const [allowScripts, setAllowScripts] = createSignal(false);
  return (
    <div class={styles.preview}>
      <div class={styles.toolbar}>
        <label class={styles.muted}>
          <input
            type="checkbox"
            checked={allowScripts()}
            onChange={(e) => setAllowScripts(e.currentTarget.checked)}
          />{" "}
          Allow scripts
        </label>
        <Show when={allowScripts()}>
          <span class={styles.warning} role="note">
            Scripts run in an isolated frame with no network access.
          </span>
        </Show>
        <span class={styles.spacer} />
        <span class={styles.muted}>Remote images, styles and scripts are blocked.</span>
      </div>
      {/* Re-create the frame when the sandbox changes; browsers ignore later changes. */}
      <Show when={allowScripts()} keyed fallback={<Frame bodyId={props.bodyId} scripts={false} />}>
        {(_) => <Frame bodyId={props.bodyId} scripts={true} />}
      </Show>
    </div>
  );
}

function Frame(props: { bodyId: number; scripts: boolean }) {
  return (
    <iframe
      class={styles.frame}
      title="HTML preview"
      sandbox={previewSandbox(props.scripts)}
      referrerPolicy="no-referrer"
      src={previewUrl(props.bodyId)}
    />
  );
}
