import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import {
  api,
  type CurlParseResultDraft,
  type ImportWarning,
  type ItemId,
  type OpenApiPreview,
  type PlanNode,
} from "../../ipc/api";
import { countPlan } from "../../model/curl";
import { allContainers } from "../../model/tree";
import type { AppStore } from "../../stores/app";
import { closeDialog } from "../../stores/dialogs";
import { attempt, notify } from "../../stores/notify";
import { MethodBadge } from "../common/MethodBadge";
import { Modal } from "../common/Modal";
import { Tabs } from "../common/Tabs";
import styles from "./import.module.css";

/** Debounce for the cURL tab's live preview. */
export const CURL_PREVIEW_MS = 250;

const VERSION_LABEL: Record<OpenApiPreview["version"], string> = {
  swagger2: "Swagger 2.0",
  openApi30: "OpenAPI 3.0",
  openApi31: "OpenAPI 3.1",
};

/** Import a cURL command or an OpenAPI/Swagger spec (PLAN.md Phase 6). */
export function ImportDialog(props: { app: AppStore; initial?: "curl" | "openapi" }) {
  // Captured once: props come from a <Match> that goes stale when the dialog closes.
  const { app } = props;
  // State lives here so switching tabs keeps it.
  const curl = createCurlState();
  const openapi = createOpenApiState(app);

  return (
    <Modal title="Import" wide onClose={closeDialog}>
      <Tabs
        label="Import from"
        initial={props.initial ?? "curl"}
        tabs={[
          { key: "curl", label: "cURL" },
          { key: "openapi", label: "OpenAPI / Swagger" },
        ]}
      >
        {(active) =>
          active === "curl" ? (
            <CurlTab app={app} state={curl} />
          ) : (
            <OpenApiTab app={app} state={openapi} />
          )
        }
      </Tabs>
    </Modal>
  );
}

// ---------------------------------------------------------------------------------
// cURL

function createCurlState() {
  const [text, setText] = createSignal("");
  const [parsed, setParsed] = createSignal<CurlParseResultDraft>();
  const [error, setError] = createSignal<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  createEffect(
    on(text, (t) => {
      clearTimeout(timer);
      const g = ++generation;
      if (!t.trim()) {
        setParsed(undefined);
        setError(undefined);
        return;
      }
      timer = setTimeout(() => {
        api.curlParse(t).then(
          (r) => {
            if (g !== generation) return;
            setParsed(r);
            setError(undefined);
          },
          (e: unknown) => {
            if (g !== generation) return;
            setParsed(undefined);
            setError(e instanceof Error ? e.message : String(e));
          },
        );
      }, CURL_PREVIEW_MS);
    }),
  );
  onCleanup(() => clearTimeout(timer));
  return { text, setText, parsed, error };
}

function bodySummary(r: CurlParseResultDraft["request"]): string {
  const body = r.body;
  if (!body || body.type === "none") return "No body";
  const content = typeof body.content === "string" ? body.content : "";
  return content ? `${body.type} body, ${content.length} characters` : `${body.type} body`;
}

function CurlTab(props: { app: AppStore; state: ReturnType<typeof createCurlState> }) {
  const s = props.state;
  const open = async () => {
    const tab = await props.app.importCurl(s.text());
    if (tab) closeDialog();
  };
  return (
    <div class={styles.pane}>
      <label class={styles.field}>
        Paste a cURL command (bash, or Chrome's "Copy as cURL (cmd)")
        <textarea
          class={styles.code}
          rows={7}
          spellcheck={false}
          aria-label="cURL command"
          placeholder={"curl 'https://api.example.com/users' -H 'Accept: application/json'"}
          value={s.text()}
          onInput={(e) => s.setText(e.currentTarget.value)}
          autofocus
        />
      </label>
      <Show when={s.error()}>
        <p class={styles.error} role="alert">
          {s.error()}
        </p>
      </Show>
      <Show when={s.parsed()}>
        {(p) => (
          <section class={styles.summary} aria-label="cURL preview">
            <p class={styles.requestLine}>
              <MethodBadge method={p().request.method} />{" "}
              <code>{p().request.url || "(no URL)"}</code>
            </p>
            <p class={styles.muted}>
              {(p().request.headers ?? []).length} header
              {(p().request.headers ?? []).length === 1 ? "" : "s"} · {bodySummary(p().request)}
              {(p().request.params ?? []).length > 0
                ? ` · ${(p().request.params ?? []).length} query params`
                : ""}
            </p>
            <Warnings warnings={p().warnings} />
          </section>
        )}
      </Show>
      <div class={styles.footer}>
        <button type="button" onClick={closeDialog}>
          Cancel
        </button>
        <button type="button" class="primary" disabled={!s.parsed()} onClick={() => void open()}>
          Open in new tab
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------
// OpenAPI

function createOpenApiState(app: AppStore) {
  const [preview, setPreview] = createSignal<OpenApiPreview>();
  const [error, setError] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [pasted, setPasted] = createSignal("");
  const [parent, setParent] = createSignal<ItemId | undefined>(app.workspace.tree()?.id);
  const [wrap, setWrap] = createSignal(true);
  const [createEnv, setCreateEnv] = createSignal(true);
  const [activate, setActivate] = createSignal(true);

  const run = async (load: () => Promise<OpenApiPreview | null>) => {
    setBusy(true);
    setError(undefined);
    try {
      const p = await load();
      if (p) setPreview(p);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return {
    preview,
    error,
    busy,
    pasted,
    setPasted,
    parent,
    setParent,
    wrap,
    setWrap,
    createEnv,
    setCreateEnv,
    activate,
    setActivate,
    pick: () => run(() => api.openapiPickAndPreview()),
    previewText: () => run(() => api.openapiPreviewText(pasted())),
    setBusy,
  };
}

function OpenApiTab(props: { app: AppStore; state: ReturnType<typeof createOpenApiState> }) {
  const s = props.state;
  const app = props.app;
  const tree = () => app.workspace.tree();

  const doImport = async () => {
    const p = s.preview();
    const parent = s.parent() ?? tree()?.id;
    if (!p || !parent) return;
    s.setBusy(true);
    const result = await attempt(
      () =>
        api.openapiImport({
          planId: p.planId,
          parent,
          wrapInFolder: s.wrap(),
          createEnvironment: s.createEnv(),
          activateEnvironment: s.createEnv() && s.activate(),
        }),
      "Couldn't import the spec",
    );
    s.setBusy(false);
    if (!result) return;
    closeDialog();
    await app.workspace.refresh();
    // The import may have created (and activated) an environment.
    if (result.environment) await app.vars.refresh();
    if (result.folder) {
      app.workspace.expand(result.folder);
      app.workspace.setSelected(result.folder);
    } else if (parent !== tree()?.id) {
      app.workspace.expand(parent);
    }
    if (result.environment) await app.vars.refresh();
    const warn = result.warnings.length + p.warnings.length;
    notify(
      `Imported ${result.requestsCreated} request${result.requestsCreated === 1 ? "" : "s"}` +
        ` in ${result.foldersCreated} folder${result.foldersCreated === 1 ? "" : "s"}` +
        (result.environment ? ", plus an environment" : "") +
        (warn ? ` (${warn} warning${warn === 1 ? "" : "s"})` : ""),
    );
    for (const w of result.warnings) notify(warningText(w), "error");
  };

  return (
    <Show
      when={tree()}
      fallback={
        <div class={styles.pane}>
          <p class={styles.hint}>
            Open or create a workspace first. OpenAPI imports create files there.
          </p>
        </div>
      }
    >
      {(t) => (
        <div class={styles.pane}>
          <div class={styles.row}>
            <button type="button" disabled={s.busy()} onClick={() => void s.pick()}>
              Choose file…
            </button>
            <span class={styles.muted}>YAML or JSON (OpenAPI 3.0, 3.1 or Swagger 2.0)</span>
          </div>
          <details class={styles.paste}>
            <summary>Or paste the spec</summary>
            <textarea
              class={styles.code}
              rows={6}
              spellcheck={false}
              aria-label="Spec text"
              value={s.pasted()}
              onInput={(e) => s.setPasted(e.currentTarget.value)}
            />
            <button
              type="button"
              disabled={s.busy() || !s.pasted().trim()}
              onClick={() => void s.previewText()}
            >
              Preview
            </button>
          </details>
          <Show when={s.error()}>
            <p class={styles.error} role="alert">
              {s.error()}
            </p>
          </Show>
          <Show when={s.preview()}>
            {(p) => (
              <>
                <section class={styles.summary} aria-label="Import preview">
                  <h3 class={styles.title}>
                    {p().title} <span class={styles.badge}>{VERSION_LABEL[p().version]}</span>
                  </h3>
                  <Show when={p().description}>
                    <p class={styles.muted}>{p().description}</p>
                  </Show>
                  <p>
                    {p().requestCount} request{p().requestCount === 1 ? "" : "s"}
                    {(() => {
                      const n = countPlan(p().tree).folders;
                      return n ? ` in ${n} folder${n === 1 ? "" : "s"}` : "";
                    })()}
                  </p>
                  <Show when={p().servers.length > 0}>
                    <p class={styles.muted}>
                      Servers: <code>{p().servers.join(", ")}</code> (the first becomes{" "}
                      <code>{"{{baseUrl}}"}</code>)
                    </p>
                  </Show>
                  <Show when={p().auth.length > 0}>
                    <ul class={styles.list} aria-label="Auth mappings">
                      <For each={p().auth}>{(a) => <li>{a}</li>}</For>
                    </ul>
                  </Show>
                  <ul class={styles.tree} aria-label="Requests to import">
                    <For each={p().tree}>{(n) => <PlanItem node={n} />}</For>
                  </ul>
                  <Warnings warnings={p().warnings} />
                </section>
                <fieldset class={styles.options}>
                  <legend>Import into</legend>
                  <label>
                    Folder
                    <select
                      aria-label="Target folder"
                      value={s.parent() ?? t().id}
                      onChange={(e) => s.setParent(e.currentTarget.value)}
                    >
                      <For each={allContainers(t())}>
                        {(c) => (
                          <option value={c.node.id}>
                            {"  ".repeat(c.depth)}
                            {c.node.kind === "collection"
                              ? `${c.node.name} (top level)`
                              : c.node.name}
                          </option>
                        )}
                      </For>
                    </select>
                  </label>
                  <label class={styles.check}>
                    <input
                      type="checkbox"
                      checked={s.wrap()}
                      onChange={(e) => s.setWrap(e.currentTarget.checked)}
                    />
                    Put in a new folder ("{p().title}")
                  </label>
                  <label class={styles.check}>
                    <input
                      type="checkbox"
                      checked={s.createEnv()}
                      onChange={(e) => s.setCreateEnv(e.currentTarget.checked)}
                    />
                    Create environment with <code>baseUrl</code>
                  </label>
                  <label class={styles.check}>
                    <input
                      type="checkbox"
                      checked={s.createEnv() && s.activate()}
                      disabled={!s.createEnv()}
                      onChange={(e) => s.setActivate(e.currentTarget.checked)}
                    />
                    Make it active
                  </label>
                </fieldset>
              </>
            )}
          </Show>
          <div class={styles.footer}>
            <button type="button" onClick={closeDialog}>
              Cancel
            </button>
            <button
              type="button"
              class="primary"
              disabled={!s.preview() || s.busy()}
              onClick={() => void doImport()}
            >
              Import
            </button>
          </div>
        </div>
      )}
    </Show>
  );
}

function PlanItem(props: { node: PlanNode }) {
  const folder = () => props.node.method === null;
  const [open, setOpen] = createSignal(true);
  return (
    <li>
      <Show
        when={folder()}
        fallback={
          <span class={styles.leaf}>
            <MethodBadge method={props.node.method} /> {props.node.name}{" "}
            <code class={styles.muted}>{props.node.url}</code>
          </span>
        }
      >
        <button
          type="button"
          class={`ghost ${styles.folder}`}
          aria-expanded={open()}
          onClick={() => setOpen(!open())}
        >
          {open() ? "▾" : "▸"} {props.node.name}{" "}
          <span class={styles.muted}>({props.node.children.length})</span>
        </button>
        <Show when={open()}>
          <ul>
            <For each={props.node.children}>{(c) => <PlanItem node={c} />}</For>
          </ul>
        </Show>
      </Show>
    </li>
  );
}

function warningText(w: ImportWarning): string {
  return w.location ? `${w.location}: ${w.message}` : w.message;
}

function Warnings(props: { warnings: readonly ImportWarning[] }) {
  return (
    <Show when={props.warnings.length > 0}>
      <details class={styles.warnings}>
        <summary>
          {props.warnings.length} warning{props.warnings.length === 1 ? "" : "s"}
        </summary>
        <ul aria-label="Import warnings">
          <For each={props.warnings}>
            {(w) => (
              <li>
                <Show when={w.location}>
                  <code>{w.location}</code>{" "}
                </Show>
                {w.message}
              </li>
            )}
          </For>
        </ul>
      </details>
    </Show>
  );
}
