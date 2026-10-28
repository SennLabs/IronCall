import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi } from "./ipc/mock";
import { reportUncaughtErrors } from "./stores/diagnostics";
import { closeDialog } from "./stores/dialogs";
import { setTheme } from "./theme/theme";

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock: MockApi = createMockApi(options);
  setApi(mock);
  render(() => <App />);
  await screen.findByLabelText("URL");
  // Settings load on start.
  await waitFor(() => expect(mock.settings.zooms.length).toBeGreaterThan(0));
  return mock;
}

const ctrl = (key: string) => fireEvent.keyDown(window, { key, ctrlKey: true });

describe("settings", () => {
  beforeEach(() => {
    localStorage.clear();
    setTheme("system");
  });
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("closing a dialog returns focus to the button that opened it", async () => {
    await setup();
    const button = screen.getByRole("button", { name: "Settings" });
    button.focus();
    fireEvent.click(button);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(dialog.contains(document.activeElement)).toBe(true);
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    expect(document.activeElement).toBe(button);
  });

  it("applies saved appearance on start", async () => {
    const mock = await setup({
      settings: { appearance: { theme: "dark", fontSize: 15, zoom: 125 } },
    });
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--font-size")).toBe("15px");
    expect(mock.settings.zooms.at(-1)).toBe(1.25);
  });

  it("edits request defaults, appearance and sensitive headers", async () => {
    const mock = await setup();
    ctrl(",");
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.change(within(dialog).getByLabelText("Theme"), { target: { value: "light" } });

    fireEvent.click(within(dialog).getByRole("button", { name: "Requests" }));
    fireEvent.change(within(dialog).getByLabelText("Default timeout"), {
      target: { value: "5000" },
    });
    fireEvent.change(within(dialog).getByLabelText("HTTP version"), { target: { value: "http1" } });
    fireEvent.click(within(dialog).getByLabelText(/Always use a new connection/));

    fireEvent.click(within(dialog).getByRole("button", { name: "Privacy" }));
    fireEvent.input(within(dialog).getByLabelText("Extra sensitive headers"), {
      target: { value: "X-Session\nx-session\n\nX-Csrf" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());

    const s = mock.settings.current();
    expect(s.requests).toMatchObject({
      timeoutMs: 5000,
      httpVersion: "http1",
      freshConnection: true,
    });
    expect(s.appearance.theme).toBe("light");
    expect(s.sensitiveHeaders).toEqual(["X-Session", "X-Csrf"]);
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("keeps the dialog open when the backend rejects a value", async () => {
    const mock = await setup();
    ctrl(",");
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Privacy" }));
    fireEvent.input(within(dialog).getByLabelText("Extra sensitive headers"), {
      target: { value: "bad header" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/isn't a valid header name/)).toBeTruthy();
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
    expect(mock.settings.current().sensitiveHeaders).toEqual([]);
  });

  it("Ctrl +/− step through zoom levels and Ctrl+0 resets; the toggle saves the theme", async () => {
    const mock = await setup();
    ctrl("=");
    await waitFor(() => expect(mock.settings.current().appearance.zoom).toBe(110));
    ctrl("-");
    ctrl("-");
    await waitFor(() => expect(mock.settings.current().appearance.zoom).toBe(90));
    ctrl("0");
    await waitFor(() => expect(mock.settings.current().appearance.zoom).toBe(100));
    expect(mock.settings.zooms.at(-1)).toBe(1);

    fireEvent.click(screen.getByRole("button", { name: "Theme: System" }));
    await waitFor(() => expect(mock.settings.current().appearance.theme).toBe("light"));
  });

  it("quick zoom presses each take a step (no flicker back)", async () => {
    const mock = await setup();
    ctrl("=");
    ctrl("=");
    ctrl("=");
    await waitFor(() => expect(mock.settings.current().appearance.zoom).toBe(150));
    await new Promise((r) => setTimeout(r, 20));
    expect(mock.settings.zooms.at(-1)).toBe(1.5);
  });

  it("About: copies diagnostic info and opens the logs folder", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    const mock = await setup();
    ctrl(",");
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "About & diagnostics" }));
    expect(within(dialog).getByRole("heading", { name: /IronCall \d+\.\d+\.\d+/ })).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Copy diagnostic info" }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(expect.stringContaining("IronCall:")),
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Open logs folder" }));
    await waitFor(() => expect(mock.settings.logsOpened()).toBe(1));
  });

  it("uncaught errors and rejections reach the backend log", async () => {
    const mock = await setup();
    const target = new EventTarget() as unknown as Window;
    reportUncaughtErrors(target);
    const error = new Error("boom");
    target.dispatchEvent(Object.assign(new Event("error"), { message: "boom", error }));
    target.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: "nope" }));
    await waitFor(() => expect(mock.settings.errors).toHaveLength(2));
    expect(mock.settings.errors[0]).toMatchObject({ message: "boom" });
    expect(mock.settings.errors[0]?.stack).toContain("boom");
    expect(mock.settings.errors[1]).toEqual({ message: "nope", stack: null });
  });
});
