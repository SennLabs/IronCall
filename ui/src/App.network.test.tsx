import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { setApi, tauriApi } from "./ipc/api";
import { createMockApi, type MockApi, type MockNetworkSeed } from "./ipc/mock";
import type { MockItem } from "./ipc/mockWorkspace";
import { closeDialog, openDialog } from "./stores/dialogs";

const ROOT = "/home/me/demo";

async function setup(
  opts: { items?: MockItem[]; network?: MockNetworkSeed; latencyMs?: number; tabs?: string[] } = {},
) {
  const mock: MockApi = createMockApi({
    latencyMs: opts.latencyMs,
    workspaces: {
      [ROOT]: {
        name: "Demo",
        headers: [{ name: "X-Team", value: "core", enabled: true }],
        items: opts.items ?? [
          {
            folder: "Users",
            id: "F1",
            auth: { type: "bearer", token: "{{token}}" },
            children: [{ request: "List users", id: "R1", url: "https://api.example.com/users" }],
          },
        ],
      },
    },
    session: {
      workspace: ROOT,
      tabs: (opts.tabs ?? ["R1"]).map((id) => ({ kind: "request" as const, id })),
      active: 0,
    },
    network: opts.network,
  });
  setApi(mock);
  render(() => <App />);
  await screen.findByRole("tree", { name: "Workspace" });
  await screen.findByLabelText("URL");
  return mock;
}

const editorTab = (name: string) => {
  const tablist = screen.getByRole("tablist", { name: "Request" });
  const tab = within(tablist)
    .getAllByRole("tab")
    .find((t) => t.textContent?.startsWith(name));
  if (!tab) throw new Error(`no ${name} tab`);
  fireEvent.click(tab);
};

/** Answers the in-app prompt (secrets, passphrases). */
async function answer(title: string, value: string) {
  const dialog = await screen.findByRole("dialog", { name: title });
  const input = dialog.querySelector<HTMLInputElement>("input");
  if (input) fireEvent.input(input, { target: { value } });
  const ok = within(dialog)
    .getAllByRole("button")
    .find((b) => b.getAttribute("type") === "submit");
  if (!ok) throw new Error("no submit button");
  fireEvent.click(ok);
}

describe("auth, bodies and network", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => {
    closeDialog();
    setApi(tauriApi);
  });

  it("shows the inherited auth and opens the folder's settings", async () => {
    await setup();
    editorTab("Auth");
    const card = await screen.findByRole("note", { name: "Inherited auth" });
    await waitFor(() =>
      expect(card).toHaveTextContent("Inherited from Folder › Users: Bearer {{token}}"),
    );
    fireEvent.click(within(card).getByRole("button", { name: "Open folder settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Folder auth & headers · Users" });
    await waitFor(() => expect(within(dialog).getByLabelText("Type")).toHaveValue("bearer"));
  });

  it("switching auth type keeps unknown keys and saves", async () => {
    const mock = await setup({
      items: [
        {
          request: "Login",
          id: "R1",
          url: "https://api.example.com/login",
          fields: { auth: { type: "bearer", token: "{{token}}", future: "kept" } },
        },
      ],
    });
    editorTab("Auth");
    fireEvent.change(screen.getByLabelText("Type"), { target: { value: "basic" } });
    fireEvent.input(screen.getByLabelText("Username"), { target: { value: "ada" } });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() =>
      expect(mock.disk.diskRequest("R1")?.auth).toEqual({
        type: "basic",
        username: "ada",
        password: "",
        future: "kept",
      }),
    );
  });

  it("masks secrets until shown, and hints when a literal credential is typed", async () => {
    await setup({
      items: [
        {
          request: "R",
          id: "R1",
          url: "https://x.io",
          fields: { auth: { type: "bearer", token: "" } },
        },
      ],
    });
    editorTab("Auth");
    const token = screen.getByLabelText("Token") as HTMLInputElement;
    expect(token.type).toBe("password");
    fireEvent.input(token, { target: { value: "sk_live_51Hx0aB2cD3eF4gH" } });
    expect(await screen.findByText(/looks like a real credential/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show Token" }));
    expect((screen.getByLabelText("Token") as HTMLInputElement).type).toBe("text");
  });

  it("edits a multipart body and picks a file", async () => {
    const mock = await setup();
    editorTab("Body");
    fireEvent.change(screen.getByLabelText("Body type"), { target: { value: "multipart" } });
    fireEvent.click(screen.getByRole("button", { name: "Add text part" }));
    fireEvent.input(screen.getByLabelText("Part name 1"), { target: { value: "title" } });
    fireEvent.input(screen.getByLabelText("Part value 1"), { target: { value: "Report" } });
    mock.network.queueBodyFile({
      path: "/home/me/report.pdf",
      fileName: "report.pdf",
      size: 2048,
      contentType: "application/pdf",
      outsideWorkspace: true,
    });
    fireEvent.click(screen.getByRole("button", { name: "Add file part…" }));
    const chip = await screen.findByRole("button", { name: "Part file 2: /home/me/report.pdf" });
    expect(chip).toHaveTextContent("report.pdf");
    expect(await screen.findByText("outside workspace")).toBeInTheDocument();
    expect(screen.getByLabelText("Part name 2")).toHaveValue("report");
    expect(screen.getByLabelText("Part content type 2")).toHaveValue("application/pdf");
    // Picked this session, so it's allowed: no problem shown.
    await waitFor(() => expect(screen.queryByText(/re-select this file/)).toBeNull());
  });

  it("blocks Send for a file outside the workspace that wasn't picked this session", async () => {
    const mock = await setup({
      items: [
        {
          request: "Upload",
          id: "R1",
          method: "POST",
          url: "https://x.io/upload",
          fields: { body: { type: "binaryFile", path: "/etc/passwd" } },
        },
      ],
      network: { files: { "/etc/passwd": { size: 100 } } },
    });
    let sends = 0;
    const original = mock.sendRequest;
    mock.sendRequest = (...args) => {
      sends += 1;
      return original(...args);
    };
    editorTab("Body");
    expect(await screen.findByRole("alert")).toHaveTextContent(/re-select this file/);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(
      await screen.findByText(/Re-select this file to allow it this session/),
    ).toBeInTheDocument();
    expect(sends).toBe(0);
  });

  it("shows upload progress while a file body is sent", async () => {
    const mock = await setup({
      latencyMs: 200,
      items: [
        {
          request: "Upload",
          id: "R1",
          method: "PUT",
          url: "https://x.io/upload",
          fields: { body: { type: "binaryFile", path: "data/big.bin" } },
        },
      ],
      network: { files: { "data/big.bin": { size: 4096 } } },
    });
    void mock;
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const bar = await screen.findByRole("progressbar", { name: "Upload progress" });
    expect(bar).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText(/50% · 2\.0 KB of 4\.0 KB sent/)).toBeInTheDocument();
    await screen.findByTestId("status");
  });

  it("previews auth and default headers with their origin; overridden ones are struck through", async () => {
    await setup({
      items: [
        {
          folder: "Users",
          id: "F1",
          auth: { type: "bearer", token: "{{token}}" },
          children: [
            {
              request: "List users",
              id: "R1",
              url: "https://api.example.com/users",
              fields: { headers: [{ name: "X-Team", value: "mine", enabled: true }] },
            },
          ],
        },
      ],
    });
    editorTab("Headers");
    const table = screen.getByRole("table", { name: "Headers" });
    await waitFor(() => expect(within(table).getByText("Bearer {{token}}")).toBeInTheDocument());
    const authRow = within(table).getByText("Bearer {{token}}").closest("tr");
    expect(authRow).toHaveTextContent("Folder › Users");
    expect(authRow).toHaveTextContent("auth");
    const defaultValue = within(table).getByText("core");
    expect(defaultValue.className).toMatch(/overridden/);
    expect(defaultValue.closest("tr")).toHaveTextContent("Collection");
  });

  it("adds, edits, deletes and clears cookies", async () => {
    const mock = await setup({
      network: {
        cookies: [
          {
            domain: "api.example.com",
            path: "/",
            name: "session",
            value: "abc",
            expires: null,
            secure: true,
            httpOnly: true,
            sameSite: "lax",
            hostOnly: true,
          },
        ],
      },
    });
    openDialog({ kind: "cookies" });
    const dialog = await screen.findByRole("dialog", { name: "Cookies" });
    const table = await within(dialog).findByRole("table", { name: "Cookies" });
    expect(within(table).getByText("session")).toBeInTheDocument();
    expect(within(table).getByText(/Secure · HttpOnly · SameSite=lax/)).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Add cookie" }));
    const form = within(dialog).getByRole("form", { name: "New cookie" });
    fireEvent.input(within(form).getByLabelText("Domain"), { target: { value: "x.io" } });
    fireEvent.input(within(form).getByLabelText("Name"), { target: { value: "theme" } });
    fireEvent.input(within(form).getByLabelText("Value"), { target: { value: "dark" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save cookie" }));
    await waitFor(() => expect(within(dialog).getByText("theme")).toBeInTheDocument());

    fireEvent.click(within(dialog).getByRole("button", { name: "Edit theme" }));
    const edit = within(dialog).getByRole("form", { name: "Edit cookie" });
    fireEvent.input(within(edit).getByLabelText("Value"), { target: { value: "light" } });
    fireEvent.click(within(edit).getByRole("button", { name: "Save cookie" }));
    await waitFor(() => expect(within(dialog).getByText("light")).toBeInTheDocument());

    fireEvent.click(within(dialog).getByRole("button", { name: "Delete session" }));
    await waitFor(() => expect(within(dialog).queryByText("session")).toBeNull());

    mock.disk.dialog.confirm = true;
    fireEvent.click(within(dialog).getByRole("button", { name: "Clear all" }));
    await waitFor(() => expect(within(dialog).getByText("No cookies.")).toBeInTheDocument());
    expect(mock.network.calls.map((c) => c.op)).toEqual(
      expect.arrayContaining(["cookieSet", "cookieDelete", "cookiesClear"]),
    );
  });

  it("saves manual proxy settings and stores the password separately", async () => {
    const mock = await setup({ network: { environmentProxy: "http://env-proxy:3128" } });
    openDialog({ kind: "network" });
    const dialog = await screen.findByRole("dialog", { name: "Network settings" });
    expect(await within(dialog).findByText("http://env-proxy:3128")).toBeInTheDocument();

    fireEvent.change(within(dialog).getByLabelText("Mode"), { target: { value: "manual" } });
    const save = within(dialog).getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    fireEvent.input(within(dialog).getByLabelText("Proxy URL"), {
      target: { value: "http://proxy.corp:8080" },
    });
    fireEvent.input(within(dialog).getByLabelText("Username"), { target: { value: "me" } });
    fireEvent.input(within(dialog).getByLabelText("New no-proxy host"), {
      target: { value: ".corp" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(within(dialog).getByRole("list", { name: "No-proxy hosts" })).toHaveTextContent(".corp");

    fireEvent.click(within(dialog).getByRole("button", { name: "Set…" }));
    await answer("Proxy password", "pa55word");
    expect(await within(dialog).findByText("stored")).toBeInTheDocument();
    expect(mock.network.password()).toBe("pa55word");

    fireEvent.click(save);
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Network settings" })).toBeNull(),
    );
    const call = mock.network.calls.find((c) => c.op === "networkSettingsSet");
    expect(call?.args[0]).toMatchObject({
      mode: "manual",
      url: "http://proxy.corp:8080",
      username: "me",
      noProxy: [".corp"],
    });
    // The password never goes through the settings call.
    expect(JSON.stringify(call?.args)).not.toContain("pa55word");
  });

  it("adds and removes global and workspace CA certificates", async () => {
    const mock = await setup({
      network: {
        caCerts: [
          { path: "/certs/old.pem", subject: "CN=Old CA", notAfter: null, count: 1, error: null },
        ],
      },
    });
    openDialog({ kind: "network" });
    const dialog = await screen.findByRole("dialog", { name: "Network settings" });
    const globals = await within(dialog).findByRole("list", { name: "Global CA certificates" });
    expect(globals).toHaveTextContent("CN=Old CA");

    mock.network.queueCaCert({
      path: "/certs/corp.pem",
      subject: "CN=Corp Root CA",
      notAfter: "2031-06-30T00:00:00Z",
      count: 2,
      error: null,
    });
    const [addGlobal, addWorkspace] = within(dialog).getAllByRole("button", {
      name: "Add certificate…",
    });
    if (!addGlobal || !addWorkspace) throw new Error("missing add buttons");
    fireEvent.click(addGlobal);
    await waitFor(() => expect(globals).toHaveTextContent("CN=Corp Root CA"));
    expect(globals).toHaveTextContent("2 certificates · expires 2031-06-30");
    fireEvent.click(within(globals).getByRole("button", { name: "Remove CN=Old CA" }));
    expect(globals).not.toHaveTextContent("CN=Old CA");

    mock.network.queueCaCert({
      path: "/certs/dev.crt",
      subject: "CN=Dev CA",
      notAfter: null,
      count: 1,
      error: null,
    });
    fireEvent.click(addWorkspace);
    const wsList = within(dialog).getByRole("list", { name: "Workspace CA certificates" });
    await waitFor(() => expect(wsList).toHaveTextContent("CN=Dev CA"));

    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Network settings" })).toBeNull(),
    );
    const set = mock.network.calls.find((c) => c.op === "networkSettingsSet");
    expect(set?.args[1]).toEqual(["/certs/corp.pem"]);
    const ws = mock.network.calls.find((c) => c.op === "workspaceCaCertsSet");
    expect(ws?.args[0]).toEqual(["/certs/dev.crt"]);
  });

  it("saves folder auth and default headers", async () => {
    const mock = await setup();
    openDialog({ kind: "containerSettings", target: { kind: "folder", id: "F1", name: "Users" } });
    const dialog = await screen.findByRole("dialog", { name: "Folder auth & headers · Users" });
    await waitFor(() => expect(within(dialog).getByLabelText("Type")).toHaveValue("bearer"));
    fireEvent.change(within(dialog).getByLabelText("Type"), { target: { value: "apiKey" } });
    fireEvent.input(within(dialog).getByLabelText("Key"), { target: { value: "X-Api-Key" } });
    fireEvent.input(within(dialog).getByLabelText("New name"), { target: { value: "Accept" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mock.disk.calls).toContain("folderSave"));
    expect(await mock.folderLoad("F1")).toMatchObject({
      folder: {
        auth: { type: "apiKey", key: "X-Api-Key", in: "header" },
        headers: [{ name: "Accept", value: "", enabled: true }],
      },
    });
  });
});
