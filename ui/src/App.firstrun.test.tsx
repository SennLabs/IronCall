import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi } from "./ipc/mock";

describe("first run", () => {
  afterEach(() => setApi(tauriApi));

  it("creates the sample workspace and opens its first request", async () => {
    const mock = createMockApi();
    mock.disk.dialog.folder = "/home/me/sample";
    setApi(mock);
    render(() => <App />);
    fireEvent.click(await screen.findByRole("button", { name: "Try the sample workspace…" }));
    const tree = await screen.findByRole("tree", { name: "Workspace" });
    await waitFor(() => expect(screen.getByRole("tab", { name: /Echo a GET/ })).toBeTruthy());
    expect(within(tree).getByRole("treeitem", { name: "Basics" })).toBeTruthy();
    expect(within(tree).getByRole("treeitem", { name: "Chaining" })).toBeTruthy();
    expect((await mock.workspaceCurrent())?.name).toBe("IronCall sample");
  });

  it("does nothing if the folder picker is cancelled", async () => {
    const mock = createMockApi();
    mock.disk.dialog.folder = null;
    setApi(mock);
    render(() => <App />);
    fireEvent.click(await screen.findByRole("button", { name: "Try the sample workspace…" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(await mock.workspaceCurrent()).toBeNull();
  });
});
