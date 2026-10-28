import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { CURL_PREVIEW_MS } from "./components/import/ImportDialog";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi } from "./ipc/mock";
import { EXPORT_PREFS_KEY } from "./model/curl";
import { closeDialog, openDialog } from "./stores/dialogs";
import { dismissImportNotice } from "./stores/importNotice";

const ROOT = "/home/me/demo";

const CURL = `curl -X POST 'https://api.example.com/users?notify=true' -H 'Content-Type: application/json' -d '{"name":"Ada"}' --cert me.pem`;

const SPEC = JSON.stringify({
  openapi: "3.0.3",
  info: { title: "Petstore", description: "A sample API" },
  servers: [{ url: "https://petstore.example.com/v1" }],
  paths: {
    "/pets": {
      get: { summary: "List pets", tags: ["pets"] },
      post: { summary: "Create pet", tags: ["pets"] },
    },
    "/pets/{petId}": { get: { summary: "Show pet", tags: ["pets"] } },
    "/health": { get: { summary: "Health" } },
  },
});

async function setup(options: Parameters<typeof createMockApi>[0] = {}, withWorkspace = true) {
  const mock: MockApi = createMockApi({
    ...(withWorkspace
      ? {
          workspaces: {
            [ROOT]: { name: "Demo", items: [{ folder: "Existing", id: "F1", children: [] }] },
          },
          session: { workspace: ROOT, tabs: [], active: null },
        }
      : {}),
    ...options,
  });
  setApi(mock);
  render(() => <App />);
  const url = (await screen.findByLabelText("URL")) as HTMLInputElement;
  return { mock, url };
}

function paste(el: HTMLElement, text: string) {
  fireEvent.paste(el, { clipboardData: { getData: () => text } });
}

const tabNames = () =>
  screen.getAllByRole("tab").map((t) => t.getAttribute("aria-label") ?? t.textContent);

describe("pasting cURL into the URL bar", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    dismissImportNotice();
    closeDialog();
    setApi(tauriApi);
  });

  it("replaces an empty tab in place, keeping its ID", async () => {
    const { mock, url } = await setup({}, false);
    const before = screen.getAllByRole("tab", { name: /Untitled request/ }).length;
    paste(url, CURL);
    await waitFor(() => expect(url.value).toBe("https://api.example.com/users?notify=true"));
    expect(screen.getByLabelText("Method")).toHaveValue("POST");
    // Same number of tabs: replaced, not added.
    expect(screen.getAllByRole("tab", { name: /POST \/users/ })).toHaveLength(before);
    expect(mock.imports.calls.map((c) => c.op)).toEqual(["curlParse"]);
  });

  it("opens a new tab when the current one already has content", async () => {
    const { url } = await setup({}, false);
    fireEvent.input(url, { target: { value: "https://keep.me/please" } });
    const count = tabNames().length;
    paste(url, CURL);
    await waitFor(() => expect(tabNames()).toHaveLength(count + 1));
    expect(await screen.findByLabelText("URL")).toHaveValue(
      "https://api.example.com/users?notify=true",
    );
    // The original tab is untouched.
    fireEvent.click(screen.getAllByRole("tab", { name: /Untitled request/ })[0] as HTMLElement);
    await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue("https://keep.me/please"));
  });

  it("shows ignored flags in a dismissable notice", async () => {
    const { url } = await setup({}, false);
    paste(url, CURL);
    const notice = await screen.findByRole("status", { name: "cURL import" });
    expect(notice).toHaveTextContent("Imported from cURL; ignored: --cert");
    fireEvent.click(within(notice).getByRole("button", { name: "Dismiss import notice" }));
    await waitFor(() => expect(screen.queryByRole("status", { name: "cURL import" })).toBeNull());
  });

  it("undo restores the request from before the paste", async () => {
    const { url } = await setup({}, false);
    paste(url, CURL);
    await waitFor(() => expect(url.value).toContain("api.example.com"));
    fireEvent.keyDown(url, { key: "z", ctrlKey: true });
    await waitFor(() => expect((screen.getByLabelText("URL") as HTMLInputElement).value).toBe(""));
    expect(screen.getByLabelText("Method")).toHaveValue("GET");
  });

  it("pastes normally when the text isn't a cURL command", async () => {
    const { mock, url } = await setup({}, false);
    paste(url, "https://plain.example.com");
    // Not prevented: the browser inserts it (jsdom doesn't, so just check no import ran).
    await new Promise((r) => setTimeout(r, 20));
    expect(mock.imports.calls).toEqual([]);
  });
});

describe("import dialog", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("is reachable from the sidebar", async () => {
    await setup();
    fireEvent.click(await screen.findByRole("button", { name: "Import" }));
    expect(await screen.findByRole("dialog", { name: "Import" })).toBeInTheDocument();
  });

  it("cURL tab previews live (debounced) and opens a new tab", async () => {
    const { mock } = await setup();
    openDialog({ kind: "import", tab: "curl" });
    const box = await screen.findByLabelText("cURL command");
    fireEvent.input(box, { target: { value: "curl https://a.io" } });
    fireEvent.input(box, { target: { value: CURL } });
    expect(mock.imports.calls).toHaveLength(0);
    const preview = await screen.findByRole("region", { name: "cURL preview" });
    expect(preview).toHaveTextContent("https://api.example.com/users");
    expect(preview).toHaveTextContent("1 header");
    expect(preview).toHaveTextContent("1 warning");
    // Two inputs within the debounce window: one parse.
    expect(mock.imports.calls.filter((c) => c.op === "curlParse")).toHaveLength(1);
    expect(CURL_PREVIEW_MS).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole("button", { name: "Open in new tab" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Import" })).toBeNull());
    expect(await screen.findByLabelText("URL")).toHaveValue(
      "https://api.example.com/users?notify=true",
    );
  });

  it("OpenAPI: preview tree, options and import", async () => {
    const { mock } = await setup({ import: { pickedSpec: SPEC } });
    openDialog({ kind: "import", tab: "openapi" });
    fireEvent.click(await screen.findByRole("button", { name: "Choose file…" }));
    const preview = await screen.findByRole("region", { name: "Import preview" });
    expect(preview).toHaveTextContent("Petstore");
    expect(preview).toHaveTextContent("OpenAPI 3.0");
    expect(preview).toHaveTextContent("4 requests in 1 folder");
    expect(preview).toHaveTextContent("https://petstore.example.com/v1");
    const list = within(preview).getByRole("list", { name: "Requests to import" });
    expect(list).toHaveTextContent("pets");
    expect(list).toHaveTextContent("Show pet");
    expect(list).toHaveTextContent("{{baseUrl}}/pets/:petId");

    // Import into the existing folder, without wrapping or an environment.
    fireEvent.change(screen.getByLabelText("Target folder"), { target: { value: "F1" } });
    fireEvent.click(screen.getByLabelText(/Put in a new folder/));
    fireEvent.click(screen.getByLabelText(/Create environment/));
    expect(screen.getByLabelText("Make it active")).toBeDisabled();
    const dialog = screen.getByRole("dialog", { name: "Import" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Import" })).toBeNull());
    const call = mock.imports.calls.find((c) => c.op === "openapiImport");
    expect(call?.args[0]).toMatchObject({
      parent: "F1",
      wrapInFolder: false,
      createEnvironment: false,
      activateEnvironment: false,
    });
    expect(await screen.findByText(/Imported 4 requests in 1 folder/)).toBeInTheDocument();
    const tree = screen.getByRole("tree", { name: "Workspace" });
    expect(await within(tree).findByRole("treeitem", { name: "pets" })).toBeInTheDocument();
  });

  it("OpenAPI: wraps in a new folder, creates and activates an environment by default", async () => {
    const { mock } = await setup();
    openDialog({ kind: "import", tab: "openapi" });
    fireEvent.click(await screen.findByText("Or paste the spec"));
    fireEvent.input(screen.getByLabelText("Spec text"), { target: { value: SPEC } });
    const dialog = screen.getByRole("dialog", { name: "Import" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Preview" }));
    await screen.findByRole("region", { name: "Import preview" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));
    await screen.findByText(/plus an environment/);
    const call = mock.imports.calls.find((c) => c.op === "openapiImport");
    expect(call?.args[0]).toMatchObject({
      wrapInFolder: true,
      createEnvironment: true,
      activateEnvironment: true,
    });
    const tree = screen.getByRole("tree", { name: "Workspace" });
    const folder = await within(tree).findByRole("treeitem", { name: "Petstore" });
    // Expanded and selected after import.
    expect(folder).toHaveAttribute("aria-expanded", "true");
    expect(folder).toHaveAttribute("aria-selected", "true");
    expect(mock.vars.calls.some((c) => c.op === "activeEnvironmentSet")).toBe(true);
  });

  it("OpenAPI tab asks for a workspace first", async () => {
    await setup({}, false);
    openDialog({ kind: "import", tab: "openapi" });
    expect(await screen.findByText(/Open or create a workspace first/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Choose file…" })).toBeNull();
  });
});

describe("Copy as cURL", () => {
  let writeText: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    localStorage.clear();
    writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  });
  afterEach(() => {
    closeDialog();
    vi.restoreAllMocks();
    setApi(tauriApi);
  });

  async function openCopy() {
    const setupResult = await setup({}, false);
    fireEvent.input(setupResult.url, { target: { value: "https://x.io/users" } });
    const tab = screen.getAllByRole("tab", { name: /Untitled request/ })[0] as HTMLElement;
    fireEvent.contextMenu(tab);
    fireEvent.click(await screen.findByRole("menuitem", { name: /Copy as cURL/ }));
    await screen.findByRole("dialog", { name: "Copy as cURL" });
    return setupResult;
  }

  it("exports masked by default with bash off Windows, and copies", async () => {
    const { mock } = await openCopy();
    const box = (await screen.findByLabelText("cURL command")) as HTMLTextAreaElement;
    expect(box.value).toContain("curl");
    expect(box.value).toContain("'https://x.io/users'");
    expect(screen.getByLabelText("bash")).toBeChecked();
    expect(screen.getByLabelText(/Resolved, secrets masked/)).toBeChecked();
    expect(mock.imports.calls.at(-1)?.args.slice(1)).toEqual(["posix", "masked"]);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(box.value));
  });

  it("defaults to PowerShell on Windows", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
    );
    const { mock } = await openCopy();
    expect(await screen.findByLabelText("PowerShell")).toBeChecked();
    await waitFor(() => expect(mock.imports.calls.at(-1)?.args[1]).toBe("powerShell"));
    await waitFor(() =>
      expect((screen.getByLabelText("cURL command") as HTMLTextAreaElement).value).toContain(
        "curl.exe",
      ),
    );
  });

  it("requires confirmation before including secrets, and doesn't remember that choice", async () => {
    const { mock } = await openCopy();
    await screen.findByLabelText("cURL command");
    const exportsBefore = mock.imports.calls.length;
    fireEvent.click(screen.getByLabelText(/Resolved with secrets/));
    // Nothing is exported (and no secret fetched) until confirmed.
    await new Promise((r) => setTimeout(r, 20));
    expect(mock.imports.calls).toHaveLength(exportsBefore);
    expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();

    fireEvent.click(screen.getByLabelText("This copies secret values to the clipboard"));
    await waitFor(() => expect(mock.imports.calls.at(-1)?.args[2]).toBe("resolved"));
    expect(localStorage.getItem(EXPORT_PREFS_KEY) ?? "").not.toContain("resolved");
  });

  it("remembers the dialect and a safe mode for next time", async () => {
    await openCopy();
    fireEvent.click(await screen.findByLabelText("PowerShell"));
    fireEvent.click(screen.getByLabelText(/With variables/));
    expect(JSON.parse(localStorage.getItem(EXPORT_PREFS_KEY) ?? "{}")).toEqual({
      dialect: "powerShell",
      mode: "variables",
    });
  });

  it("is offered from the response's Request tab", async () => {
    const { url } = await setup({}, false);
    fireEvent.input(url, { target: { value: "https://x.io/sent" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByTestId("status");
    const response = screen.getByRole("region", { name: "Response" });
    fireEvent.click(within(response).getByRole("tab", { name: "Request" }));
    fireEvent.click(await within(response).findByRole("button", { name: "Copy as cURL…" }));
    const box = (await screen.findByLabelText("cURL command")) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toContain("https://x.io/sent"));
  });

  it("is offered from the tree's request menu too", async () => {
    await setup({
      workspaces: {
        [ROOT]: {
          name: "Demo",
          items: [{ request: "List users", id: "R1", url: "https://x.io/u" }],
        },
      },
      session: { workspace: ROOT, tabs: [], active: null },
    });
    const tree = await screen.findByRole("tree", { name: "Workspace" });
    fireEvent.contextMenu(within(tree).getByRole("treeitem", { name: "List users" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: /Copy as cURL/ }));
    const box = (await screen.findByLabelText("cURL command")) as HTMLTextAreaElement;
    expect(box.value).toContain("https://x.io/u");
  });
});
