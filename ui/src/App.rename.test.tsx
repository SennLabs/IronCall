import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi } from "./ipc/mock";

const ROOT = "/home/me/demo";

async function setup() {
  const mock: MockApi = createMockApi({
    workspaces: {
      [ROOT]: {
        name: "Demo",
        items: [{ request: "Health", id: "R1", url: "https://x.io/health" }],
      },
    },
    session: { workspace: ROOT, tabs: [], active: null },
  });
  setApi(mock);
  render(() => <App />);
  const tree = await screen.findByRole("tree", { name: "Workspace" });
  fireEvent.dblClick(within(tree).getByRole("treeitem", { name: "Health" }));
  await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue("https://x.io/health"));
  return { mock, tree };
}

async function renameInTree(tree: HTMLElement, from: string, to: string) {
  fireEvent.contextMenu(within(tree).getByRole("treeitem", { name: from }));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Rename\s*(F2)?$/ }));
  const input = await within(tree).findByRole("textbox", { name: "Name" });
  fireEvent.input(input, { target: { value: to } });
  fireEvent.keyDown(input, { key: "Enter" });
}

describe("tree changes reach open tabs", () => {
  afterEach(() => setApi(tauriApi));

  it("renaming a request with unsaved edits keeps them and saves without a conflict", async () => {
    const { mock, tree } = await setup();
    fireEvent.input(screen.getByLabelText("URL"), { target: { value: "https://x.io/ping" } });
    await renameInTree(tree, "Health", "Ping");
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /^Ping, unsaved changes/ })).toBeTruthy(),
    );
    expect(screen.getByLabelText("URL")).toHaveValue("https://x.io/ping");

    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(mock.disk.diskRequest("R1")?.url).toBe("https://x.io/ping"));
    expect(mock.disk.diskRequest("R1")?.name).toBe("Ping");
    expect(screen.queryByRole("tab", { name: /changed on disk/ })).toBeNull();
  });

  it("renaming a clean tab updates its title", async () => {
    const { tree } = await setup();
    await renameInTree(tree, "Health", "Status");
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Status$/ })).toBeTruthy());
  });

  it("deleting a request from the tree marks its tab deleted", async () => {
    const { tree } = await setup();
    fireEvent.contextMenu(within(tree).getByRole("treeitem", { name: "Health" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Delete/ }));
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /Health, deleted on disk/ })).toBeTruthy(),
    );
  });
});
