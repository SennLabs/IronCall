import { For, Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import styles from "./Welcome.module.css";

/** Shown when no tab is open: start a request or open a workspace. */
export function Welcome(props: { app: AppStore }) {
  const ws = () => props.app.workspace;
  return (
    <div class={styles.welcome}>
      <h1>IronCall</h1>
      <div class={styles.actions}>
        <button type="button" class="primary" onClick={() => void props.app.tabs.newScratch()}>
          New request <kbd>Ctrl+N</kbd>
        </button>
        <button type="button" onClick={() => void props.app.openWorkspace()}>
          Open workspace…
        </button>
        <button type="button" onClick={() => void props.app.createWorkspace()}>
          Create workspace…
        </button>
        <button type="button" onClick={() => openDialog({ kind: "import" })}>
          Import…
        </button>
        <Show when={!ws().info()}>
          <button type="button" onClick={() => void props.app.createSampleWorkspace()}>
            Try the sample workspace…
          </button>
        </Show>
      </div>
      <section class={styles.tips} aria-label="Getting started">
        <h2>Getting started</h2>
        <ul>
          <li>
            <strong>Paste a cURL command</strong> into the URL bar to turn it into a request.
          </li>
          <Show when={!ws().info()}>
            <li>
              The <strong>sample workspace</strong> calls IronCall's built-in echo server, so it
              works offline: assertions, a chained token and the runner.
            </li>
          </Show>
          <li>
            <kbd>Ctrl+Enter</kbd> send · <kbd>Ctrl+S</kbd> save · <kbd>Ctrl+P</kbd> go to request ·{" "}
            <kbd>Ctrl+L</kbd> URL bar · <kbd>Ctrl+,</kbd> settings
          </li>
        </ul>
      </section>
      <Show when={ws().recent().length > 0}>
        <section class={styles.recent} aria-label="Recent workspaces">
          <h2>Recent workspaces</h2>
          <ul>
            <For each={ws().recent()}>
              {(r) => (
                <li classList={{ [styles.unavailable as string]: !r.available }}>
                  <button
                    type="button"
                    class="ghost"
                    disabled={!r.available}
                    onClick={() => void props.app.openWorkspace(r.path)}
                    title={r.available ? r.path : "Folder not found or has no ironcall.yaml"}
                  >
                    <strong>{r.name}</strong>
                    <span class={styles.path}>{r.path}</span>
                  </button>
                  <button
                    type="button"
                    class="ghost"
                    aria-label={`Forget ${r.name}`}
                    title="Remove from this list"
                    onClick={() => void ws().forget(r.path)}
                  >
                    ✕
                  </button>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
    </div>
  );
}
