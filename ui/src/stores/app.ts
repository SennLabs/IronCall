/**
 * App-level wiring: startup and session restore, workspace switching, change events
 * from the file watcher, and session autosave.
 */

import { createEffect, createSignal, on, onCleanup } from "solid-js";
import { api, type HistoryEntryDraft, type ItemId } from "../ipc/api";
import { bodyFilePaths } from "../model/bodyFiles";
import { adoptParsed, curlNoticeText, isEmptyDraft } from "../model/curl";
import { allRequests } from "../model/tree";
import { missingSecretsUsed } from "../model/variables";
import { openDialog, prompt } from "./dialogs";
import { createHistoryStore, requestForNewTab } from "./history";
import { showImportNotice } from "./importNotice";
import { bumpCookies } from "./network";
import { attempt, notify } from "./notify";
import { createRunnerStore } from "./runner";
import { createTabsStore, type Tab } from "./tabs";
import { createVarsStore } from "./vars";
import { createWorkspaceStore } from "./workspace";

export const SESSION_SAVE_MS = 500;

export type AppStore = ReturnType<typeof createAppStore>;

export function createAppStore() {
  const workspace = createWorkspaceStore({
    // Our own writes don't come back through the watcher; tell open tabs directly.
    onRenamed: (id) => void tabs.renamed(id),
    onRemoved: (ids) => tabs.removed(ids),
  });
  const vars = createVarsStore();
  const tabs = createTabsStore({
    onWrite: () => void workspace.refresh(),
    varsVersion: vars.version,
  });
  const [ready, setReady] = createSignal(false);
  const history = createHistoryStore({ workspaceId: () => workspace.info()?.id ?? null });
  const runner = createRunnerStore({
    onFinished: () => {
      // Runs extract into the session and record every request in history.
      vars.bump();
      history.sendRecorded();
    },
  });

  let unlisten: (() => void) | undefined;
  let disposed = false;
  onCleanup(() => {
    disposed = true;
    unlisten?.();
  });

  // Save the session whenever tabs, the active tab or the workspace change.
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(
      () => (ready() ? JSON.stringify(tabs.session(workspace.info()?.root ?? null)) : ""),
      (json) => {
        if (!json) return;
        clearTimeout(timer);
        timer = setTimeout(() => {
          void attempt(() => api.sessionSave(JSON.parse(json)), "Couldn't save the session");
        }, SESSION_SAVE_MS);
      },
      { defer: true },
    ),
  );
  onCleanup(() => clearTimeout(timer));

  // Environments, the active environment and secrets belong to the open workspace.
  createEffect(
    on(
      () => workspace.info()?.root ?? null,
      (root) => {
        void vars.refresh(root !== null);
        history.workspaceChanged();
      },
      { defer: true },
    ),
  );

  /** Closes the current workspace's tabs; false if the user cancelled. */
  const leaveWorkspace = async () => !workspace.info() || (await tabs.closeWorkspaceTabs());

  const store = {
    workspace,
    tabs,
    vars,
    history,
    runner,
    ready,

    /** Opens the collection runner for a folder, or the whole workspace with `null`. */
    async openRunner(folder: ItemId | null, name: string) {
      openDialog({ kind: "runner" });
      await runner.prepare({ folder }, name, workspace.info()?.root ?? null);
    },

    /**
     * Sends a tab's request. Refuses if a body file is missing or outside the workspace
     * and not picked this session (PLAN.md Phase 4), and warns if it uses secrets with no
     * value on this machine (PLAN.md §4.5).
     */
    async send(tab: Tab) {
      if (tab.sender.isSending()) return;
      const draft = tab.request.draft();
      if (bodyFilePaths(draft).length > 0) {
        const files = await attempt(
          () => api.bodyFilesCheck(draft),
          "Couldn't check the body files",
        );
        if (!files) return;
        const bad = files.find((f) => !f.exists || !f.allowed);
        if (bad) {
          notify(
            bad.exists
              ? `Can't send: ${bad.path} is outside the workspace. Re-select this file to allow it this session.`
              : `Can't send: ${bad.path} doesn't exist.`,
            "error",
          );
          return;
        }
      }
      const missing = missingSecretsUsed(draft, tab.vars.context());
      if (missing.length > 0) {
        const ok = await api.confirm(
          `This request uses ${missing.length === 1 ? "a secret" : "secrets"} with no value on this machine: ${missing.join(", ")}. The {{references}} would be sent as-is.`,
          { title: "Missing secrets", okLabel: "Send anyway", kind: "warning" },
        );
        if (!ok) return;
      }
      await tab.sender.send(draft);
      // The response may have set cookies, and the send was recorded in history.
      bumpCookies();
      history.sendRecorded();
      // Extracted values are runtime variables now: re-resolve every tab.
      const s = tab.sender.state();
      if (s.status === "done" && s.response.checks?.extracted.some((e) => e.error === null)) {
        vars.bump();
      }
    },

    /**
     * Opens a history entry's authored request in a new scratch tab (new ID, same
     * `{{variables}}`), optionally sending it right away.
     */
    async openFromHistory(entry: HistoryEntryDraft, send: boolean) {
      const draft = await requestForNewTab(entry);
      const tab = tabs.openScratch(draft);
      await attempt(() => api.scratchSave(draft), "Couldn't save the scratch request");
      if (send) await store.send(tab);
      return tab;
    },

    /**
     * Imports a cURL command (PLAN.md Phase 6). An empty tab is replaced in place (one
     * undo step; it keeps its ID); otherwise a new scratch tab opens. Warnings appear as
     * a dismissable notice on the tab. Resolves the tab, or undefined if parsing failed.
     */
    async importCurl(command: string, into?: Tab): Promise<Tab | undefined> {
      const parsed = await attempt(
        () => api.curlParse(command),
        "Couldn't import the cURL command",
      );
      if (!parsed) return undefined;
      let tab: Tab;
      if (into && !into.readOnly() && isEmptyDraft(into.request.draft())) {
        tabs.replaceRequest(into.key, adoptParsed(into.request.draft(), parsed.request));
        tab = into;
      } else {
        tab = tabs.openScratch(parsed.request);
        await attempt(() => api.scratchSave(parsed.request), "Couldn't save the scratch request");
      }
      showImportNotice(tab.key, curlNoticeText(parsed.warnings));
      return tab;
    },

    async start() {
      const off = await api.onWorkspaceChanged((change) => {
        if (change.tree) workspace.applyTree(change.tree);
        void tabs.handleChange(change);
        // Variables may live in ironcall.yaml, _folder.yaml or environment files.
        if (change.environmentsChanged || change.metaChanged) void vars.refresh();
      });
      if (disposed) off();
      else unlisten = off;

      const session = (await attempt(() => api.sessionLoad(), "Couldn't restore the session")) ?? {
        workspace: null,
        tabs: [],
        active: null,
      };
      // Reuse an already-open backend workspace (e.g. after a webview reload).
      await workspace.adopt();
      if (session.workspace && workspace.info()?.root !== session.workspace) {
        await workspace.open(session.workspace);
      }
      await vars.refresh(!!workspace.info());
      await tabs.restore(session);
      if (tabs.tabs().length === 0) await tabs.newScratch();
      await workspace.refreshRecent();
      setReady(true);
    },

    async openWorkspace(path?: string) {
      const target = path ?? (await api.pickFolder("Open workspace"));
      if (!target || !(await leaveWorkspace())) return;
      await workspace.open(target);
    },

    async createWorkspace() {
      const folder = await api.pickFolder("Choose a folder for the new workspace");
      if (!folder) return;
      const suggested = folder.split(/[\\/]/).filter(Boolean).pop() ?? "My API";
      const name = await prompt("New workspace", "Workspace name", suggested, "Create");
      if (!name?.trim() || !(await leaveWorkspace())) return;
      await workspace.create(folder, name.trim());
    },

    /** First run: the sample workspace, with its first request open (PLAN.md Phase 8). */
    async createSampleWorkspace() {
      const folder = await api.pickFolder("Choose an empty folder for the sample workspace");
      if (!folder || !(await leaveWorkspace())) return;
      const w = await workspace.createSample(folder);
      const first = w && allRequests(w.tree)[0];
      if (first) await tabs.openRequest(first.id, { pin: true });
    },

    /**
     * The window is closing: save scratch requests, ask about unsaved tabs, and write the
     * session now (its debounced save would otherwise be lost). False keeps the window.
     */
    async beforeQuit() {
      if (!(await tabs.beforeQuit())) return false;
      await attempt(
        () => api.sessionSave(tabs.session(workspace.info()?.root ?? null)),
        "Couldn't save the session",
      );
      return true;
    },

    async closeWorkspace() {
      if (await leaveWorkspace()) await workspace.close();
    },

    /** Opens a request from the tree; single click previews, double click pins. */
    openRequest: (id: string, pin: boolean) => tabs.openRequest(id, { pin }),
  };
  return store;
}
