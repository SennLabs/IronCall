import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi } from "./ipc/mock";

const ROOT = "/home/me/demo";

function workspaces() {
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
        { error: "broken.yaml", id: "E1", message: "line 4: unresolved merge conflict marker" },
      ],
    },
  };
}

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock: MockApi = createMockApi({
    workspaces: workspaces(),
    session: { workspace: ROOT, tabs: [], active: null },
    ...options,
  });
  setApi(mock);
  render(() => <App />);
  const tree = await screen.findByRole("tree", { name: "Workspace" });
  return { mock, tree };
}

/** The URL input of the active tab (re-queried: switching tabs remounts the view). */
async function activeUrl(expected: string): Promise<HTMLInputElement> {
  await waitFor(() =>
    expect((screen.getByLabelText("URL") as HTMLInputElement).value).toBe(expected),
  );
  return screen.getByLabelText("URL") as HTMLInputElement;
}

const item = (tree: HTMLElement, name: string) => within(tree).getByRole("treeitem", { name });
const tabNamed = (name: RegExp) => screen.getByRole("tab", { name });

describe("workspace UI", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => setApi(tauriApi));

  it("shows the tree in backend order with folders collapsed and error nodes marked", async () => {
    const { tree } = await setup();
    const names = within(tree)
      .getAllByRole("treeitem")
      .map((el) => el.getAttribute("aria-label"));
    expect(names).toEqual(["Demo", "Users", "Health", "broken.yaml (error)"]);
    expect(item(tree, "Users")).toHaveAttribute("aria-expanded", "false");
  });

  it("clicking a folder expands it; clicking a request opens a preview tab, double-click pins", async () => {
    const { tree } = await setup();
    fireEvent.click(item(tree, "Users"));
    expect(item(tree, "Users")).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(item(tree, "List users"));
    await waitFor(() => expect(tabNamed(/List users/)).toBeInTheDocument());
    fireEvent.click(item(tree, "Get user"));
    await waitFor(() => expect(tabNamed(/Get user/)).toBeInTheDocument());
    expect(screen.queryByRole("tab", { name: /List users/ })).toBeNull();
    fireEvent.dblClick(item(tree, "Get user"));
    fireEvent.click(item(tree, "Health"));
    await waitFor(() => expect(tabNamed(/Health/)).toBeInTheDocument());
    expect(tabNamed(/Get user/)).toBeInTheDocument();
  });

  it("filters the tree and highlights matches", async () => {
    const { tree } = await setup();
    fireEvent.input(screen.getByRole("searchbox", { name: "Filter requests" }), {
      target: { value: "get" },
    });
    const names = within(tree)
      .getAllByRole("treeitem")
      .map((el) => el.getAttribute("aria-label"));
    expect(names).toEqual(["Demo", "Users", "Get user"]);
    expect(item(tree, "Get user").querySelector("mark")?.textContent).toBe("Get");
  });

  it("edits show a dirty tab; Ctrl+S saves to disk", async () => {
    const { tree, mock } = await setup();
    fireEvent.dblClick(item(tree, "Health"));
    const url = await activeUrl("https://x.io/health");
    fireEvent.input(url, { target: { value: "https://x.io/healthz" } });
    expect(tabNamed(/Health, unsaved changes/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Method"), { target: { value: "POST" } });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(mock.disk.diskRequest("R3")?.url).toBe("https://x.io/healthz"));
    // The tree's method badge follows the save.
    await waitFor(() => expect(item(tree, "Health")).toHaveTextContent("POST"));
    expect(await screen.findByRole("tab", { name: /^Health$/ })).toBeInTheDocument();
  });

  it("closing a dirty tab asks; cancel keeps it open", async () => {
    const { tree, mock } = await setup();
    fireEvent.dblClick(item(tree, "Health"));
    const url = await activeUrl("https://x.io/health");
    fireEvent.input(url, { target: { value: "https://x.io/other" } });
    mock.disk.dialog.choose = "cancel";
    fireEvent.click(screen.getByLabelText("Close Health"));
    await waitFor(() => expect(mock.disk.calls).toContain("choose"));
    expect(tabNamed(/Health/)).toBeInTheDocument();
  });

  it("external edits to a dirty tab show the conflict bar; Compare shows a diff; Keep mine saves", async () => {
    const { tree, mock } = await setup();
    fireEvent.dblClick(item(tree, "Health"));
    const url = await activeUrl("https://x.io/health");
    fireEvent.input(url, { target: { value: "https://x.io/mine" } });
    mock.disk.externalEdit("R3", { url: "https://x.io/theirs" });

    const bar = await screen.findByRole("alert", { name: "Conflict" });
    fireEvent.click(within(bar).getByRole("button", { name: "Compare" }));
    const diff = await screen.findByLabelText("Differences");
    expect(diff).toHaveTextContent('− url: "https://x.io/theirs"');
    expect(diff).toHaveTextContent('+ url: "https://x.io/mine"');
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Keep mine" }));
    await waitFor(() => expect(mock.disk.diskRequest("R3")?.url).toBe("https://x.io/mine"));
    await waitFor(() => expect(screen.queryByRole("alert", { name: "Conflict" })).toBeNull());
  });

  it("Take theirs reloads the file", async () => {
    const { tree, mock } = await setup();
    fireEvent.dblClick(item(tree, "Health"));
    const url = await activeUrl("https://x.io/health");
    fireEvent.input(url, { target: { value: "https://x.io/mine" } });
    mock.disk.externalEdit("R3", { url: "https://x.io/theirs" });
    const bar = await screen.findByRole("alert", { name: "Conflict" });
    fireEvent.click(within(bar).getByRole("button", { name: "Take theirs" }));
    await waitFor(() =>
      expect((screen.getByLabelText("URL") as HTMLInputElement).value).toBe("https://x.io/theirs"),
    );
  });

  it("selecting an error node shows what's wrong with open/reveal actions", async () => {
    const { tree, mock } = await setup();
    fireEvent.click(item(tree, "broken.yaml (error)"));
    const panel = screen.getByRole("region", { name: "File error" });
    expect(panel).toHaveTextContent("unresolved merge conflict marker");
    fireEvent.click(within(panel).getByRole("button", { name: "Open in external editor" }));
    await waitFor(() => expect(mock.disk.calls).toContain("itemOpenExternal:E1"));
  });

  it("keyboard: arrows move, Enter opens, F2 renames inline", async () => {
    const { tree, mock } = await setup();
    tree.focus();
    fireEvent.focus(tree);
    fireEvent.keyDown(tree, { key: "ArrowDown" }); // Users
    fireEvent.keyDown(tree, { key: "ArrowRight" }); // expand
    fireEvent.keyDown(tree, { key: "ArrowDown" }); // List users
    expect(item(tree, "List users")).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(tree, { key: "Enter" });
    await waitFor(() => expect(tabNamed(/List users/)).toBeInTheDocument());

    fireEvent.keyDown(tree, { key: "F2" });
    const input = within(tree).getByRole("textbox", { name: "Name" }) as HTMLInputElement;
    fireEvent.input(input, { target: { value: "All users" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(item(tree, "All users")).toBeInTheDocument());
    expect(mock.disk.diskRequest("R1")?.name).toBe("All users");
  });

  it("context menu: new request creates and starts renaming; delete asks and trashes", async () => {
    const { tree, mock } = await setup();
    fireEvent.contextMenu(item(tree, "Users"));
    fireEvent.click(screen.getByRole("menuitem", { name: "New request" }));
    const input = (await within(tree).findByRole("textbox", { name: "Name" })) as HTMLInputElement;
    expect(input.value).toBe("New request");
    fireEvent.keyDown(input, { key: "Escape" });

    fireEvent.contextMenu(item(tree, "Health"));
    fireEvent.click(screen.getByRole("menuitem", { name: /Delete/ }));
    await waitFor(() =>
      expect(within(tree).queryByRole("treeitem", { name: "Health" })).toBeNull(),
    );
    expect(mock.disk.calls).toEqual(expect.arrayContaining(["confirm", "itemDelete"]));
  });

  it("right-clicking empty space below the rows offers new items at the top level", async () => {
    const { tree } = await setup();
    fireEvent.contextMenu(tree);
    expect(screen.getByRole("menu", { name: "Demo actions" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "New folder" }));
    const input = (await within(tree).findByRole("textbox", { name: "Name" })) as HTMLInputElement;
    expect(input.value).toBe("New folder");
    // Created in the root, not in whatever was selected.
    const row = input.closest('[role="treeitem"]') as HTMLElement;
    expect(row.getAttribute("aria-level")).toBe("2");
  });

  it("drag and drop moves a request into a folder", async () => {
    const { tree, mock } = await setup();
    const health = item(tree, "Health");
    const users = item(tree, "Users");
    fireEvent.dragStart(health, { dataTransfer: { setData() {}, effectAllowed: "" } });
    // Middle of the row (rows are 24px; jsdom reports a zero rect) → "inside".
    fireEvent.dragOver(users, { clientY: 12, dataTransfer: { dropEffect: "" } });
    fireEvent.drop(users, { clientY: 12 });
    await waitFor(() => expect(mock.disk.calls).toContain("itemMove"));
    fireEvent.click(item(tree, "Users"));
    await waitFor(() => expect(item(tree, "Users")).toHaveAttribute("aria-expanded", "true"));
    const names = within(tree)
      .getAllByRole("treeitem")
      .map((el) => el.getAttribute("aria-label"));
    expect(names.indexOf("Health")).toBeGreaterThan(names.indexOf("Get user"));
  });

  it("closing every tab shows the welcome screen; New request opens a scratch tab", async () => {
    await setup();
    for (const close of screen.getAllByLabelText(/^Close /)) fireEvent.click(close);
    expect(await screen.findByRole("button", { name: /New request Ctrl\+N/ })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Recent workspaces" })).toHaveTextContent("Demo");
    fireEvent.click(screen.getByRole("button", { name: /New request Ctrl\+N/ }));
    expect(await screen.findByLabelText("URL")).toBeInTheDocument();
  });

  it("scratch tabs can be saved to a workspace folder", async () => {
    const { mock } = await setup();
    const url = (await screen.findByLabelText("URL")) as HTMLInputElement;
    fireEvent.input(url, { target: { value: "https://x.io/scratch" } });
    fireEvent.click(screen.getByRole("button", { name: "Save to workspace…" }));
    const dialog = await screen.findByRole("dialog", { name: "Save to workspace" });
    fireEvent.click(within(dialog).getByRole("radio", { name: "Users" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mock.disk.calls).toContain("scratchSaveToWorkspace"));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Save to workspace…" })).toBeNull(),
    );
  });

  it("undo and redo with Ctrl+Z / Ctrl+Shift+Z", async () => {
    await setup();
    const url = (await screen.findByLabelText("URL")) as HTMLInputElement;
    fireEvent.input(url, { target: { value: "https://x.io/a" } });
    fireEvent.keyDown(url, { key: "z", ctrlKey: true });
    await waitFor(() => expect(url.value).toBe(""));
    fireEvent.keyDown(url, { key: "z", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(url.value).toBe("https://x.io/a"));
  });

  it("starts with the workspace switcher and no-workspace sidebar when nothing is open", async () => {
    const mock = createMockApi({ workspaces: workspaces() });
    setApi(mock);
    render(() => <App />);
    expect(await screen.findByText("No workspace open.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Demo/ }));
    expect(await screen.findByRole("tree", { name: "Workspace" })).toBeInTheDocument();
  });
});
