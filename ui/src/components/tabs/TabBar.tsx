import { createSignal, For, Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import type { Tab } from "../../stores/tabs";
import { ContextMenu } from "../common/ContextMenu";
import { MethodBadge } from "../common/MethodBadge";
import styles from "./TabBar.module.css";

/** Open tabs. Middle-click closes; preview tabs are italic until pinned. */
export function TabBar(props: { app: AppStore }) {
  const tabs = () => props.app.tabs;
  const [menu, setMenu] = createSignal<{ x: number; y: number; tab: Tab }>();

  const statusLabel = (tab: Tab) => {
    const s = tab.status().kind;
    if (s === "conflict") return "changed on disk";
    if (s === "deleted") return "deleted on disk";
    return tab.dirty() ? "unsaved changes" : "";
  };

  return (
    <div class={styles.bar}>
      <div class={styles.list} role="tablist" aria-label="Open requests">
        <For each={tabs().tabs()}>
          {(tab) => (
            // The close button sits beside the tab, not inside it: ARIA makes a tab's
            // children presentational, which would hide a nested button. This wrapper only
            // adds mouse conveniences (middle-click, context menu); keys go to the tab.
            // biome-ignore lint/a11y/noStaticElementInteractions: see above.
            <div
              role="presentation"
              class={styles.tab}
              classList={{
                [styles.active as string]: tabs().active()?.key === tab.key,
                [styles.preview as string]: !tab.pinned(),
                [styles.deleted as string]: tab.status().kind === "deleted",
              }}
              data-key={tab.key}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  void tabs().close(tab.key);
                }
              }}
              onMouseDown={(e) => e.button === 1 && e.preventDefault()}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, tab });
              }}
            >
              <div
                role="tab"
                class={styles.label}
                tabIndex={tabs().active()?.key === tab.key ? 0 : -1}
                aria-selected={tabs().active()?.key === tab.key}
                aria-label={[tab.title(), statusLabel(tab)].filter(Boolean).join(", ")}
                aria-keyshortcuts="Delete Control+W"
                title={tab.isScratch() ? "Scratch request (not in a workspace)" : tab.title()}
                onClick={() => tabs().activate(tab.key)}
                onDblClick={() => tabs().pin(tab.key)}
                onKeyDown={(e) => {
                  if (e.key === "Delete") void tabs().close(tab.key);
                }}
              >
                <MethodBadge method={tab.request.draft().method} />
                <span class={styles.title}>{tab.title()}</span>
                <Show when={tab.isScratch()}>
                  <span class={styles.scratchTag}>scratch</span>
                </Show>
                <Show when={tab.status().kind === "conflict"}>
                  <span class={styles.conflict} aria-hidden="true" title="Changed on disk">
                    ⚠
                  </span>
                </Show>
              </div>
              <button
                type="button"
                class={styles.close}
                classList={{ [styles.dirty as string]: tab.dirty() }}
                aria-label={`Close ${tab.title()}`}
                title={tab.dirty() ? "Unsaved changes. Close (Ctrl+W)" : "Close (Ctrl+W)"}
                // A mouse convenience: a tablist may only contain tabs, and keyboard and
                // screen reader users close the focused tab with Delete or Ctrl+W.
                aria-hidden="true"
                tabIndex={-1}
                onClick={(e) => {
                  e.stopPropagation();
                  void tabs().close(tab.key);
                }}
              >
                <span class={styles.dot} aria-hidden="true">
                  ●
                </span>
                <span class={styles.x} aria-hidden="true">
                  ✕
                </span>
              </button>
            </div>
          )}
        </For>
      </div>
      <button
        type="button"
        class={`ghost ${styles.add}`}
        aria-label="New scratch request"
        title="New scratch request (Ctrl+N)"
        onClick={() => void tabs().newScratch()}
      >
        +
      </button>
      <Show when={menu()}>
        {(m) => (
          <ContextMenu
            x={m().x}
            y={m().y}
            label={`${m().tab.title()} tab actions`}
            onClose={() => setMenu(undefined)}
            items={[
              { label: "Close", shortcut: "Ctrl+W", action: () => void tabs().close(m().tab.key) },
              { label: "Close others", action: () => void tabs().closeOthers(m().tab.key) },
              { label: "Close all", action: () => void tabs().closeAll() },
              null,
              {
                label: "Copy as cURL…",
                action: () => openDialog({ kind: "copyCurl", source: { tabKey: m().tab.key } }),
              },
              null,
              m().tab.isScratch()
                ? {
                    label: "Save to workspace…",
                    action: () => openDialog({ kind: "saveTo", tabKey: m().tab.key }),
                  }
                : {
                    label: "Save",
                    shortcut: "Ctrl+S",
                    disabled: !m().tab.dirty() && m().tab.status().kind === "ok",
                    action: () => void tabs().save(m().tab.key),
                  },
              ...(m().tab.isScratch()
                ? []
                : [
                    {
                      label: "Reveal in file manager",
                      action: () => void props.app.workspace.reveal(m().tab.key),
                    },
                  ]),
            ]}
          />
        )}
      </Show>
    </div>
  );
}
