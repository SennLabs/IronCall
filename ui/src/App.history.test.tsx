import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, mockResponse } from "./ipc/mock";

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock = createMockApi(options);
  setApi(mock);
  render(() => <App />);
  const url = (await screen.findByLabelText("URL")) as HTMLInputElement;
  return { mock, url };
}

describe("App: Phase 5", () => {
  afterEach(() => setApi(tauriApi));

  it("shows Timing, TLS and Timeline tabs for a response", async () => {
    const { url } = await setup();
    fireEvent.input(url, { target: { value: "https://api.example.com/users" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByTestId("status");
    fireEvent.click(screen.getByRole("tab", { name: "Timing" }));
    expect(screen.getByText(/Total 63 ms/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "TLS" }));
    expect(screen.getByText("TLS13_AES_128_GCM_SHA256")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Timeline" }));
    expect(screen.getByRole("list", { name: "Timeline" })).toBeInTheDocument();
  });

  it("offers a Preview tab for images", async () => {
    const { url } = await setup({
      respond: (req) => {
        const r = mockResponse(req, "PNGDATA");
        return {
          ...r,
          body: { ...r.body, kind: "binary", contentType: "image/png", prettyAvailable: false },
        };
      },
    });
    fireEvent.input(url, { target: { value: "https://api.example.com/logo.png" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByTestId("status");
    fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
    const img = screen.getByAltText("Response body") as HTMLImageElement;
    expect(img.getAttribute("src")).toMatch(/ironcall(:\/\/localhost|\.localhost)\/body\/\d+$/);
  });

  it("records sends in history; Ctrl+H opens it and an entry reopens in a new tab", async () => {
    const { url } = await setup();
    fireEvent.input(url, { target: { value: "https://api.example.com/users/{{id}}" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByTestId("status");

    fireEvent.keyDown(window, { key: "h", ctrlKey: true });
    const panel = await screen.findByRole("complementary", { name: "History" });
    const list = within(panel).getByRole("listbox", { name: "History entries" });
    const option = await within(list).findByRole("option");
    expect(option).toHaveTextContent("api.example.com/users");
    fireEvent.click(option);
    const detail = await within(panel).findByRole("region", { name: "History entry" });
    const tabsBefore = screen.getAllByRole("tab", { name: /Untitled request/ }).length;
    fireEvent.click(within(detail).getByRole("button", { name: "Open in new tab" }));
    await waitFor(() =>
      expect(screen.getAllByRole("tab", { name: /Untitled request/ }).length).toBe(tabsBefore + 1),
    );
    // The new tab holds the authored URL, variables and all.
    expect((screen.getByLabelText("URL") as HTMLInputElement).value).toBe(
      "https://api.example.com/users/{{id}}",
    );

    // A new send shows up at the top while the panel is open.
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(2));
  });
});
