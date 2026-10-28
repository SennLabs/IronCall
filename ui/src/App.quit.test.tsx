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
  return mock;
}

describe("closing the window", () => {
  afterEach(() => setApi(tauriApi));

  it("asks about unsaved changes; Cancel keeps the window open", async () => {
    const mock = await setup();
    fireEvent.input(screen.getByLabelText("URL"), { target: { value: "https://x.io/healthz" } });
    mock.disk.dialog.choose = "cancel";
    expect(await mock.disk.requestClose()).toBe(false);
    expect(mock.disk.calls).toContain("choose");
  });

  it("Save writes the request and the session before closing", async () => {
    const mock = await setup();
    fireEvent.input(screen.getByLabelText("URL"), { target: { value: "https://x.io/healthz" } });
    mock.disk.dialog.choose = "yes";
    expect(await mock.disk.requestClose()).toBe(true);
    expect(mock.disk.diskRequest("R1")?.url).toBe("https://x.io/healthz");
    expect(mock.disk.session.tabs.length).toBeGreaterThan(0);
  });

  it("closes without asking when nothing is unsaved, flushing scratch autosaves", async () => {
    const mock = await setup();
    mock.disk.calls.length = 0;
    expect(await mock.disk.requestClose()).toBe(true);
    expect(mock.disk.calls).not.toContain("choose");
  });
});
