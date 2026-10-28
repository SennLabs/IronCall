import { fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { SendFailure, sendError, setApi, tauriApi } from "./ipc/api";
import { createMockApi, mockResponse } from "./ipc/mock";

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock = createMockApi(options);
  setApi(mock);
  render(() => <App />);
  const url = (await screen.findByLabelText("URL")) as HTMLInputElement;
  return { mock, url };
}

describe("App", () => {
  afterEach(() => setApi(tauriApi));

  it("sends and shows the response status, time and size", async () => {
    const { url } = await setup();
    fireEvent.input(url, { target: { value: "https://api.example.com/users?page=2" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByTestId("status")).toHaveTextContent("200 OK");
    expect(screen.getByText("63 ms")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Headers \(1\)/ })).toBeInTheDocument();
  });

  it("typing a query in the URL fills the params table", async () => {
    const { url } = await setup();
    fireEvent.input(url, { target: { value: "https://x.io/:id?a=1&b=2" } });
    expect(screen.getByRole("tab", { name: "Params (2)" })).toBeInTheDocument();
    const table = screen.getByRole("table", { name: "Query parameters" });
    expect(table.querySelectorAll("tbody tr")).toHaveLength(3); // 2 rows + add row
    expect(screen.getByRole("table", { name: "Path parameters" })).toBeInTheDocument();
  });

  it("Ctrl+Enter sends", async () => {
    const { url } = await setup();
    fireEvent.input(url, { target: { value: "https://x.io" } });
    fireEvent.keyDown(window, { key: "Enter", ctrlKey: true });
    expect(await screen.findByTestId("status")).toHaveTextContent("200");
  });

  it.each([
    ["dns", "Host not found"],
    ["connect", "Could not connect"],
    ["tls", "TLS / certificate error"],
    ["timeout", "Timed out"],
  ] as const)("shows %s errors with a hint", async (kind, title) => {
    await setup({
      respond: () => new SendFailure(sendError(kind, `boom: ${kind}`, "cause chain here")),
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(title);
    expect(alert).toHaveTextContent(`boom: ${kind}`);
    expect(alert).toHaveAttribute("data-kind", kind);
    expect(screen.getByText("Details")).toBeInTheDocument();
  });

  it("cancel turns the Send button back and shows Cancelled", async () => {
    await setup({ latencyMs: 50 });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const [cancel] = await screen.findAllByRole("button", { name: "Cancel" });
    expect(screen.getByRole("status")).toHaveTextContent("Connecting");
    fireEvent.click(cancel as HTMLElement);
    expect(await screen.findByRole("alert")).toHaveTextContent("Cancelled");
    await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument());
  });

  it("shows an empty state for bodiless responses", async () => {
    await setup({
      respond: (req) => mockResponse(req, "", { status: 204, statusText: "No Content" }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText(/No body \(204 No Content\)/)).toBeInTheDocument();
  });

  it("shows auto headers from the preview", async () => {
    await setup();
    fireEvent.click(screen.getByRole("tab", { name: /Headers/ }));
    expect(await screen.findByText("User-Agent", {}, { timeout: 2000 })).toBeInTheDocument();
  });

  it("large bodies use the paged viewer with search", async () => {
    const text = Array.from({ length: 50 }, (_, i) => `line ${i} ${i === 30 ? "needle" : ""}`).join(
      "\n",
    );
    await setup({
      respond: (req) => {
        const r = mockResponse(req, text);
        // Plain text: shown raw in the paged viewer (JSON would be pretty-printed first).
        return {
          ...r,
          body: { ...r.body, kind: "text", size: 3 * 1024 * 1024, prettyAvailable: false },
        };
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const search = await screen.findByRole("searchbox", { name: "Search body" });
    expect(screen.getByText(/50 lines · raw/)).toBeInTheDocument();
    fireEvent.input(search, { target: { value: "needle" } });
    // Searches as you type (debounced), in Rust.
    expect(await screen.findByText("Line 31")).toBeInTheDocument();
  });

  it("small bodies toggle between pretty and raw", async () => {
    await setup({ respond: (req) => mockResponse(req, '{"a":1}') });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const raw = await screen.findByRole("button", { name: "Raw" });
    expect(screen.getByRole("button", { name: "Pretty" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(raw);
    expect(raw).toHaveAttribute("aria-pressed", "true");
  });
});
