import { ErrorBoundary, onCleanup, onMount, Show } from "solid-js";
import styles from "./App.module.css";
import { Notices } from "./components/common/Notices";
import { SplitPane } from "./components/common/SplitPane";
import { DialogHost } from "./components/dialogs/DialogHost";
import { HistoryPanel } from "./components/history/HistoryPanel";
import { RecentSends } from "./components/response/RecentSends";
import { Sidebar } from "./components/sidebar/Sidebar";
import { ThemeToggle } from "./components/ThemeToggle";
import { TabBar } from "./components/tabs/TabBar";
import { TabView } from "./components/tabs/TabView";
import { EnvironmentSwitcher } from "./components/vars/EnvironmentSwitcher";
import { SecretsBanner } from "./components/vars/SecretsBanner";
import { Welcome } from "./components/workspace/Welcome";
import { WorkspaceSwitcher } from "./components/workspace/WorkspaceSwitcher";
import { api } from "./ipc/api";
import { type AppStore, createAppStore } from "./stores/app";
import { copyDiagnostics, openLogs } from "./stores/diagnostics";
import { anyDialog, openDialog } from "./stores/dialogs";
import { loadSettings, resetZoom, zoomBy } from "./stores/settings";

/** True if the event comes from a place with its own undo (CodeMirror, inline rename). */
const hasOwnUndo = (target: EventTarget | null) =>
  target instanceof HTMLElement && !!target.closest(".cm-editor, [data-native-undo]");

/** Global keyboard shortcuts (PLAN.md Phases 1, 2 and 8). */
function shortcuts(app: AppStore) {
  return (e: KeyboardEvent) => {
    if (anyDialog()) return;
    const mod = e.ctrlKey || e.metaKey;
    const tab = app.tabs.active();
    const key = e.key.toLowerCase();
    let handled = true;
    if (mod && e.key === "Enter") {
      if (tab) void app.send(tab);
    } else if (e.key === "Escape" && e.defaultPrevented) {
      // Already handled (a menu, CodeMirror's search panel): not a global Escape.
      return;
    } else if (e.key === "Escape" && tab?.sender.isSending()) {
      void tab.sender.cancel();
    } else if (mod && key === "s") {
      void app.tabs.save();
    } else if (mod && key === "n") {
      void app.tabs.newScratch();
    } else if (mod && key === "w") {
      if (tab) void app.tabs.close(tab.key);
    } else if (mod && (key === "=" || key === "+")) {
      zoomBy(1);
    } else if (mod && key === "-") {
      zoomBy(-1);
    } else if (mod && key === "0") {
      resetZoom();
    } else if (mod && key === ",") {
      openDialog({ kind: "settings" });
    } else if (mod && key === "p") {
      openDialog({ kind: "quickOpen" });
    } else if (mod && key === "l") {
      const url = document.querySelector<HTMLInputElement>('input[aria-label="URL"]');
      url?.focus();
      url?.select();
    } else if (mod && key === "h") {
      app.history.toggle();
    } else if (e.key === "Escape" && app.history.open()) {
      app.history.setOpen(false);
    } else if (mod && e.key === "Tab") {
      app.tabs.next(e.shiftKey ? -1 : 1);
    } else if (mod && (key === "z" || key === "y") && !e.altKey) {
      if (hasOwnUndo(e.target)) return;
      if (key === "y" || e.shiftKey) app.tabs.redo();
      else app.tabs.undo();
    } else {
      handled = false;
    }
    if (handled) e.preventDefault();
  };
}

/** Shown if rendering fails; the error is logged so diagnostic info includes it. */
function Crashed(props: { error: unknown; reset: () => void }) {
  const message = () => (props.error instanceof Error ? props.error.message : String(props.error));
  onMount(() => {
    const e = props.error;
    void api.logFrontendError(message(), e instanceof Error ? (e.stack ?? null) : null);
  });
  return (
    <div class={styles.crashed} role="alert">
      <h2>Something went wrong</h2>
      <p>IronCall hit an unexpected error. Your saved files are not affected.</p>
      <pre>{message()}</pre>
      <div class={styles.crashActions}>
        <button type="button" class="primary" onClick={props.reset}>
          Try again
        </button>
        <button type="button" onClick={() => void copyDiagnostics()}>
          Copy diagnostic info
        </button>
        <button type="button" onClick={openLogs}>
          Open logs folder
        </button>
      </div>
    </div>
  );
}

export default function App() {
  const app = createAppStore();
  onMount(() => {
    void loadSettings();
    void app.start();
    const onKeyDown = shortcuts(app);
    window.addEventListener("keydown", onKeyDown);
    onCleanup(() => window.removeEventListener("keydown", onKeyDown));
    // Closing the window asks about unsaved work and saves the session first.
    let unlistenClose: (() => void) | undefined;
    void api
      .onCloseRequested(() => app.beforeQuit())
      .then((u) => {
        unlistenClose = u;
      })
      .catch(() => {
        // Not available (e.g. a plain browser): nothing to guard.
      });
    onCleanup(() => unlistenClose?.());
  });

  return (
    <div class={styles.shell}>
      <header class={styles.toolbar}>
        <strong>IronCall</strong>
        <WorkspaceSwitcher app={app} />
        <span class={styles.spacer} />
        <EnvironmentSwitcher app={app} />
        <Show when={app.workspace.info()}>
          {(info) => (
            <button
              type="button"
              class="ghost"
              title="Run every request in the workspace"
              onClick={() => void app.openRunner(null, info().name)}
            >
              Run
            </button>
          )}
        </Show>
        <RecentSends recent={app.tabs.active()?.sender.recent() ?? []} />
        <button
          type="button"
          class="ghost"
          aria-pressed={app.history.open()}
          title="History (Ctrl+H)"
          onClick={() => app.history.toggle()}
        >
          History
        </button>
        <ThemeToggle />
        <button
          type="button"
          class="ghost"
          title="Settings (Ctrl+,)"
          onClick={() => openDialog({ kind: "settings" })}
        >
          Settings
        </button>
      </header>
      <ErrorBoundary fallback={(error, reset) => <Crashed error={error} reset={reset} />}>
        <Show when={app.ready()} fallback={<p class={styles.loading}>Starting…</p>}>
          <SecretsBanner app={app} />
          <main class={styles.main}>
            <SplitPane
              direction="row"
              initial={0.22}
              first={<Sidebar app={app} />}
              second={
                <div class={styles.editor}>
                  <Show when={app.tabs.tabs().length > 0} fallback={<Welcome app={app} />}>
                    <TabBar app={app} />
                    <div class={styles.tabBody}>
                      <Show when={app.tabs.active()} keyed>
                        {(tab) => <TabView app={app} tab={tab} />}
                      </Show>
                    </div>
                  </Show>
                </div>
              }
            />
            <Show when={app.history.open()}>
              <HistoryPanel app={app} />
            </Show>
          </main>
        </Show>
      </ErrorBoundary>
      <DialogHost app={app} />
      <Notices />
    </div>
  );
}
