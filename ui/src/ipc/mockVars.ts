/**
 * In-memory variables, environments and secrets for the mock backend.
 *
 * Mirrors the contract in `src-tauri/src/vars.rs`: precedence Global < Collection <
 * Environment < Folder < Request, nested `{{refs}}` with cycle detection, zero-argument
 * helpers, secret values held only in a fake per-backend secret store, and the
 * encrypted-file backend's lock. It's a stand-in for tests and browser dev, not a model
 * of every backend detail.
 */

import type { RequestDraft } from "./api";
import { contentHash, mockId } from "./mockWorkspace";
import {
  type EnvironmentDraft,
  type EnvironmentSummary,
  type MissingSecret,
  type SecretBackend,
  type SecretBackendStatus,
  type SecretScope,
  type VariableContext,
  type VarSource,
  type VarsDraft,
  VarsFailure,
  type VisibleVar,
} from "./varsTypes";
import type { CollectionDraft, FolderDraft, ItemId, WorkspaceChanged } from "./workspaceTypes";

export interface MockVarsSeed {
  globals?: VarsDraft;
  /** Environments per workspace root. */
  environments?: Record<string, EnvironmentDraft[]>;
  /** Active environment per workspace root. */
  active?: Record<string, ItemId | null>;
  /** Stored secrets, keyed `scopeKey/name` (see `scopeKey`), in the current backend. */
  secrets?: Record<string, string>;
  keychainAvailable?: boolean;
  backend?: SecretBackend;
  /** Passphrase of an existing encrypted file (makes it exist and start locked). */
  encryptedPassphrase?: string;
  /** A backend named by the workspace's own `.ironcall/local.yaml` (not used). */
  folderBackend?: SecretBackend;
}

/** What the vars layer needs from the mock workspace. */
export interface MockWorkspaceAccess {
  root(): string | null;
  collection(): CollectionDraft | null;
  setCollection(c: CollectionDraft): void;
  folderChain(id: ItemId): FolderDraft[];
  folders(): FolderDraft[];
  emit(change: Partial<WorkspaceChanged>): void;
}

export const HELPERS = [
  { name: "$uuid", description: "Random UUID v4", example: "3f2b8c1e-5d4a-4e7b-9c0d-1a2b3c4d5e6f" },
  { name: "$ulid", description: "Random ULID", example: "01J9Z3K7T2V8Q4M6N0P5R1S3W7" },
  { name: "$timestamp", description: "Unix time in seconds", example: "1791172800" },
  {
    name: "$isoTimestamp",
    description: "Current time, ISO 8601 (UTC)",
    example: "2026-10-05T08:00:00Z",
  },
  { name: "$randomInt", description: "Random integer 0–1000", example: "417" },
  { name: "$randomEmail", description: "Random email address", example: "user4821@example.com" },
];

const MASK = "••••";
const MAX_DEPTH = 10;
const REF = /\{\{\s*(\$?[\w.-]+)\s*\}\}/g;

export function scopeKey(scope: SecretScope): string {
  return scope.kind === "environment" || scope.kind === "folder"
    ? `${scope.kind}:${scope.id}`
    : scope.kind;
}

function sourceScope(source: VarSource): SecretScope | undefined {
  if (source.scope === "global") return { kind: "global" };
  if (source.scope === "collection") return { kind: "collection" };
  if (source.scope === "environment") return { kind: "environment", id: source.id };
  if (source.scope === "folder") return { kind: "folder", id: source.id };
  return undefined;
}

interface Def {
  raw: string | null;
  secret: boolean;
  missing: boolean;
  description: string | null;
  source: VarSource;
}

const clone = <T>(v: T): T => structuredClone(v);

interface WsState {
  envs: Map<ItemId, { env: EnvironmentDraft; path: string }>;
  active: ItemId | null;
  backend: SecretBackend;
  encryptedPassphrase: string | null;
  unlocked: boolean;
  /** Stored secrets per backend. */
  stores: Record<SecretBackend, Map<string, string>>;
}

export function createMockVars(access: MockWorkspaceAccess, seed: MockVarsSeed = {}) {
  let globals: VarsDraft = clone(seed.globals ?? {});
  const globalSecrets = new Map<string, string>();
  const states = new Map<string, WsState>();
  const keychainAvailable = seed.keychainAvailable ?? true;
  let folderBackend = seed.folderBackend ?? null;
  const calls: { op: string; args: unknown[] }[] = [];

  for (const [k, v] of Object.entries(seed.secrets ?? {})) {
    if (k.startsWith("global/")) globalSecrets.set(k, v);
  }

  const state = (): WsState => {
    const root = access.root();
    if (!root) throw new VarsFailure("no workspace is open");
    let s = states.get(root);
    if (!s) {
      const backend = seed.backend ?? "keychain";
      s = {
        envs: new Map(
          (seed.environments?.[root] ?? []).map((e) => [
            e.id,
            { env: clone(e), path: `environments/${slug(e.name)}.yaml` },
          ]),
        ),
        active: seed.active?.[root] ?? null,
        backend,
        encryptedPassphrase: seed.encryptedPassphrase ?? null,
        unlocked: !seed.encryptedPassphrase,
        stores: { keychain: new Map(), encryptedFile: new Map(), plaintextFile: new Map() },
      };
      for (const [k, v] of Object.entries(seed.secrets ?? {})) {
        if (!k.startsWith("global/")) s.stores[backend].set(k, v);
      }
      states.set(root, s);
    }
    return s;
  };

  const locked = (s: WsState) => s.backend === "encryptedFile" && !s.unlocked;
  const assertUnlocked = (s: WsState) => {
    if (locked(s)) throw new VarsFailure("the secrets file is locked", true);
  };

  const secretValue = (scope: SecretScope, name: string): string | undefined => {
    const key = `${scopeKey(scope)}/${name}`;
    if (scope.kind === "global") return globalSecrets.get(key);
    const s = access.root() ? state() : undefined;
    if (!s || locked(s)) return undefined;
    return s.stores[s.backend].get(key);
  };

  /** Variables declared at one scope, as definitions. */
  const defsOf = (vars: VarsDraft | undefined, source: VarSource): [string, Def][] =>
    Object.entries(vars ?? {}).map(([name, v]) => {
      if (typeof v === "string") {
        return [name, { raw: v, secret: false, missing: false, description: null, source }];
      }
      if (v.secret) {
        const scope = sourceScope(source);
        const value = scope ? secretValue(scope, name) : undefined;
        return [
          name,
          {
            raw: value ?? null,
            secret: true,
            missing: value === undefined,
            description: v.description ?? null,
            source,
          },
        ];
      }
      return [
        name,
        {
          raw: v.value ?? "",
          secret: false,
          missing: false,
          description: v.description ?? null,
          source,
        },
      ];
    });

  /** Layers lowest precedence first. */
  const layers = (request: RequestDraft | null): [string, Def][][] => {
    const out: [string, Def][][] = [defsOf(globals, { scope: "global" })];
    const collection = access.collection();
    if (collection) {
      out.push(defsOf(collection.vars as VarsDraft | undefined, { scope: "collection" }));
      const s = state();
      const env = s.active ? s.envs.get(s.active)?.env : undefined;
      if (env) {
        out.push(defsOf(env.vars, { scope: "environment", id: env.id, name: env.name }));
      }
      if (request) {
        for (const f of access.folderChain(request.id)) {
          out.push(
            defsOf(f.vars as VarsDraft | undefined, { scope: "folder", id: f.id, name: f.name }),
          );
        }
      }
    }
    if (request) out.push(defsOf(request.vars, { scope: "request" }));
    return out;
  };

  /** Winner per name, plus overridden sources (highest first). */
  const visibleDefs = (request: RequestDraft | null) => {
    const map = new Map<string, { def: Def; overrides: VarSource[] }>();
    for (const layer of layers(request)) {
      for (const [name, def] of layer) {
        const prev = map.get(name);
        map.set(name, {
          def,
          overrides: prev ? [prev.def.source, ...prev.overrides] : [],
        });
      }
    }
    return map;
  };

  type Resolved = { text: string; usedSecret: boolean; error?: string };

  const resolveText = (
    text: string,
    defs: Map<string, { def: Def }>,
    reveal: boolean,
    stack: string[] = [],
  ): Resolved => {
    let usedSecret = false;
    let error: string | undefined;
    const out = text.replace(REF, (whole, name: string) => {
      if (name.startsWith("$")) {
        const h = HELPERS.find((x) => x.name === name);
        return h ? h.example : whole;
      }
      const entry = defs.get(name);
      if (!entry) return whole;
      if (stack.includes(name)) {
        error = `cycle: ${[...stack, name].join(" → ")}`;
        return whole;
      }
      if (stack.length >= MAX_DEPTH) {
        error = "nested more than 10 levels";
        return whole;
      }
      const def = entry.def;
      if (def.secret) {
        usedSecret = true;
        if (def.raw === null) return whole;
        return reveal ? def.raw : MASK;
      }
      const inner = resolveText(def.raw ?? "", defs, reveal, [...stack, name]);
      usedSecret ||= inner.usedSecret;
      error ??= inner.error;
      return inner.text;
    });
    return { text: out, usedSecret, error };
  };

  const resolveRequest = (request: RequestDraft, reveal: boolean): RequestDraft => {
    const defs = visibleDefs(request);
    const r = (t: string | undefined) => (t === undefined ? t : resolveText(t, defs, reveal).text);
    const body = request.body;
    return {
      ...request,
      url: r(request.url) ?? "",
      params: request.params?.map((p) => ({ ...p, name: r(p.name) ?? "", value: r(p.value) })),
      pathParams: request.pathParams?.map((p) => ({ ...p, value: r(p.value) })),
      headers: request.headers?.map((h) => ({ ...h, name: r(h.name) ?? "", value: r(h.value) })),
      body: body && typeof body.content === "string" ? { ...body, content: r(body.content) } : body,
    };
  };

  const textsOf = (request: RequestDraft) => [
    request.url,
    ...(request.params ?? []).flatMap((p) => [p.name, p.value ?? ""]),
    ...(request.pathParams ?? []).map((p) => p.value ?? ""),
    ...(request.headers ?? []).flatMap((h) => [h.name, h.value ?? ""]),
    typeof request.body?.content === "string" ? request.body.content : "",
  ];

  const missingFor = (vars: VarsDraft | undefined, scope: SecretScope, label: string) =>
    Object.entries(vars ?? {})
      .filter(
        ([name, v]) => typeof v !== "string" && v.secret && secretValue(scope, name) === undefined,
      )
      .map(([name]): MissingSecret => ({ scope, name, scopeLabel: label }));

  const envMissingNames = (s: WsState, env: EnvironmentDraft) =>
    locked(s)
      ? []
      : missingFor(env.vars, { kind: "environment", id: env.id }, "").map((m) => m.name);

  const summary = (s: WsState, id: ItemId): EnvironmentSummary => {
    const e = s.envs.get(id);
    if (!e) throw new VarsFailure(`no environment with ID ${id}`);
    return {
      id,
      name: e.env.name,
      path: e.path,
      error: null,
      missingSecrets: envMissingNames(s, e.env).length,
      missingSecretNames: envMissingNames(s, e.env),
    };
  };

  const declared = (scope: SecretScope, name: string): boolean => {
    const has = (vars: VarsDraft | undefined) => {
      const v = vars?.[name];
      return typeof v === "object" && !!v?.secret;
    };
    switch (scope.kind) {
      case "global":
        return has(globals);
      case "collection":
        return has(access.collection()?.vars as VarsDraft | undefined);
      case "environment":
        return has(state().envs.get(scope.id)?.env.vars);
      case "folder":
        return has(access.folders().find((f) => f.id === scope.id)?.vars as VarsDraft | undefined);
    }
  };

  const store = (scope: SecretScope) => {
    if (scope.kind === "global") return globalSecrets;
    const s = state();
    assertUnlocked(s);
    return s.stores[s.backend];
  };

  const status = (): SecretBackendStatus => {
    const s = state();
    return {
      current: s.backend,
      keychainAvailable,
      encryptedFileExists: s.encryptedPassphrase !== null,
      unlocked: s.backend !== "encryptedFile" || s.unlocked,
      folderBackend: folderBackend !== s.backend ? folderBackend : null,
    };
  };

  const changed = () => access.emit({ environmentsChanged: true });

  const api = {
    async variablesFor(request: RequestDraft, reveal: boolean): Promise<VariableContext> {
      calls.push({ op: "variablesFor", args: [request.id, reveal] });
      const defs = visibleDefs(request);
      const variables: VisibleVar[] = [...defs.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, { def, overrides }]) => {
          const resolved = def.secret ? null : resolveText(def.raw ?? "", defs, reveal, [name]);
          return {
            name,
            value: def.secret
              ? reveal && !def.missing
                ? def.raw
                : null
              : (resolved?.text ?? null),
            raw: def.secret ? null : def.raw,
            secret: def.secret,
            missing: def.missing,
            source: def.source,
            overrides,
            error: resolved?.error ?? null,
            description: def.description,
          };
        });
      const unresolved = new Set<string>();
      for (const text of textsOf(request)) {
        for (const m of text.matchAll(REF)) {
          const name = m[1] ?? "";
          if (name.startsWith("$") ? !HELPERS.some((h) => h.name === name) : !defs.has(name)) {
            unresolved.add(name);
          }
        }
      }
      return { variables, helpers: HELPERS, unresolved: [...unresolved].sort() };
    },

    async environmentsList(): Promise<EnvironmentSummary[]> {
      const s = state();
      return [...s.envs.keys()]
        .map((id) => summary(s, id))
        .sort((a, b) => a.name.localeCompare(b.name));
    },
    async environmentCreate(name: string): Promise<EnvironmentSummary> {
      calls.push({ op: "environmentCreate", args: [name] });
      const s = state();
      const id = mockId();
      s.envs.set(id, {
        env: { schema: "ironcall/environment@1", id, name },
        path: `environments/${slug(name)}.yaml`,
      });
      return summary(s, id);
    },
    async environmentDuplicate(id: ItemId): Promise<EnvironmentSummary> {
      const s = state();
      const e = s.envs.get(id);
      if (!e) throw new VarsFailure(`no environment with ID ${id}`);
      const copyId = mockId();
      const name = `${e.env.name} copy`;
      s.envs.set(copyId, {
        env: { ...clone(e.env), id: copyId, name },
        path: `environments/${slug(name)}.yaml`,
      });
      // Secrets are copied too.
      for (const backend of Object.values(s.stores)) {
        for (const [k, v] of [...backend]) {
          const prefix = `environment:${id}/`;
          if (k.startsWith(prefix))
            backend.set(`environment:${copyId}/${k.slice(prefix.length)}`, v);
        }
      }
      return summary(s, copyId);
    },
    async environmentRename(id: ItemId, name: string) {
      const e = state().envs.get(id);
      if (!e) throw new VarsFailure(`no environment with ID ${id}`);
      e.env = { ...e.env, name };
    },
    async environmentDelete(id: ItemId) {
      calls.push({ op: "environmentDelete", args: [id] });
      const s = state();
      s.envs.delete(id);
      if (s.active === id) s.active = null;
      for (const backend of Object.values(s.stores)) {
        for (const k of [...backend.keys()])
          if (k.startsWith(`environment:${id}/`)) backend.delete(k);
      }
    },
    async environmentLoad(id: ItemId) {
      const e = state().envs.get(id);
      if (!e) throw new VarsFailure(`no environment with ID ${id}`);
      return { environment: clone(e.env), hash: contentHash(e.env) };
    },
    async environmentSave(environment: EnvironmentDraft, baseHash: string | null) {
      calls.push({ op: "environmentSave", args: [clone(environment), baseHash] });
      const s = state();
      const e = s.envs.get(environment.id);
      if (!e) throw new VarsFailure(`no environment with ID ${environment.id}`);
      if (baseHash !== null && contentHash(e.env) !== baseHash) {
        throw new VarsFailure(
          "this file changed on disk since it was opened; reload it and try again",
          false,
          true,
        );
      }
      for (const [name, v] of Object.entries(environment.vars ?? {})) {
        if (typeof v === "object" && v.secret && v.value != null) {
          throw new VarsFailure(`secret variable \`${name}\` must not have a value in the file`);
        }
      }
      e.env = clone(environment);
      return contentHash(e.env);
    },
    async activeEnvironmentGet() {
      return access.root() ? state().active : null;
    },
    async activeEnvironmentSet(id: ItemId | null) {
      calls.push({ op: "activeEnvironmentSet", args: [id] });
      const s = state();
      if (id !== null && !s.envs.has(id)) throw new VarsFailure(`no environment with ID ${id}`);
      s.active = id;
    },
    async globalsLoad() {
      return { vars: clone(globals), hash: contentHash(globals) };
    },
    async globalsSave(vars: VarsDraft, baseHash: string | null) {
      calls.push({ op: "globalsSave", args: [clone(vars), baseHash] });
      if (baseHash !== null && contentHash(globals) !== baseHash) {
        throw new VarsFailure("the global variables changed since they were opened; reload them");
      }
      globals = clone(vars);
      return contentHash(globals);
    },

    async secretBackendStatus() {
      return status();
    },
    async secretBackendSet(backend: SecretBackend, passphrase: string | null) {
      calls.push({ op: "secretBackendSet", args: [backend, passphrase ? "<passphrase>" : null] });
      const s = state();
      if (backend === "keychain" && !keychainAvailable) {
        throw new VarsFailure("no OS keychain is available on this machine");
      }
      if (backend === "encryptedFile") {
        if (!passphrase) throw new VarsFailure("a passphrase is required");
        if (s.encryptedPassphrase !== null && s.encryptedPassphrase !== passphrase) {
          throw new VarsFailure("wrong passphrase");
        }
        s.encryptedPassphrase = passphrase;
        s.unlocked = true;
      }
      assertUnlocked(s);
      // Migrate every stored value.
      const from = s.stores[s.backend];
      const to = s.stores[backend];
      if (from !== to) {
        for (const [k, v] of from) to.set(k, v);
        from.clear();
      }
      s.backend = backend;
      folderBackend = null;
      changed();
      return status();
    },
    async secretsUnlock(passphrase: string) {
      const s = state();
      if (s.encryptedPassphrase !== passphrase) throw new VarsFailure("wrong passphrase");
      s.unlocked = true;
      changed();
    },
    async secretSet(scope: SecretScope, name: string, value: string) {
      calls.push({ op: "secretSet", args: [scope, name] });
      if (!declared(scope, name)) {
        throw new VarsFailure(`\`${name}\` is not declared as a secret in that scope`);
      }
      store(scope).set(`${scopeKey(scope)}/${name}`, value);
      changed();
    },
    async secretClear(scope: SecretScope, name: string) {
      calls.push({ op: "secretClear", args: [scope, name] });
      store(scope).delete(`${scopeKey(scope)}/${name}`);
      changed();
    },
    async secretReveal(scope: SecretScope, name: string) {
      calls.push({ op: "secretReveal", args: [scope, name] });
      return store(scope).get(`${scopeKey(scope)}/${name}`) ?? null;
    },
    async secretsMissing(): Promise<MissingSecret[]> {
      const out: MissingSecret[] = [...missingFor(globals, { kind: "global" }, "Global")];
      if (!access.root()) return out;
      const s = state();
      if (locked(s)) return out;
      out.push(
        ...missingFor(
          access.collection()?.vars as VarsDraft | undefined,
          { kind: "collection" },
          "Collection",
        ),
      );
      for (const f of access.folders()) {
        out.push(
          ...missingFor(
            f.vars as VarsDraft | undefined,
            { kind: "folder", id: f.id },
            `Folder › ${f.name}`,
          ),
        );
      }
      const env = s.active ? s.envs.get(s.active)?.env : undefined;
      if (env) {
        out.push(
          ...missingFor(env.vars, { kind: "environment", id: env.id }, `Environment › ${env.name}`),
        );
      }
      return out;
    },
  };

  /** Test helpers. */
  const external = {
    calls,
    /** Reads a stored secret directly (for assertions). */
    storedSecret(scope: SecretScope, name: string, backend?: SecretBackend): string | undefined {
      const key = `${scopeKey(scope)}/${name}`;
      if (scope.kind === "global") return globalSecrets.get(key);
      const s = state();
      return s.stores[backend ?? s.backend].get(key);
    },
    /** Edits an environment file "on disk" and emits `environmentsChanged`. */
    externalEditEnvironment(id: ItemId, patch: Partial<EnvironmentDraft>) {
      const e = state().envs.get(id);
      if (!e) throw new Error("no such environment");
      e.env = { ...e.env, ...patch };
      changed();
    },
    environment(id: ItemId): EnvironmentDraft | undefined {
      return state().envs.get(id)?.env;
    },
    get globals() {
      return globals;
    },
  };

  return { api, external, resolveRequest };
}

function slug(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 60) || "untitled"
  );
}

export type MockVars = ReturnType<typeof createMockVars>;
