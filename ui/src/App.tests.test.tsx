import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import {
  type RequestDraft,
  type ResponseChecks,
  SendFailure,
  sendError,
  setApi,
  tauriApi,
} from "./ipc/api";
import { createMockApi, type MockApi, mockResponse } from "./ipc/mock";
import { closeDialog } from "./stores/dialogs";

const ROOT = "/home/me/demo";

const pass = (label: string) => ({ index: 0, passed: true, label, actual: "200", message: null });
const fail = (label: string) => ({
  index: 0,
  passed: false,
  label,
  actual: "401",
  message: "expected eq 200",
});

/** Responds per URL: `/login` extracts a token, `/fail` fails an assertion, `/down` errors. */
function respond(request: RequestDraft) {
  const checks = (c: ResponseChecks) => mockResponse(request, "{}", { checks: c });
  if (request.url.endsWith("/login")) {
    return checks({
      assertions: [pass("status eq 200")],
      extracted: [{ var: "authToken", value: "••••", sensitive: true, error: null }],
    });
  }
  if (request.url.endsWith("/fail")) {
    return checks({ assertions: [fail("status eq 200")], extracted: [] });
  }
  if (request.url.endsWith("/down")) {
    return new SendFailure(sendError("connect", "Connection refused"));
  }
  return mockResponse(request, "{}");
}

async function setup(options: Parameters<typeof createMockApi>[0] = {}) {
  const mock: MockApi = createMockApi({
    workspaces: {
      [ROOT]: {
        name: "Demo",
        items: [
          {
            folder: "Auth",
            id: "F1",
            children: [{ request: "Login", id: "R1", method: "POST", url: "https://x.io/login" }],
          },
          { request: "Profile", id: "R2", url: "https://x.io/fail" },
          { request: "Health", id: "R3", url: "https://x.io/down" },
        ],
      },
    },
    session: { workspace: ROOT, tabs: [], active: null },
    respond,
    ...options,
  });
  setApi(mock);
  render(() => <App />);
  const tree = await screen.findByRole("tree", { name: "Workspace" });
  return { mock, tree };
}

const item = (tree: HTMLElement, name: string) => within(tree).getByRole("treeitem", { name });

/** Opens a request from the tree and waits until its tab shows. */
async function open(tree: HTMLElement, name: string, url: string) {
  if (name === "Login") fireEvent.click(item(tree, "Auth"));
  fireEvent.dblClick(await within(tree).findByRole("treeitem", { name }));
  await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue(url));
}

async function openTab(name: RegExp) {
  fireEvent.click(await screen.findByRole("tab", { name }));
}

describe("Tests tab (assertions and extractions)", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("edits typed assertions and extractions in the draft", async () => {
    const { tree, mock } = await setup();
    await open(tree, "Profile", "https://x.io/fail");
    await openTab(/^Tests/);
    fireEvent.click(screen.getByRole("button", { name: "Add assertion" }));
    const table = screen.getByRole("table", { name: "Assertions" });
    expect(within(table).getByLabelText("Expected value of assertion 1")).toHaveValue("200");

    fireEvent.change(within(table).getByLabelText("Target of assertion 1"), {
      target: { value: "body" },
    });
    fireEvent.input(within(table).getByLabelText("JSONPath of assertion 1"), {
      target: { value: "$.id" },
    });
    fireEvent.input(within(table).getByLabelText("Expected value of assertion 1"), {
      target: { value: '"42"' },
    });
    fireEvent.change(within(table).getByLabelText("Check of assertion 1"), {
      target: { value: "exists" },
    });
    expect(within(table).queryByLabelText("Expected value of assertion 1")).toBeNull();
    expect(screen.getByRole("tab", { name: /^Tests \(1\)/ })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Add extraction" }));
    const ex = screen.getByRole("table", { name: "Extractions" });
    fireEvent.input(within(ex).getByLabelText("Variable of extraction 1"), {
      target: { value: "authToken" },
    });
    const sensitive = within(ex).getByLabelText("Sensitive extraction 1") as HTMLInputElement;
    expect(sensitive.checked && sensitive.disabled).toBe(true);

    // Saved as typed values.
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(mock.disk.calls).toContain("requestSave"));
    const saved = (await mock.requestLoad("R2")).request;
    expect(saved.assert).toEqual([{ target: "body", op: "exists", enabled: true, path: "$.id" }]);
    expect(saved.extract).toEqual([
      { var: "authToken", from: "body", path: "$.", sensitive: false },
    ]);
  });

  it("the Expected field keeps what you type: lists, decimals", async () => {
    const { tree } = await setup();
    await open(tree, "Profile", "https://x.io/fail");
    await openTab(/^Tests/);
    fireEvent.click(screen.getByRole("button", { name: "Add assertion" }));
    const table = screen.getByRole("table", { name: "Assertions" });
    fireEvent.change(within(table).getByLabelText("Check of assertion 1"), {
      target: { value: "in" },
    });
    const expected = within(table).getByLabelText(
      "Expected value of assertion 1",
    ) as HTMLInputElement;
    for (const text of ["2", "200", "200,", "200, ", "200, 2", "200, 201"]) {
      fireEvent.input(expected, { target: { value: text } });
      expect(expected.value).toBe(text);
    }
    fireEvent.blur(expected);
    expect(expected.value).toBe("[200,201]");
    fireEvent.change(within(table).getByLabelText("Check of assertion 1"), {
      target: { value: "eq" },
    });
    for (const text of ["1", "1.", "1.0", "1.05"]) {
      fireEvent.input(expected, { target: { value: text } });
      expect(expected.value).toBe(text);
    }
  });

  it("shows results in the response and makes extracted values runtime variables", async () => {
    const { tree, mock } = await setup();
    await open(tree, "Login", "https://x.io/login");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await openTab(/^Tests 1\/1/);
    expect(screen.getByRole("status")).toHaveTextContent("1 of 1 assertions passed");
    expect(screen.getByLabelText("Extracted authToken")).toHaveTextContent("authToken = ••••");
    expect(mock.run.runtime()).toEqual([{ name: "authToken", value: "••••", sensitive: true }]);
  });

  it("marks a failing response", async () => {
    const { tree } = await setup();
    await open(tree, "Profile", "https://x.io/fail");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await openTab(/^Tests 0\/1 ✗/);
    expect(screen.getByLabelText("Failed: status eq 200")).toHaveTextContent("expected eq 200");
    expect(screen.getByLabelText("Failed: status eq 200")).toHaveTextContent("Actual: 401");
  });
});

describe("collection runner", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("runs the workspace in order, shows results and drills into history", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    await setup();
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    const dialog = await screen.findByRole("dialog", { name: "Run: Demo" });
    const results = await within(dialog).findByRole("table", { name: "Run results" });
    await waitFor(() => expect(within(results).getAllByRole("row")).toHaveLength(4));
    expect(within(dialog).getByText("3 requests, in order.")).toBeTruthy();

    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    await within(dialog).findByLabelText("Run summary");
    expect(within(dialog).getByLabelText("Run summary")).toHaveTextContent(
      /1 passed, 1 failed, 1 errors/,
    );
    expect(within(results).getByRole("row", { name: "Login Passed" })).toHaveTextContent(
      "Auth / Login",
    );
    within(results).getByRole("row", { name: "Profile Failed" });
    within(results).getByRole("row", { name: "Health Error" });

    fireEvent.click(within(results).getByRole("button", { name: "Details of Health" }));
    expect(within(results).getByText("Connection refused")).toBeTruthy();

    fireEvent.click(within(dialog).getByRole("button", { name: "Copy results as Markdown" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0]?.[0]).toContain("## Run: Demo");

    fireEvent.click(within(results).getByRole("button", { name: "Details of Profile" }));
    fireEvent.click(
      within(results).getAllByRole("button", { name: "Open in history" })[0] as HTMLElement,
    );
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Run: Demo" })).toBeNull());
    expect(await screen.findByRole("complementary", { name: "History" })).toBeTruthy();
  });

  it("runs a folder from the context menu, with options", async () => {
    const { tree, mock } = await setup();
    const spy = vi.spyOn(mock, "runCollection");
    fireEvent.contextMenu(item(tree, "Auth"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Run folder…" }));
    const dialog = await screen.findByRole("dialog", { name: "Run: Auth" });
    await within(dialog).findByText("1 request, in order.");
    fireEvent.change(within(dialog).getByLabelText("Iterations"), { target: { value: "2" } });
    fireEvent.click(within(dialog).getByLabelText("Stop on first failure"));
    fireEvent.change(within(dialog).getByLabelText("Run environment"), {
      target: { value: "none" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    await within(dialog).findByLabelText("Run summary");
    expect(spy).toHaveBeenCalledWith(
      expect.any(String),
      { folder: "F1" },
      { kind: "none" },
      { iterations: 2, delayMs: 0, stopOnFailure: true, parallel: false },
    );
    const results = within(dialog).getByRole("table", { name: "Run results" });
    const rows = within(results).getAllByRole("row", { name: "Login Passed" });
    expect(rows.map((r) => r.firstElementChild?.textContent)).toEqual(["1.1", "2.1"]);
    expect(within(dialog).getByLabelText("Run summary")).toHaveTextContent(/2 passed/);
  });

  it("cancels a run", async () => {
    const { mock } = await setup({ latencyMs: 30 });
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    const dialog = await screen.findByRole("dialog", { name: "Run: Demo" });
    await within(dialog).findByText("3 requests, in order.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    fireEvent.click(await within(dialog).findByRole("button", { name: "Cancel run" }));
    await within(dialog).findByLabelText("Run summary");
    expect(within(dialog).getByLabelText("Run summary")).toHaveTextContent("(cancelled)");
    expect(mock.run.runtime()).toEqual([]);
  });
});

describe("runtime variables in the quick look", () => {
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("saves to the active environment and clears", async () => {
    const { tree, mock } = await setup({
      vars: {
        environments: {
          [ROOT]: [{ schema: "ironcall/environment@1", id: "E1", name: "Dev", vars: {} }],
        },
        active: { [ROOT]: "E1" },
      },
    });
    // The mock's variablesFor doesn't know runtime variables; add one through it.
    const base = mock.variablesFor.bind(mock);
    mock.variablesFor = async (r, reveal) => {
      const ctx = await base(r, reveal);
      const names = mock.run.runtime().map((v) => v.name);
      return {
        ...ctx,
        variables: [
          ...ctx.variables,
          ...names.map((name) => ({
            name,
            value: null,
            raw: null,
            secret: true,
            missing: false,
            source: { scope: "runtime" as const },
            overrides: [],
            error: null,
            description: null,
          })),
        ],
      };
    };
    mock.run.setRuntime([{ name: "authToken", value: "t", sensitive: true }]);
    await open(tree, "Login", "https://x.io/login");
    fireEvent.click(screen.getByRole("button", { name: "Variables quick look" }));
    const look = await screen.findByRole("dialog", { name: "Variables" });
    fireEvent.click(await within(look).findByRole("button", { name: "Save authToken to Dev" }));
    await waitFor(() => expect(mock.run.saved).toEqual(["authToken"]));
    fireEvent.click(within(look).getByRole("button", { name: "Clear authToken" }));
    await waitFor(() => expect(mock.run.runtime()).toEqual([]));
  });
});
