import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, mockDraft } from "./ipc/mock";
import { closeDialog } from "./stores/dialogs";

const ROOT = "/home/me/demo";

async function setup() {
  const mock = createMockApi({
    workspaces: {
      [ROOT]: {
        name: "Demo",
        items: [
          {
            folder: "Users",
            id: "F1",
            children: [{ request: "List users", id: "R1", url: "https://x.io/users" }],
          },
          { request: "Health", id: "R2", url: "https://x.io/health" },
        ],
      },
    },
    session: { workspace: ROOT, tabs: [], active: null },
    scratch: [mockDraft({ id: "S1", name: "Try webhook", url: "https://hook.test/in" })],
  });
  setApi(mock);
  render(() => <App />);
  await screen.findByRole("tree", { name: "Workspace" });
  return mock;
}

const ctrl = (key: string) => fireEvent.keyDown(window, { key, ctrlKey: true });

describe("keyboard shortcuts", () => {
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("Ctrl+P finds a request by folder and opens it with Enter", async () => {
    await setup();
    ctrl("p");
    const dialog = await screen.findByRole("dialog", { name: "Go to request" });
    const search = within(dialog).getByRole("combobox", { name: "Search requests" });
    expect(document.activeElement).toBe(search);
    // Workspace requests plus scratch ones (listed asynchronously).
    await waitFor(() =>
      expect(within(dialog).getAllByRole("option").length).toBeGreaterThanOrEqual(3),
    );

    fireEvent.input(search, { target: { value: "users list" } });
    const options = within(dialog).getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("Users");
    fireEvent.keyDown(search, { key: "Enter" });
    await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue("https://x.io/users"));
    expect(screen.queryByRole("dialog", { name: "Go to request" })).toBeNull();
  });

  it("Ctrl+P opens scratch requests too, and arrows move the selection", async () => {
    await setup();
    ctrl("p");
    const dialog = await screen.findByRole("dialog", { name: "Go to request" });
    const search = within(dialog).getByRole("combobox");
    fireEvent.input(search, { target: { value: "e" } });
    const first = within(dialog).getAllByRole("option")[0] as HTMLElement;
    expect(first).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(first).toHaveAttribute("aria-selected", "false");

    fireEvent.input(search, { target: { value: "webhook" } });
    fireEvent.click(await within(dialog).findByRole("option", { name: /Try webhook/ }));
    await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue("https://hook.test/in"));
  });

  it("Ctrl+L focuses the URL bar", async () => {
    await setup();
    const url = await screen.findByLabelText("URL");
    ctrl("l");
    expect(document.activeElement).toBe(url);
  });
});
