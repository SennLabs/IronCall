import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi, mockDraft } from "./ipc/mock";

const ping = mockDraft({ id: "01PING", name: "Ping", url: "https://x.io/ping" });

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock: MockApi = createMockApi({
    session: { workspace: null, tabs: [], active: null },
    scratch: [ping],
    scratchLayout: { folders: [{ id: "F1", name: "Auth", parent: null }], placement: {} },
    ...options,
  });
  setApi(mock);
  render(() => <App />);
  const scratch = await screen.findByRole("region", { name: "Scratch requests" });
  await within(scratch).findByRole("button", { name: "Auth" });
  return { mock, scratch };
}

/** Scratch rows in display order (folder names, request URLs). */
const order = (scratch: HTMLElement) =>
  within(scratch)
    .getAllByRole("listitem")
    .map((li) => li.textContent?.replace(/[▾▸]/g, "").trim());

describe("scratch folders", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => setApi(tauriApi));

  it("right-clicking the section creates a folder and starts renaming it", async () => {
    const { mock, scratch } = await setup();
    fireEvent.contextMenu(within(scratch).getByRole("button", { name: /Scratch/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "New folder" }));
    const input = (await within(scratch).findByRole("textbox", {
      name: "Name",
    })) as HTMLInputElement;
    expect(input.value).toBe("New folder");
    fireEvent.input(input, { target: { value: "Billing" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await within(scratch).findByRole("button", { name: "Billing" });
    expect(mock.disk.scratchLayout.folders.map((f) => f.name)).toEqual(["Auth", "Billing"]);
  });

  it("a folder's context menu creates a request inside it", async () => {
    const { mock, scratch } = await setup();
    fireEvent.contextMenu(within(scratch).getByRole("button", { name: "Auth" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "New request" }));
    await waitFor(() => expect(Object.values(mock.disk.scratchLayout.placement)).toEqual(["F1"]));
    const [id] = Object.keys(mock.disk.scratchLayout.placement);
    expect(mock.disk.scratch.has(id as string)).toBe(true);
    await waitFor(() => expect(order(scratch)[1]).toMatch(/^GET/));
    expect(order(scratch)[0]).toMatch(/^Auth/);
  });

  it("dragging a request onto a folder moves it there; the menu moves it back", async () => {
    const { mock, scratch } = await setup();
    const row = within(scratch).getByRole("button", { name: /x\.io\/ping/ });
    const folder = within(scratch).getByRole("button", { name: "Auth" });
    fireEvent.dragStart(row, { dataTransfer: { setData() {}, effectAllowed: "" } });
    fireEvent.dragOver(folder, { dataTransfer: { dropEffect: "" } });
    fireEvent.drop(folder);
    await waitFor(() => expect(mock.disk.scratchLayout.placement[ping.id]).toBe("F1"));

    fireEvent.contextMenu(within(scratch).getByRole("button", { name: /x\.io\/ping/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Move to top level" }));
    await waitFor(() => expect(mock.disk.scratchLayout.placement[ping.id]).toBeUndefined());
  });

  it("collapsing a folder hides its requests", async () => {
    const { scratch } = await setup({
      scratchLayout: {
        folders: [{ id: "F1", name: "Auth", parent: null }],
        placement: { [ping.id]: "F1" },
      },
    });
    await within(scratch).findByRole("button", { name: /x\.io\/ping/ });
    fireEvent.click(within(scratch).getByRole("button", { name: "Auth" }));
    await waitFor(() =>
      expect(within(scratch).queryByRole("button", { name: /x\.io\/ping/ })).toBeNull(),
    );
  });

  it("deleting a folder asks, then deletes the requests in it", async () => {
    const { mock, scratch } = await setup({
      scratchLayout: {
        folders: [{ id: "F1", name: "Auth", parent: null }],
        placement: { [ping.id]: "F1" },
      },
    });
    fireEvent.contextMenu(within(scratch).getByRole("button", { name: "Auth" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    await waitFor(() => expect(mock.disk.scratchLayout.folders).toEqual([]));
    expect(mock.disk.calls).toContain("confirm");
    expect(mock.disk.scratch.has(ping.id)).toBe(false);
    expect(within(scratch).queryByRole("button", { name: "Auth" })).toBeNull();
  });
});
