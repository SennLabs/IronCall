import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi, type MockVarsSeed } from "./ipc/mock";

const ROOT = "/home/me/demo";

const DEV = {
  schema: "ironcall/environment@1",
  id: "E1",
  name: "Dev",
  vars: { baseUrl: "https://dev.example.com", apiKey: { secret: true } },
};
const PROD = {
  schema: "ironcall/environment@1",
  id: "E2",
  name: "Prod",
  vars: { baseUrl: "https://prod.example.com", token: { secret: true } },
};

async function setup(vars: MockVarsSeed = {}, tabs = ["R1"]) {
  const mock: MockApi = createMockApi({
    workspaces: {
      [ROOT]: {
        name: "Demo",
        vars: { baseUrl: "https://collection.example.com" },
        items: [
          { request: "Users", id: "R1", url: "{{baseUrl}}/users" },
          { request: "Token", id: "R2", url: "{{baseUrl}}/t?token={{token}}" },
        ],
      },
    },
    session: {
      workspace: ROOT,
      tabs: tabs.map((id) => ({ kind: "request" as const, id })),
      active: 0,
    },
    vars: {
      environments: { [ROOT]: [DEV, PROD] },
      secrets: { "environment:E1/apiKey": "k-dev" },
      ...vars,
    },
  });
  setApi(mock);
  render(() => <App />);
  await screen.findByRole("tree", { name: "Workspace" });
  return mock;
}

const envSelect = () => screen.getByLabelText("Environment") as HTMLSelectElement;
const url = () => screen.getByLabelText("URL") as HTMLInputElement;

async function selectEnv(id: string) {
  await waitFor(() => expect(within(envSelect()).queryByText("Dev")).not.toBeNull());
  fireEvent.change(envSelect(), { target: { value: id } });
}

/** Answers the in-app prompt (secrets, passphrases). */
async function answer(title: string, ...values: string[]) {
  const dialog = await screen.findByRole("dialog", { name: title });
  const inputs = dialog.querySelectorAll<HTMLInputElement>("input");
  values.forEach((v, i) => {
    const input = inputs[i];
    if (input) fireEvent.input(input, { target: { value: v } });
  });
  const ok = within(dialog)
    .getAllByRole("button")
    .find((b) => b.getAttribute("type") === "submit");
  if (!ok) throw new Error("no submit button");
  fireEvent.click(ok);
}

describe("variables and environments", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => setApi(tauriApi));

  it("switches environment, lists missing counts, and persists the choice", async () => {
    const mock = await setup();
    await waitFor(() => expect(within(envSelect()).getByText("Prod (1 missing)")).toBeDefined());
    await selectEnv("E1");
    await waitFor(() => expect(envSelect().value).toBe("E1"));
    expect(mock.vars.calls.some((c) => c.op === "activeEnvironmentSet" && c.args[0] === "E1")).toBe(
      true,
    );
    expect(await mock.activeEnvironmentGet()).toBe("E1");
  });

  it("preview resolves variables and masks secrets until revealed", async () => {
    const mock = await setup();
    await selectEnv("E1");
    // Add a header that uses the secret.
    const tab = await mock.requestLoad("R1");
    mock.disk.externalEdit("R1", {
      ...tab.request,
      headers: [{ name: "X-Key", value: "{{apiKey}}", enabled: true }],
    });
    await waitFor(() => expect(screen.getByRole("tab", { name: /Headers \(1\)/ })).toBeDefined());

    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    const dialog = await screen.findByRole("dialog", { name: "Resolved request" });
    await waitFor(() =>
      expect(within(dialog).getByLabelText("Resolved URL").textContent).toContain(
        "https://dev.example.com/users",
      ),
    );
    const headers = within(dialog).getByRole("table", { name: "Resolved headers" });
    expect(within(headers).getByText("••••")).toBeDefined();
    expect(headers.textContent).not.toContain("k-dev");

    fireEvent.click(within(dialog).getByLabelText("Reveal secrets"));
    await waitFor(() => expect(within(headers).getByText("k-dev")).toBeDefined());
  });

  it("quick look shows values, sources and overrides, revealing secrets per row", async () => {
    await setup();
    await selectEnv("E1");
    fireEvent.click(screen.getByRole("button", { name: "Variables quick look" }));
    const pop = await screen.findByRole("dialog", { name: "Variables" });
    const row = await within(pop).findByRole("row", { name: "baseUrl" });
    expect(row.textContent).toContain("https://dev.example.com");
    expect(row.textContent).toContain("Environment › Dev");
    expect(row.textContent).toContain("overrides Collection");

    const secret = within(pop).getByRole("row", { name: "apiKey" });
    expect(secret.textContent).toContain("••••");
    fireEvent.click(within(secret).getByRole("button", { name: "Reveal" }));
    await waitFor(() => expect(secret.textContent).toContain("k-dev"));
  });

  it("re-resolves the tab's variables when the environment changes", async () => {
    await setup();
    await selectEnv("E1");
    fireEvent.click(screen.getByRole("button", { name: "Variables quick look" }));
    const pop = await screen.findByRole("dialog", { name: "Variables" });
    await waitFor(() =>
      expect(within(pop).getByRole("row", { name: "baseUrl" }).textContent).toContain(
        "dev.example",
      ),
    );
    fireEvent.change(envSelect(), { target: { value: "E2" } });
    await waitFor(() =>
      expect(within(pop).getByRole("row", { name: "baseUrl" }).textContent).toContain(
        "prod.example",
      ),
    );
    fireEvent.change(envSelect(), { target: { value: "" } });
    await waitFor(() =>
      expect(within(pop).getByRole("row", { name: "baseUrl" }).textContent).toContain(
        "collection.example",
      ),
    );
  });

  it("shows the missing-secrets banner and fills secrets from it", async () => {
    const mock = await setup();
    await selectEnv("E2");
    const banner = await screen.findByRole("status", { name: "Missing secrets" });
    expect(banner.textContent).toContain("1 secret missing");
    expect(banner.textContent).toContain("Environment › Prod");

    fireEvent.click(within(banner).getByRole("button", { name: "Enter now" }));
    const dialog = await screen.findByRole("dialog", { name: "Missing secrets" });
    fireEvent.input(within(dialog).getByLabelText("token (Environment › Prod)"), {
      target: { value: "t-prod" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.queryByRole("status", { name: "Missing secrets" })).toBeNull(),
    );
    expect(mock.vars.storedSecret({ kind: "environment", id: "E2" }, "token")).toBe("t-prod");
  });

  it("warns before sending a request that uses a missing secret", async () => {
    const mock = await setup({}, ["R2"]);
    await selectEnv("E2");
    await waitFor(() => expect(url().value).toContain("{{token}}"));
    // Wait for the tab's variable context to see the missing secret.
    await waitFor(() =>
      expect(document.querySelector('[data-state="missing"]')?.textContent).toBe("{{token}}"),
    );

    mock.disk.dialog.confirm = false;
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(mock.disk.calls).toContain("confirm"));
    expect(screen.queryByTestId("status")).toBeNull();

    mock.disk.dialog.confirm = true;
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByTestId("status")).toBeDefined();
  });

  it("autocompletes {{ in the URL bar and highlights unresolved names", async () => {
    await setup();
    await selectEnv("E1");
    const input = url();
    await waitFor(() => expect(document.querySelector('[data-state="defined"]')).not.toBeNull());

    input.value = "{{ba";
    input.setSelectionRange(4, 4);
    fireEvent.input(input);
    const list = await screen.findByRole("listbox", { name: "Variables" });
    const option = within(list).getByRole("option", { name: /baseUrl/ });
    expect(option.textContent).toContain("https://dev.example.com");
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(url().value).toBe("{{baseUrl}}"));

    fireEvent.input(url(), { target: { value: "{{nope}}/x" } });
    await waitFor(() =>
      expect(document.querySelector('[data-state="unresolved"]')?.textContent).toBe("{{nope}}"),
    );
  });

  it("manages environments and keeps secret values out of the file", async () => {
    const mock = await setup();
    fireEvent.change(envSelect(), { target: { value: "__manage__" } });
    const dialog = await screen.findByRole("dialog", { name: "Environments" });

    fireEvent.click(within(dialog).getByRole("button", { name: "New" }));
    await answer("New environment", "Staging");
    await within(dialog).findByRole("option", { name: /Staging/ });

    const table = await within(dialog).findByRole("table", { name: "Variables of Staging" });
    fireEvent.input(within(table).getByLabelText("New variable name"), {
      target: { value: "password" },
    });
    const value = await within(table).findByLabelText("Value of password");
    fireEvent.input(value, { target: { value: "hunter2" } });
    fireEvent.click(within(table).getByLabelText("Secret password"));
    await waitFor(() =>
      expect(within(table).getByLabelText("Value of password").textContent).toContain(
        "not stored yet",
      ),
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => {
      const saved = mock.vars.calls.filter((c) => c.op === "environmentSave");
      expect(saved.length).toBeGreaterThan(0);
      expect(JSON.stringify(saved)).not.toContain("hunter2");
    });
    const created = (await mock.environmentsList()).find((e) => e.name === "Staging");
    if (!created) throw new Error("no Staging");
    await waitFor(() =>
      expect(mock.vars.storedSecret({ kind: "environment", id: created.id }, "password")).toBe(
        "hunter2",
      ),
    );
    expect(mock.vars.environment(created.id)?.vars).toEqual({ password: { secret: true } });
  });

  it("switches the secret backend to an encrypted file with a confirmed passphrase", async () => {
    const mock = await setup();
    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Secret storage…" }));
    const dialog = await screen.findByRole("dialog", { name: "Secret storage" });
    await waitFor(() => expect(within(dialog).getByText("(current)")).toBeDefined());

    fireEvent.click(within(dialog).getByLabelText(/Encrypted file/));
    fireEvent.click(within(dialog).getByRole("button", { name: "Switch and migrate" }));

    // Mismatched entries can't be submitted.
    const prompt = await screen.findByRole("dialog", { name: "Create encrypted file" });
    const [first, second] = prompt.querySelectorAll<HTMLInputElement>("input");
    if (!first || !second) throw new Error("expected two passphrase fields");
    fireEvent.input(first, { target: { value: "correct horse" } });
    fireEvent.input(second, { target: { value: "wrong" } });
    expect(within(prompt).getByText("The entries don't match.")).toBeDefined();
    expect(
      (within(prompt).getByRole("button", { name: "Create" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    fireEvent.input(second, { target: { value: "correct horse" } });
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));

    await waitFor(async () =>
      expect((await mock.secretBackendStatus()).current).toBe("encryptedFile"),
    );
    expect(
      mock.vars.storedSecret({ kind: "environment", id: "E1" }, "apiKey", "encryptedFile"),
    ).toBe("k-dev");
    expect(
      mock.vars.storedSecret({ kind: "environment", id: "E1" }, "apiKey", "keychain"),
    ).toBeUndefined();
  });

  it("asks before switching to the plaintext file", async () => {
    const mock = await setup();
    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Secret storage…" }));
    const dialog = await screen.findByRole("dialog", { name: "Secret storage" });
    await waitFor(() => expect(within(dialog).getByText("(current)")).toBeDefined());
    mock.disk.dialog.confirm = false;
    fireEvent.click(within(dialog).getByLabelText(/Plaintext file/));
    fireEvent.click(within(dialog).getByRole("button", { name: "Switch and migrate" }));
    await waitFor(() => expect(mock.disk.calls).toContain("confirm"));
    expect((await mock.secretBackendStatus()).current).toBe("keychain");
  });

  it("suggests the encrypted file once when no keychain is available", async () => {
    await setup({ keychainAvailable: false });
    const banner = await screen.findByRole("status", { name: "Keychain unavailable" });
    fireEvent.click(within(banner).getByRole("button", { name: "Dismiss" }));
    await waitFor(() =>
      expect(screen.queryByRole("status", { name: "Keychain unavailable" })).toBeNull(),
    );
    expect(localStorage.getItem("ironcall.keychainSuggestionDismissed")).toBe("1");
  });

  it("says when the workspace folder asks for a backend that isn't used", async () => {
    const mock = await setup({ folderBackend: "plaintextFile" });
    const banner = await screen.findByRole("status", { name: "Secret storage not changed" });
    expect(banner.textContent).toContain("plaintext file");
    expect((await mock.secretBackendStatus()).current).toBe("keychain");
    fireEvent.click(within(banner).getByRole("button", { name: "Dismiss" }));
    await waitFor(() =>
      expect(screen.queryByRole("status", { name: "Secret storage not changed" })).toBeNull(),
    );
  });

  it("unlocks a locked encrypted file, retrying a wrong passphrase", async () => {
    const mock = await setup({ backend: "encryptedFile", encryptedPassphrase: "pw" });
    const banner = await screen.findByRole("status", { name: "Secrets locked" });
    fireEvent.click(within(banner).getByRole("button", { name: "Unlock…" }));
    await answer("Unlock secrets", "nope");
    const again = await screen.findByRole("dialog", { name: "Unlock secrets" });
    expect(again.textContent).toContain("wrong passphrase");
    await answer("Unlock secrets", "pw");
    await waitFor(() =>
      expect(screen.queryByRole("status", { name: "Secrets locked" })).toBeNull(),
    );
    expect((await mock.secretBackendStatus()).unlocked).toBe(true);
  });

  it("edits request variables in the Vars tab", async () => {
    await setup();
    fireEvent.click(await screen.findByRole("tab", { name: "Vars" }));
    const table = await screen.findByRole("table", { name: "Request variables" });
    expect(within(table).queryByLabelText(/^Secret/)).toBeNull();
    fireEvent.input(within(table).getByLabelText("New variable name"), {
      target: { value: "userId" },
    });
    fireEvent.input(await within(table).findByLabelText("Value of userId"), {
      target: { value: "42" },
    });
    await waitFor(() => expect(screen.getByRole("tab", { name: "Vars (1)" })).toBeDefined());
  });
});
