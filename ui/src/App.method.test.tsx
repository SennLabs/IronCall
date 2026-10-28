import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "./App";
import { isMethodToken } from "./components/request/RequestBar";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi } from "./ipc/mock";

const ROOT = "/home/me/demo";

async function setup() {
  const mock = createMockApi({
    workspaces: {
      [ROOT]: {
        name: "Demo",
        items: [
          { request: "Health", id: "R1", url: "https://x.io/health" },
          { request: "Lock", id: "R2", method: "LOCK", url: "https://x.io/dav" },
        ],
      },
    },
    session: { workspace: ROOT, tabs: [], active: null },
  });
  setApi(mock);
  render(() => <App />);
  const tree = await screen.findByRole("tree", { name: "Workspace" });
  return { mock, tree };
}

async function open(tree: HTMLElement, name: string, url: string) {
  fireEvent.dblClick(within(tree).getByRole("treeitem", { name }));
  await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue(url));
  return screen.getByRole("combobox", { name: "Method" }) as HTMLSelectElement;
}

const options = (select: HTMLSelectElement) => [...select.options].map((o) => o.textContent);

describe("method dropdown", () => {
  afterEach(() => setApi(tauriApi));

  it("lists the common methods and sends the chosen one", async () => {
    const { tree, mock } = await setup();
    const method = await open(tree, "Health", "https://x.io/health");
    expect(options(method)).toEqual([
      "GET",
      "POST",
      "PUT",
      "PATCH",
      "DELETE",
      "HEAD",
      "OPTIONS",
      "Custom…",
    ]);
    expect(method.value).toBe("GET");
    fireEvent.change(method, { target: { value: "PATCH" } });
    expect(method).toHaveAttribute("data-method", "patch");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(mock.history.entries()[0]?.summary.method).toBe("PATCH"));
  });

  it("shows a request's custom method", async () => {
    const { tree } = await setup();
    const method = await open(tree, "Lock", "https://x.io/dav");
    expect(method.value).toBe("LOCK");
    expect(options(method)).toContain("LOCK");
    expect(method).toHaveAttribute("data-method", "other");
  });

  it("Custom… asks for a method, upper-cases it and rejects invalid ones", async () => {
    const { tree } = await setup();
    const method = await open(tree, "Health", "https://x.io/health");
    fireEvent.change(method, { target: { value: " custom" } });
    let dialog = await screen.findByRole("dialog", { name: "Custom method" });
    expect(method.value).toBe("GET");
    fireEvent.input(within(dialog).getByRole("textbox"), { target: { value: "propfind" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Use method" }));
    await waitFor(() => expect(method.value).toBe("PROPFIND"));

    fireEvent.change(method, { target: { value: " custom" } });
    dialog = await screen.findByRole("dialog", { name: "Custom method" });
    expect(within(dialog).getByRole("textbox")).toHaveValue("PROPFIND");
    fireEvent.input(within(dialog).getByRole("textbox"), { target: { value: "BAD METHOD" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Use method" }));
    expect(await screen.findByText(/isn't a valid HTTP method/)).toBeTruthy();
    expect(method.value).toBe("PROPFIND");
  });

  it("validates method tokens", () => {
    for (const ok of ["GET", "PROPFIND", "M-SEARCH", "X_CUSTOM"])
      expect(isMethodToken(ok)).toBe(true);
    for (const bad of ["", "BAD METHOD", "GET/1", "é"]) expect(isMethodToken(bad)).toBe(false);
  });

  it("the scratch list follows an open scratch request's method and URL", async () => {
    await setup();
    const scratch = await screen.findByRole("region", { name: "Scratch requests" });
    const method = screen.getByRole("combobox", { name: "Method" });
    fireEvent.change(method, { target: { value: "DELETE" } });
    fireEvent.input(screen.getByLabelText("URL"), { target: { value: "https://x.io/items/1" } });
    await waitFor(() => expect(within(scratch).getByText("https://x.io/items/1")).toBeTruthy());
    expect(within(scratch).getByText("DEL")).toBeTruthy();
  });
});
