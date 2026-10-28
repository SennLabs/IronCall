import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  api,
  type CollectionDraft,
  type EnvironmentDraft,
  type FolderDraft,
  setApi,
  tauriApi,
  type VarsDraft,
} from "../ipc/api";
import { createMockApi, type MockApi } from "../ipc/mock";
import { promptRequest } from "./dialogs";
import { createVarsEditor, type VarsSource } from "./varsEditor";

const ROOT = "/home/me/demo";
const ENV: EnvironmentDraft = {
  schema: "ironcall/environment@1",
  id: "E1",
  name: "Dev",
  vars: { baseUrl: "https://dev.example.com", apiKey: { secret: true, description: "key" } },
};

async function setup() {
  const mock: MockApi = createMockApi({
    workspaces: { [ROOT]: { name: "Demo", items: [{ folder: "Users", id: "F1" }] } },
    vars: {
      environments: { [ROOT]: [structuredClone(ENV)] },
      secrets: { "environment:E1/apiKey": "k-123" },
    },
  });
  setApi(mock);
  await mock.workspaceOpen(ROOT);
  return mock;
}

function envSource(id = "E1"): VarsSource {
  let file: EnvironmentDraft | undefined;
  return {
    async load() {
      const l = await api.environmentLoad(id);
      file = l.environment;
      return { vars: l.environment.vars ?? {}, hash: l.hash };
    },
    save: (vars, hash) => api.environmentSave({ ...(file as EnvironmentDraft), vars }, hash),
  };
}

function envEditor() {
  return createRoot(() => createVarsEditor(envSource(), { kind: "environment", id: "E1" }));
}

const scope = { kind: "environment", id: "E1" } as const;

/** Answers the next in-app prompt. */
async function answerPrompt(value: string | null) {
  await vi.waitFor(() => expect(promptRequest()).toBeDefined());
  promptRequest()?.resolve(value);
}

afterEach(() => setApi(tauriApi));

describe("variables editor", () => {
  it("loads rows and saves plain edits with the base hash", async () => {
    const mock = await setup();
    const save = vi.spyOn(mock, "environmentSave");
    const e = envEditor();
    await e.reload();
    expect(e.rows().map((r) => r.name)).toEqual(["baseUrl", "apiKey"]);
    expect(e.dirty()).toBe(false);

    e.setRows(e.rows().map((r, i) => (i === 0 ? { ...r, value: "https://dev2" } : r)));
    expect(e.dirty()).toBe(true);
    expect(await e.save()).toBe(true);
    expect(save.mock.calls[0]?.[1]).toMatch(/^[0-9a-f]{8}$/);
    expect(mock.vars.environment("E1")?.vars?.baseUrl).toBe("https://dev2");
    expect(e.dirty()).toBe(false);
  });

  it("turning secret on keeps the value out of the file and stores it", async () => {
    const mock = await setup();
    const e = envEditor();
    await e.reload();
    await e.toggleSecret(0, true);
    expect(e.rows()[0]).toMatchObject({
      secret: true,
      value: "",
      pendingSecret: "https://dev.example.com",
    });
    expect(e.secretState(e.rows()[0] as never)).toMatchObject({ pending: true });

    await e.save();
    expect(mock.vars.environment("E1")?.vars?.baseUrl).toEqual({ secret: true });
    expect(JSON.stringify(mock.vars.environment("E1"))).not.toContain("dev.example.com");
    expect(mock.vars.storedSecret(scope, "baseUrl")).toBe("https://dev.example.com");
  });

  it("turning secret off asks, moves the value into the file and clears the store", async () => {
    const mock = await setup();
    const e = envEditor();
    await e.reload();

    mock.disk.dialog.confirm = false;
    await e.toggleSecret(1, false);
    expect(e.rows()[1]?.secret).toBe(true);

    mock.disk.dialog.confirm = true;
    await e.toggleSecret(1, false);
    expect(e.rows()[1]).toMatchObject({ secret: false, value: "k-123" });
    await e.save();
    expect(mock.vars.environment("E1")?.vars?.apiKey).toEqual({
      value: "k-123",
      description: "key",
    });
    expect(mock.vars.storedSecret(scope, "apiKey")).toBeUndefined();
  });

  it("sets a stored secret right away and reveals it on demand", async () => {
    const mock = await setup();
    const e = envEditor();
    await e.reload();
    const done = e.setSecret(1);
    await answerPrompt("k-456");
    await done;
    expect(mock.vars.storedSecret(scope, "apiKey")).toBe("k-456");
    expect(e.dirty()).toBe(false);

    await e.reveal(1);
    expect(e.revealed()).toEqual({ apiKey: "k-456" });
    await e.reveal(1);
    expect(e.revealed()).toEqual({});
  });

  it("moves a renamed secret's value and clears deleted ones", async () => {
    const mock = await setup();
    const e = envEditor();
    await e.reload();
    e.setRows(e.rows().map((r) => (r.name === "apiKey" ? { ...r, name: "key2" } : r)));
    await e.save();
    expect(mock.vars.storedSecret(scope, "key2")).toBe("k-123");
    expect(mock.vars.storedSecret(scope, "apiKey")).toBeUndefined();

    e.setRows(e.rows().filter((r) => r.name !== "key2"));
    await e.save();
    expect(mock.vars.storedSecret(scope, "key2")).toBeUndefined();
  });

  it("refuses to overwrite a file changed on disk and reloads it", async () => {
    const mock = await setup();
    const e = envEditor();
    await e.reload();
    mock.vars.externalEditEnvironment("E1", { vars: { baseUrl: "https://theirs" } });
    e.setRows(e.rows().map((r, i) => (i === 0 ? { ...r, value: "https://mine" } : r)));
    expect(await e.save()).toBe(false);
    expect(e.conflict()).toBe(true);
    expect(mock.vars.environment("E1")?.vars?.baseUrl).toBe("https://theirs");

    await e.reload();
    expect(e.conflict()).toBe(false);
    expect(e.rows().map((r) => r.value)).toEqual(["https://theirs"]);
  });
});

describe("secret values never reach a saved file", () => {
  it("across environments, globals, the collection and folders", async () => {
    const mock = await setup();
    const spies = [
      vi.spyOn(mock, "environmentSave"),
      vi.spyOn(mock, "globalsSave"),
      vi.spyOn(mock, "collectionSave"),
      vi.spyOn(mock, "folderSave"),
    ];
    const secrets = ["S3CRET-env", "S3CRET-global", "S3CRET-collection", "S3CRET-folder"];

    let collection: CollectionDraft | undefined;
    let folder: FolderDraft | undefined;
    const editors = createRoot(() => [
      createVarsEditor(envSource(), scope),
      createVarsEditor(
        { load: () => api.globalsLoad(), save: (v, h) => api.globalsSave(v, h) },
        { kind: "global" },
      ),
      createVarsEditor(
        {
          async load() {
            const c = await api.collectionLoad();
            collection = c.collection;
            return { vars: (c.collection.vars as VarsDraft | undefined) ?? {}, hash: c.hash };
          },
          save: (vars, h) => api.collectionSave({ ...(collection as CollectionDraft), vars }, h),
        },
        { kind: "collection" },
      ),
      createVarsEditor(
        {
          async load() {
            const f = await api.folderLoad("F1");
            folder = f.folder;
            return { vars: (f.folder.vars as VarsDraft | undefined) ?? {}, hash: f.hash };
          },
          save: (vars, h) => api.folderSave({ ...(folder as FolderDraft), vars }, h),
        },
        { kind: "folder", id: "F1" },
      ),
    ]);

    for (const [i, e] of editors.entries()) {
      await e.reload();
      // Typed as plain, then made secret; then a second secret set through the prompt.
      e.setRows([
        ...e.rows(),
        { name: "token", value: secrets[i] ?? "", secret: false, description: "", extra: {} },
        { name: "other", value: "", secret: false, description: "", extra: {} },
      ]);
      const n = e.rows().length;
      await e.toggleSecret(n - 2, true);
      await e.toggleSecret(n - 1, true);
      const set = e.setSecret(n - 1);
      await answerPrompt(`${secrets[i]}-2`);
      await set;
      expect(await e.save()).toBe(true);
    }

    const written = JSON.stringify(spies.map((s) => s.mock.calls as unknown[]));
    for (const s of secrets) expect(written).not.toContain(s);
    // …but every value did reach the secret store.
    expect(mock.vars.storedSecret({ kind: "global" }, "token")).toBe("S3CRET-global");
    expect(mock.vars.storedSecret({ kind: "folder", id: "F1" }, "other")).toBe("S3CRET-folder-2");
    expect(mock.vars.storedSecret(scope, "token")).toBe("S3CRET-env");
  });
});
