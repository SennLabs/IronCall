import { createRoot } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { type HistoryEntryDraft, setApi, tauriApi } from "../ipc/api";
import { createMockApi, mockDraft } from "../ipc/mock";
import { mockHistoryId } from "../ipc/mockHistory";
import { createHistoryStore, requestForNewTab } from "./history";

function entry(over: {
  method?: string;
  url?: string;
  status?: number | null;
  workspace?: string | null;
}): HistoryEntryDraft {
  const id = mockHistoryId();
  return {
    summary: {
      id,
      at: "2026-10-05T10:00:00Z",
      workspace: over.workspace === undefined ? "WS1" : over.workspace,
      workspaceName: "Demo",
      requestId: "REQ",
      requestName: "Get user",
      method: over.method ?? "GET",
      url: over.url ?? `https://api.test/${id}`,
      status: over.status === undefined ? 200 : over.status,
      error: over.status === null ? "connect" : null,
      totalMs: 12,
      size: 10,
      hasBody: false,
    },
    request: mockDraft({
      url: "{{base}}/users/{{id}}",
      headers: [{ name: "X-Token", value: "{{token}}" }],
    }),
    sent: null,
    response: null,
    errorMessage: null,
  };
}

const tick = () => new Promise((r) => setTimeout(r, 5));

function setup(entries: HistoryEntryDraft[], workspaceId: string | null = "WS1", pageSize = 3) {
  const mock = createMockApi({ history: { entries } });
  setApi(mock);
  mock.disk.dialog.confirm = true;
  const store = createRoot(() =>
    createHistoryStore({ workspaceId: () => workspaceId, pageSize, debounceMs: 0 }),
  );
  return { mock, store };
}

describe("history store", () => {
  afterEach(() => setApi(tauriApi));

  it("pages newest first with infinite scroll", async () => {
    const entries = Array.from({ length: 7 }, () => entry({}));
    const { store } = setup(entries);
    store.setOpen(true);
    await tick();
    const newest = [...entries].reverse().map((e) => e.summary.id);
    expect(store.entries().map((e) => e.id)).toEqual(newest.slice(0, 3));
    await store.loadMore();
    await store.loadMore();
    expect(store.entries().map((e) => e.id)).toEqual(newest);
    expect(store.next()).toBeNull();
  });

  it("searches and filters by scope, method and status class", async () => {
    const { store } = setup(
      [
        entry({ url: "https://api.test/users", method: "GET" }),
        entry({ url: "https://api.test/orders", method: "POST", status: 500 }),
        entry({ url: "https://other.test/users", method: "GET", workspace: "WS2" }),
        entry({ url: "https://api.test/fail", status: null }),
      ],
      "WS1",
      50,
    );
    store.setOpen(true);
    await tick();
    expect(store.entries()).toHaveLength(3); // this workspace only
    store.setScope("all");
    await tick();
    expect(store.entries()).toHaveLength(4);
    store.setText("users");
    await tick();
    expect(store.entries().map((e) => e.url)).toEqual([
      "https://other.test/users",
      "https://api.test/users",
    ]);
    store.setText("");
    store.toggleMethod("POST");
    await tick();
    expect(store.entries().map((e) => e.method)).toEqual(["POST"]);
    store.toggleMethod("POST");
    store.toggleStatus("failed");
    await tick();
    expect(store.entries().map((e) => e.status)).toEqual([null]);
  });

  it("deletes one entry and clears a workspace's or all history", async () => {
    const entries = [entry({}), entry({}), entry({ workspace: "WS2" })];
    const { mock, store } = setup(entries);
    store.setScope("all");
    store.setOpen(true);
    await tick();
    await store.select(entries[0]?.summary.id);
    await store.remove(entries[0]?.summary.id as string);
    expect(store.entries()).toHaveLength(2);
    expect(store.selected()).toBeUndefined();

    await store.clear("workspace");
    expect(mock.history.calls.at(-2)).toEqual({ op: "historyClear", args: ["WS1"] });
    expect(store.entries().map((e) => e.workspace)).toEqual(["WS2"]);

    await store.clear("all");
    expect(store.entries()).toEqual([]);
  });

  it("asks before clearing and does nothing if cancelled", async () => {
    const { mock, store } = setup([entry({})]);
    mock.disk.dialog.confirm = false;
    store.setOpen(true);
    await tick();
    await store.clear("all");
    expect(store.entries()).toHaveLength(1);
  });

  it("reloads when a send is recorded while open", async () => {
    // No workspace is open in the mock, so sends are recorded without one: show "all".
    const { mock, store } = setup([], null);
    store.setScope("all");
    store.setOpen(true);
    await tick();
    expect(store.entries()).toHaveLength(0);
    await mock.sendRequest("s", mockDraft({ url: "https://api.test/new" }));
    store.sendRecorded();
    await tick();
    expect(store.entries()).toHaveLength(1);
  });

  it("opens an entry in a new tab with a new ID and the authored {{variables}}", async () => {
    const e = entry({});
    setup([e]);
    const draft = await requestForNewTab(e);
    expect(draft.id).not.toBe(e.request.id);
    expect(draft.url).toBe("{{base}}/users/{{id}}");
    expect(draft.headers).toEqual([{ name: "X-Token", value: "{{token}}" }]);
  });
});
