import { createSignal, For, onCleanup, Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import styles from "./WorkspaceSwitcher.module.css";

/** Toolbar button showing the current workspace, with recent/open/create/close. */
export function WorkspaceSwitcher(props: { app: AppStore }) {
  const ws = () => props.app.workspace;
  const [open, setOpen] = createSignal(false);
  let root!: HTMLDivElement;

  const away = (e: Event) => {
    if (!root.contains(e.target as Node)) setOpen(false);
  };
  window.addEventListener("pointerdown", away, true);
  onCleanup(() => window.removeEventListener("pointerdown", away, true));

  const run = (action: () => unknown) => {
    setOpen(false);
    void action();
  };

  return (
    <div ref={root} class={styles.switcher}>
      <button
        type="button"
        class={styles.button}
        aria-haspopup="menu"
        aria-expanded={open()}
        aria-label="Workspace"
        title={ws().info()?.root ?? "No workspace open"}
        onClick={() => {
          setOpen(!open());
          if (!open()) return;
          void ws().refreshRecent();
        }}
      >
        <span class={styles.name}>{ws().info()?.name ?? "No workspace"}</span>
        <span aria-hidden="true">▾</span>
      </button>
      <Show when={open()}>
        <div class={styles.menu} role="menu" aria-label="Workspaces">
          <Show when={ws().recent().length > 0}>
            <div class={styles.heading}>Recent</div>
            <For each={ws().recent()}>
              {(r) => (
                <button
                  type="button"
                  role="menuitem"
                  class={styles.item}
                  classList={{ [styles.current as string]: r.path === ws().info()?.root }}
                  disabled={!r.available}
                  title={r.available ? r.path : `${r.path} (not found)`}
                  onClick={() => run(() => props.app.openWorkspace(r.path))}
                >
                  <span>{r.name}</span>
                  <span class={styles.path}>{r.path}</span>
                </button>
              )}
            </For>
            <hr class={styles.sep} />
          </Show>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => props.app.openWorkspace())}
          >
            Open workspace…
          </button>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => props.app.createWorkspace())}
          >
            Create workspace…
          </button>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => openDialog({ kind: "import" }))}
          >
            Import cURL / OpenAPI…
          </button>
          <Show when={ws().info()}>
            <hr class={styles.sep} />
            <button
              type="button"
              role="menuitem"
              class={styles.item}
              onClick={() => run(() => openDialog({ kind: "environments" }))}
            >
              Environments…
            </button>
            <button
              type="button"
              role="menuitem"
              class={styles.item}
              onClick={() =>
                run(() => openDialog({ kind: "variables", target: { kind: "collection" } }))
              }
            >
              Collection variables…
            </button>
            <button
              type="button"
              role="menuitem"
              class={styles.item}
              onClick={() => run(() => openDialog({ kind: "secrets" }))}
            >
              Secret storage…
            </button>
            <button
              type="button"
              role="menuitem"
              class={styles.item}
              onClick={() =>
                run(() => openDialog({ kind: "containerSettings", target: { kind: "collection" } }))
              }
            >
              Collection auth & headers…
            </button>
          </Show>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => openDialog({ kind: "variables", target: { kind: "global" } }))}
          >
            Global variables…
          </button>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => openDialog({ kind: "cookies" }))}
          >
            Cookies…
          </button>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => openDialog({ kind: "network" }))}
          >
            Network settings…
          </button>
          <button
            type="button"
            role="menuitem"
            class={styles.item}
            onClick={() => run(() => openDialog({ kind: "history" }))}
          >
            History settings…
          </button>
          <Show when={ws().info()}>
            <hr class={styles.sep} />
            <button
              type="button"
              role="menuitem"
              class={styles.item}
              onClick={() => run(() => props.app.closeWorkspace())}
            >
              Close workspace
            </button>
          </Show>
        </div>
      </Show>
    </div>
  );
}
