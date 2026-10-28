import { Show } from "solid-js";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { dismissImportNotice, importNotice } from "../../stores/importNotice";
import type { Tab } from "../../stores/tabs";
import { SplitPane } from "../common/SplitPane";
import { RequestBar } from "../request/RequestBar";
import { RequestEditor } from "../request/RequestEditor";
import { ResponsePanel } from "../response/ResponsePanel";
import { VarsProvider } from "../vars/VarsContext";
import styles from "./TabView.module.css";

/** One tab: status bars, the request bar, and the request/response split. */
export function TabView(props: { app: AppStore; tab: Tab }) {
  const tab = () => props.tab;
  const tabs = () => props.app.tabs;
  const send = () => void props.app.send(tab());

  return (
    <VarsProvider value={() => tab().vars.context()}>
      <div class={styles.view}>
        <Show when={tab().status().kind === "conflict"}>
          <div class={`${styles.banner} ${styles.warning}`} role="alert" aria-label="Conflict">
            <span>
              <strong>This file changed on disk</strong> while you had unsaved changes.
            </span>
            <span class={styles.spacer} />
            <button type="button" onClick={() => void tabs().keepMine(tab().key)}>
              Keep mine
            </button>
            <button type="button" onClick={() => void tabs().takeTheirs(tab().key)}>
              Take theirs
            </button>
            <button
              type="button"
              onClick={() => openDialog({ kind: "compare", tabKey: tab().key })}
            >
              Compare
            </button>
          </div>
        </Show>
        <Show when={tab().status().kind === "deleted"}>
          <div
            class={`${styles.banner} ${styles.warning}`}
            role="alert"
            aria-label="Deleted on disk"
          >
            <span>
              <strong>This file was deleted on disk.</strong> Save to recreate it, or close the tab.
            </span>
            <span class={styles.spacer} />
            <button type="button" onClick={() => void tabs().save(tab().key)}>
              Save again
            </button>
            <button type="button" onClick={() => void tabs().close(tab().key)}>
              Close
            </button>
          </div>
        </Show>
        <Show when={tab().readOnly()}>
          <div class={styles.banner} role="note">
            This file uses a newer IronCall format, so it's read-only here. You can still send it.
          </div>
        </Show>
        <div class={styles.titleRow}>
          <span class={styles.name}>{tab().title()}</span>
          <Show
            when={tab().isScratch()}
            fallback={
              <button
                type="button"
                class="ghost"
                disabled={!tab().dirty() && tab().status().kind !== "deleted"}
                title="Save (Ctrl+S)"
                onClick={() => void tabs().save(tab().key)}
              >
                {tab().dirty() ? "Save" : "Saved"}
              </button>
            }
          >
            <span class={styles.muted}>Scratch · autosaved</span>
            <button
              type="button"
              class="ghost"
              onClick={() => openDialog({ kind: "saveTo", tabKey: tab().key })}
            >
              Save to workspace…
            </button>
          </Show>
        </div>
        <Show when={importNotice()?.tabKey === tab().key ? importNotice() : undefined} keyed>
          {(n) => (
            <div class={styles.banner} role="status" aria-label="cURL import">
              <span>{n.text}</span>
              <span class={styles.spacer} />
              <button
                type="button"
                class="ghost"
                aria-label="Dismiss import notice"
                onClick={dismissImportNotice}
              >
                ✕
              </button>
            </div>
          )}
        </Show>
        <RequestBar
          request={tab().request}
          sender={tab().sender}
          onSend={send}
          onPreview={() => openDialog({ kind: "preview", tabKey: tab().key })}
          onPasteCurl={(command) => void props.app.importCurl(command, tab())}
        />
        <div class={styles.main}>
          <SplitPane
            direction="row"
            initial={0.5}
            first={
              <RequestEditor
                request={tab().request}
                preview={tab().preview.preview}
                readOnly={tab().readOnly()}
              />
            }
            second={
              <ResponsePanel
                sender={tab().sender}
                onCopyCurl={() => openDialog({ kind: "copyCurl", source: { tabKey: tab().key } })}
              />
            }
          />
        </div>
      </div>
    </VarsProvider>
  );
}
