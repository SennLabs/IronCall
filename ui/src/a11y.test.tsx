/**
 * Accessibility audit (PLAN.md Phase 8): axe-core over the main screen and every dialog.
 * Colour contrast is checked separately against the theme tokens (`theme/contrast.test.ts`),
 * because jsdom has no layout or computed colours.
 */
import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, mockResponse } from "./ipc/mock";
import { closeDialog, type DialogRequest, openDialog } from "./stores/dialogs";

const ROOT = "/home/me/demo";

async function audit(context: Element = document.body) {
  const result = await axe.run(context, {
    rules: {
      "color-contrast": { enabled: false },
      // Landmarks are checked by role in the tests; jsdom renders portals outside them.
      region: { enabled: false },
    },
  });
  return result.violations.map(
    (v) =>
      `${v.id}: ${v.help} (${v.nodes
        .map((n) => n.target.join(" "))
        .slice(0, 3)
        .join(" | ")})`,
  );
}

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
    respond: (r) =>
      mockResponse(r, '{"ok":true}', {
        checks: {
          assertions: [
            {
              index: 0,
              passed: false,
              label: "status eq 201",
              actual: "200",
              message: "expected eq 201",
            },
          ],
          extracted: [],
        },
      }),
  });
  setApi(mock);
  render(() => <App />);
  await screen.findByRole("tree", { name: "Workspace" });
  return mock;
}

describe("accessibility (axe)", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("main screen with a request and a response", async () => {
    await setup();
    fireEvent.dblClick(screen.getByRole("treeitem", { name: "Health" }));
    await waitFor(() => expect(screen.getByLabelText("URL")).toHaveValue("https://x.io/health"));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    fireEvent.click(await screen.findByRole("tab", { name: /^Tests/ }));
    expect(await audit()).toEqual([]);
  });

  const dialogs: [string, DialogRequest][] = [
    ["Settings", { kind: "settings" }],
    ["Go to request", { kind: "quickOpen" }],
    ["Run: Demo", { kind: "runner" }],
    ["Environments", { kind: "environments" }],
    ["Cookies", { kind: "cookies" }],
    ["Import", { kind: "import" }],
    ["Network", { kind: "network" }],
    ["History", { kind: "history" }],
    ["Secrets", { kind: "secrets" }],
    ["Collection variables", { kind: "variables", target: { kind: "collection" } }],
    ["Copy as cURL", { kind: "copyCurl", source: { requestId: "R2" } }],
  ];
  for (const [name, request] of dialogs) {
    it(`dialog: ${name}`, async () => {
      const mock = await setup();
      if (request.kind === "runner") await mock.runItems({ folder: null });
      openDialog(request);
      const dialog = await screen.findByRole("dialog");
      await waitFor(() => expect(within(dialog).queryByText(/Loading|Starting/)).toBeNull());
      expect(await audit(dialog)).toEqual([]);
    });
  }

  it("runner results with details open", async () => {
    await setup();
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    const dialog = await screen.findByRole("dialog", { name: "Run: Demo" });
    await within(dialog).findByText("2 requests, in order.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    await within(dialog).findByLabelText("Run summary");
    fireEvent.click(within(dialog).getByRole("button", { name: "Details of Health" }));
    expect(await audit(dialog)).toEqual([]);
  });

  it("first run: welcome screen and empty sidebar", async () => {
    setApi(createMockApi());
    render(() => <App />);
    await screen.findByRole("button", { name: "Try the sample workspace…" });
    // Close the startup scratch tab so the welcome screen shows.
    fireEvent.keyDown(window, { key: "w", ctrlKey: true });
    await screen.findByRole("region", { name: "Getting started" });
    expect(await audit()).toEqual([]);
  });

  it("every Settings section", async () => {
    await setup();
    openDialog({ kind: "settings" });
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    for (const section of [
      "Appearance",
      "Requests",
      "Privacy",
      "Network & storage",
      "About & diagnostics",
    ]) {
      fireEvent.click(within(dialog).getByRole("button", { name: section }));
      expect(await audit(dialog), section).toEqual([]);
    }
  });
});
