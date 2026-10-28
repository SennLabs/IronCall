import { createSignal, Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { ScratchList } from "./ScratchList";
import styles from "./Sidebar.module.css";
import { TreeView } from "./TreeView";

/** Workspace tree with a filter box, plus scratch requests (which need no workspace). */
export function Sidebar(props: { app: AppStore }) {
  const ws = () => props.app.workspace;
  const [filter, setFilter] = createSignal("");
  let filterInput!: HTMLInputElement;

  return (
    <aside class={styles.sidebar} aria-label="Sidebar">
      <Show
        when={ws().tree()}
        fallback={
          <div class={styles.noWorkspace}>
            <p>No workspace open.</p>
            <p class={styles.muted}>A workspace is a folder of requests you can commit to git.</p>
            <div class={styles.actions}>
              <button type="button" class="primary" onClick={() => void props.app.openWorkspace()}>
                Open workspace…
              </button>
              <button type="button" onClick={() => void props.app.createWorkspace()}>
                Create workspace…
              </button>
              <button type="button" onClick={() => openDialog({ kind: "import", tab: "curl" })}>
                Import cURL…
              </button>
              <button type="button" onClick={() => void props.app.createSampleWorkspace()}>
                Try the sample workspace…
              </button>
            </div>
          </div>
        }
      >
        {(tree) => (
          <>
            <div class={styles.header}>
              <input
                ref={filterInput}
                type="search"
                class={styles.filter}
                aria-label="Filter requests"
                placeholder="Filter"
                value={filter()}
                onInput={(e) => setFilter(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setFilter("");
                    filterInput.blur();
                  }
                }}
              />
              <button
                type="button"
                class="ghost"
                title="New request"
                aria-label="New request in workspace"
                onClick={() => void ws().createItem(tree().id, "request")}
              >
                +
              </button>
              <button
                type="button"
                class="ghost"
                title="New folder"
                aria-label="New folder"
                onClick={() => void ws().createItem(tree().id, "folder")}
              >
                ▣
              </button>
              <button
                type="button"
                class="ghost"
                title="Import cURL or OpenAPI"
                aria-label="Import"
                onClick={() => openDialog({ kind: "import" })}
              >
                ⇩
              </button>
            </div>
            <TreeView app={props.app} tree={tree()} filter={filter()} />
          </>
        )}
      </Show>
      <ScratchList app={props.app} />
    </aside>
  );
}
