import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApi, tauriApi } from "../ipc/api";
import { createMockApi } from "../ipc/mock";
import { createAppStore } from "./app";
import { SCRATCH_AUTOSAVE_MS } from "./tabs";

const ROOT = "/home/me/demo";

function seed() {
  return {
    [ROOT]: {
      name: "Demo",
      id: "WS1",
      items: [
        {
          folder: "Users",
          id: "F1",
          children: [
            { request: "List users", id: "R1", url: "https://x.io/users" },
            { request: "Get user", id: "R2", url: "https://x.io/users/:id" },
          ],
        },
        { request: "Health", id: "R3", url: "https://x.io/health" },
      ],
    },
  };
}

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock = createMockApi({ workspaces: seed(), ...options });
  setApi(mock);
  let dispose!: () => void;
  const app = createRoot((d) => {
    dispose = d;
    return createAppStore();
  });
  await app.start();
  return { mock, app, tabs: app.tabs, dispose };
}

let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  vi.useRealTimers();
  setApi(tauriApi);
});

async function openDemo(o: Parameters<typeof createMockApi>[0] = {}) {
  const s = await setup(o);
  cleanup = s.dispose;
  await s.app.openWorkspace(ROOT);
  return s;
}

describe("tabs", () => {
  beforeEach(() => localStorage.clear());

  it("starts with one scratch tab when there's no session", async () => {
    const { tabs, mock, dispose } = await setup();
    cleanup = dispose;
    expect(tabs.tabs()).toHaveLength(1);
    expect(tabs.active()?.isScratch()).toBe(true);
    expect(mock.disk.scratch.size).toBe(1);
  });

  it("preview tabs replace each other; pinned ones stay", async () => {
    const { tabs } = await openDemo();
    await tabs.openRequest("R1", { pin: false });
    await tabs.openRequest("R2", { pin: false });
    expect(tabs.tabs().map((t) => t.key)).not.toContain("R1");
    expect(tabs.active()?.key).toBe("R2");
    await tabs.openRequest("R2", { pin: true });
    await tabs.openRequest("R3", { pin: false });
    expect(tabs.tabs().map((t) => t.key)).toEqual(expect.arrayContaining(["R2", "R3"]));
  });

  it("editing a preview tab pins it and makes it dirty; saving cleans it", async () => {
    const { tabs, mock } = await openDemo();
    const tab = await tabs.openRequest("R1", { pin: false });
    expect(tab?.pinned()).toBe(false);
    tab?.request.setMethod("post");
    expect(tab?.pinned()).toBe(true);
    expect(tab?.dirty()).toBe(true);
    expect(await tabs.save("R1")).toBe(true);
    expect(tab?.dirty()).toBe(false);
    expect(mock.disk.diskRequest("R1")?.method).toBe("POST");
  });

  it("closing a dirty tab asks: cancel keeps it, don't save discards, save writes", async () => {
    const { tabs, mock } = await openDemo();
    const tab = await tabs.openRequest("R1");
    tab?.request.setUrlText("https://x.io/changed");

    mock.disk.dialog.choose = "cancel";
    expect(await tabs.close("R1")).toBe(false);
    expect(tabs.findByRequest("R1")).toBeDefined();

    mock.disk.dialog.choose = "yes";
    expect(await tabs.close("R1")).toBe(true);
    expect(mock.disk.diskRequest("R1")?.url).toBe("https://x.io/changed");

    const again = await tabs.openRequest("R1");
    again?.request.setUrlText("https://x.io/discarded");
    mock.disk.dialog.choose = "no";
    expect(await tabs.close("R1")).toBe(true);
    expect(mock.disk.diskRequest("R1")?.url).toBe("https://x.io/changed");
  });

  it("clean tabs reload silently on external changes", async () => {
    const { tabs, mock } = await openDemo();
    const tab = await tabs.openRequest("R1");
    mock.disk.externalEdit("R1", { url: "https://x.io/from-git" });
    await vi.waitFor(() => expect(tab?.request.draft().url).toBe("https://x.io/from-git"));
    expect(tab?.status().kind).toBe("ok");
    expect(tab?.dirty()).toBe(false);
  });

  it("dirty tabs get a conflict: keep mine overwrites", async () => {
    const { tabs, mock } = await openDemo();
    const tab = await tabs.openRequest("R1");
    tab?.request.setMethod("PUT");
    mock.disk.externalEdit("R1", { url: "https://x.io/theirs" });
    await vi.waitFor(() => expect(tab?.status().kind).toBe("conflict"));

    const texts = tabs.compareTexts("R1");
    expect(texts?.mine).toContain("method: PUT");
    expect(texts?.theirs).toContain("https://x.io/theirs");

    await tabs.keepMine("R1");
    expect(tab?.status().kind).toBe("ok");
    expect(mock.disk.diskRequest("R1")).toMatchObject({ method: "PUT", url: "https://x.io/users" });
  });

  it("take theirs reloads from disk", async () => {
    const { tabs, mock } = await openDemo();
    const tab = await tabs.openRequest("R1");
    tab?.request.setMethod("PUT");
    mock.disk.externalEdit("R1", { url: "https://x.io/theirs" });
    await vi.waitFor(() => expect(tab?.status().kind).toBe("conflict"));
    await tabs.takeTheirs("R1");
    expect(tab?.request.draft()).toMatchObject({ method: "GET", url: "https://x.io/theirs" });
    expect(tab?.dirty()).toBe(false);
  });

  it("a save refused by the backend becomes a conflict", async () => {
    const { tabs, mock } = await openDemo();
    // Silence the change event so the tab doesn't learn about the edit first.
    const tab = await tabs.openRequest("R1");
    const disk = mock.disk.diskRequest("R1");
    if (disk) disk.url = "https://x.io/sneaky";
    tab?.request.setMethod("DELETE");
    expect(await tabs.save("R1")).toBe(false);
    expect(tab?.status()).toMatchObject({ kind: "conflict" });
  });

  it("files deleted on disk mark the tab; saving recreates them", async () => {
    const { tabs, mock, app } = await openDemo();
    const tab = await tabs.openRequest("R3");
    mock.disk.externalDelete("R3");
    await vi.waitFor(() => expect(tab?.status().kind).toBe("deleted"));
    expect(app.workspace.tree()?.children.map((c) => c.name)).toEqual(["Users"]);
    expect(await tabs.save("R3")).toBe(true);
    expect(mock.disk.diskRequest("R3")).toBeDefined();
  });

  it("scratch tabs autosave, debounced", async () => {
    const { tabs, mock, dispose } = await setup();
    cleanup = dispose;
    vi.useFakeTimers();
    const tab = tabs.active();
    const before = mock.disk.calls.filter((c) => c === "scratchSave").length;
    tab?.request.setUrlText("https://a");
    tab?.request.setUrlText("https://ab");
    tab?.request.setUrlText("https://abc");
    await vi.advanceTimersByTimeAsync(SCRATCH_AUTOSAVE_MS + 10);
    vi.useRealTimers();
    const after = mock.disk.calls.filter((c) => c === "scratchSave").length;
    expect(after - before).toBe(1);
    expect(mock.disk.scratch.get(tab?.key ?? "")?.url).toBe("https://abc");
    expect(tab?.dirty()).toBe(false);
  });

  it("save to workspace turns a scratch tab into a workspace request", async () => {
    const { tabs, mock, app } = await openDemo();
    const scratch = tabs.tabs().find((t) => t.isScratch());
    if (!scratch) throw new Error("expected a scratch tab");
    scratch.request.setUrlText("https://x.io/new");
    const scratchId = scratch.key;
    tabs.activate(scratchId);
    expect(await tabs.saveToWorkspace(scratchId, "F1")).toBe(true);
    expect(scratch.isScratch()).toBe(false);
    // The workspace copy has a new ID; the tab follows it and stays active.
    expect(scratch.key).not.toBe(scratchId);
    expect(scratch.ref()).toEqual({ kind: "request", id: scratch.key });
    expect(tabs.active()).toBe(scratch);
    expect(tabs.findByRequest(scratch.key)).toBe(scratch);
    expect(mock.disk.scratch.has(scratchId)).toBe(false);
    expect(mock.disk.diskRequest(scratch.key)?.url).toBe("https://x.io/new");
    expect(scratch.dirty()).toBe(false);
    const users = app.workspace.tree()?.children.find((c) => c.name === "Users");
    expect(users?.children.map((c) => c.id)).toContain(scratch.key);
  });

  it("undo and redo walk the draft history", async () => {
    const { tabs } = await openDemo();
    const tab = await tabs.openRequest("R1");
    tab?.request.setMethod("POST");
    tab?.state.history.breakBurst();
    tab?.request.setMethod("PUT");
    tabs.undo("R1");
    expect(tab?.request.draft().method).toBe("POST");
    tabs.undo("R1");
    expect(tab?.request.draft().method).toBe("GET");
    expect(tab?.dirty()).toBe(false);
    tabs.redo("R1");
    expect(tab?.request.draft().method).toBe("POST");
  });

  it("session is saved and restored (workspace, pinned tabs, scratch, active)", async () => {
    const first = await openDemo();
    await first.tabs.openRequest("R1");
    await first.tabs.openRequest("R2", { pin: false }); // preview: not restored
    await first.tabs.openRequest("R3");
    const saved = first.tabs.session(ROOT);
    expect(saved.tabs.map((t) => t.kind)).toEqual(["scratch", "request", "request"]);
    first.dispose();

    const scratchId = saved.tabs[0]?.id ?? "";
    const scratch = [...first.mock.disk.scratch.values()];
    const second = await setup({
      session: { ...saved, tabs: [...saved.tabs, { kind: "request", id: "GONE" }] },
      scratch,
    });
    cleanup = second.dispose;
    expect(second.app.workspace.info()?.root).toBe(ROOT);
    expect(second.tabs.tabs().map((t) => t.key)).toEqual([scratchId, "R1", "R3"]);
    expect(second.tabs.active()?.key).toBe("R3");
  });

  it("switching workspace closes its tabs (and can be cancelled)", async () => {
    const { tabs, app, mock } = await openDemo({
      workspaces: { ...seed(), "/other": { name: "Other" } },
    });
    const tab = await tabs.openRequest("R1");
    tab?.request.setMethod("PATCH");
    mock.disk.dialog.choose = "cancel";
    await app.openWorkspace("/other");
    expect(app.workspace.info()?.name).toBe("Demo");
    mock.disk.dialog.choose = "no";
    await app.openWorkspace("/other");
    expect(app.workspace.info()?.name).toBe("Other");
    expect(tabs.tabs().every((t) => t.isScratch())).toBe(true);
  });

  it("Ctrl+Tab order: next and previous wrap around", async () => {
    const { tabs } = await openDemo();
    await tabs.openRequest("R1");
    await tabs.openRequest("R3");
    const keys = tabs.tabs().map((t) => t.key);
    tabs.activate(keys[0] as string);
    tabs.next(1);
    expect(tabs.active()?.key).toBe(keys[1]);
    tabs.next(-1);
    tabs.next(-1);
    expect(tabs.active()?.key).toBe(keys[keys.length - 1]);
  });
});

describe("workspace store", () => {
  it("creates items, renames inline target, moves and deletes via trash confirm", async () => {
    const { app, mock } = await openDemo();
    const ws = app.workspace;
    const node = await ws.createItem("F1", "request");
    expect(ws.renaming()).toBe(node?.id);
    await ws.rename(node?.id ?? "", "Delete user");
    const users = () => ws.tree()?.children.find((c) => c.id === "F1");
    expect(users()?.children.map((c) => c.name)).toEqual(["List users", "Get user", "Delete user"]);

    await ws.move("R3", "F1", 0);
    expect(users()?.children[0]?.id).toBe("R3");

    mock.disk.dialog.confirm = false;
    expect(await ws.remove("R3")).toBe(false);
    mock.disk.dialog.confirm = true;
    expect(await ws.remove("R3")).toBe(true);
    expect(users()?.children.map((c) => c.id)).not.toContain("R3");
  });

  it("remembers expanded folders per workspace", async () => {
    const first = await openDemo();
    first.app.workspace.expand("F1");
    first.dispose();
    const second = await setup();
    cleanup = second.dispose;
    await second.app.openWorkspace(ROOT);
    expect(second.app.workspace.expanded().has("F1")).toBe(true);
  });
});

describe("mock dialogs", () => {
  it("create workspace picks a folder and prompts for a name", async () => {
    const { app, mock, dispose } = await setup();
    cleanup = dispose;
    mock.disk.dialog.folder = "/home/me/new-api";
    const { promptRequest } = await import("./dialogs");
    const done = app.createWorkspace();
    await vi.waitFor(() => expect(promptRequest()).toBeDefined());
    const d = promptRequest();
    if (!d) throw new Error("expected prompt");
    expect(d.initial).toBe("new-api");
    d.resolve("New API");
    await done;
    expect(app.workspace.info()).toMatchObject({ name: "New API", root: "/home/me/new-api" });
  });
});
