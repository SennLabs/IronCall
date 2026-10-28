/**
 * In-memory auth inheritance, body files, network settings and cookies for the mock
 * backend (contract: `src-tauri/src/network.rs`). A stand-in for tests and browser dev.
 */

import { bodyFilePaths } from "../model/bodyFiles";
import type { KeyValueRow, RequestDraft } from "./api";
import type { PreviewHeader } from "./bindings";
import type {
  AuthDraft,
  BodyFileStatus,
  CaCertInfo,
  CookieRow,
  EffectiveAuthDraft,
  NetworkSettings,
  PickedFile,
  ProxySettings,
} from "./networkTypes";
import type { CollectionDraft, FolderDraft, ItemId } from "./workspaceTypes";

export interface MockNetworkSeed {
  proxy?: Partial<ProxySettings>;
  /** What `environmentProxy` reports. */
  environmentProxy?: string | null;
  caCerts?: CaCertInfo[];
  cookies?: CookieRow[];
  /** Files on "disk" by path (relative paths are inside the workspace). */
  files?: Record<string, { size: number; exists?: boolean }>;
}

/** What the network layer needs from the mock workspace. */
export interface MockNetworkWorkspace {
  root(): string | null;
  collection(): CollectionDraft | null;
  folderChain(id: ItemId): FolderDraft[];
  hasRequest(id: ItemId): boolean;
}

const clone = <T>(v: T): T => structuredClone(v);

export const isOutsideWorkspace = (path: string) =>
  path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path);

function caInfo(path: string): CaCertInfo {
  return {
    path,
    subject: `CN=${
      path
        .split(/[\\/]/)
        .pop()
        ?.replace(/\.(pem|crt|der)$/, "") ?? "CA"
    }`,
    notAfter: "2030-01-01T00:00:00Z",
    count: 1,
    error: null,
  };
}

const isSet = (auth: AuthDraft | undefined): auth is AuthDraft => !!auth && auth.type !== "inherit";

export function createMockNetwork(ws: MockNetworkWorkspace, seed: MockNetworkSeed = {}) {
  const proxy: ProxySettings = {
    mode: "environment",
    url: null,
    username: null,
    hasPassword: false,
    noProxy: [],
    ...seed.proxy,
  };
  let password: string | null = null;
  let environmentProxy = seed.environmentProxy ?? null;
  let globalCas: CaCertInfo[] = clone(seed.caCerts ?? []);
  const workspaceCas = new Map<string, CaCertInfo[]>();
  let cookies: CookieRow[] = clone(seed.cookies ?? []);
  const files = new Map(Object.entries(seed.files ?? {}));
  const allowed = new Set<string>();
  const pickedBodyFiles: (PickedFile | null)[] = [];
  const pickedCas: (CaCertInfo | null)[] = [];
  const known = new Map<string, CaCertInfo>();
  const calls: { op: string; args: unknown[] }[] = [];

  const effectiveAuth = (request: RequestDraft): EffectiveAuthDraft => {
    if (isSet(request.auth)) return { auth: clone(request.auth), origin: { kind: "request" } };
    if (ws.hasRequest(request.id)) {
      const chain = ws.folderChain(request.id);
      for (const folder of [...chain].reverse()) {
        if (isSet(folder.auth)) {
          return {
            auth: clone(folder.auth),
            origin: { kind: "folder", id: folder.id, name: folder.name },
          };
        }
      }
      const c = ws.collection();
      if (isSet(c?.auth)) return { auth: clone(c.auth), origin: { kind: "collection" } };
    }
    return { auth: { type: "none" }, origin: { kind: "none" } };
  };

  const fileStatus = (path: string): BodyFileStatus => {
    const outside = isOutsideWorkspace(path);
    const f = files.get(path);
    const exists = f ? f.exists !== false : !outside;
    return {
      path,
      exists,
      size: exists ? (f?.size ?? 1024) : null,
      outsideWorkspace: outside,
      allowed: !outside || allowed.has(path),
    };
  };

  const settings = (): NetworkSettings => ({
    proxy: { ...clone(proxy), hasPassword: password !== null },
    caCerts: clone(globalCas),
    environmentProxy,
  });

  const infoFor = (path: string) => known.get(path) ?? caInfo(path);

  const api = {
    async authEffective(request: RequestDraft) {
      calls.push({ op: "authEffective", args: [clone(request)] });
      return effectiveAuth(request);
    },
    async pickBodyFile() {
      const picked = pickedBodyFiles.shift() ?? null;
      if (picked) {
        allowed.add(picked.path);
        if (!files.has(picked.path)) files.set(picked.path, { size: picked.size });
      }
      return picked;
    },
    async bodyFilesCheck(request: RequestDraft) {
      return bodyFilePaths(request).map(fileStatus);
    },
    async networkSettingsGet() {
      return settings();
    },
    async networkSettingsSet(next: ProxySettings, caPaths: string[]) {
      calls.push({ op: "networkSettingsSet", args: [clone(next), [...caPaths]] });
      if (next.mode === "manual" && !next.url?.trim()) throw new Error("enter a proxy URL");
      Object.assign(proxy, clone(next));
      globalCas = caPaths.map(infoFor);
      return settings();
    },
    async proxyPasswordSet(value: string | null) {
      calls.push({ op: "proxyPasswordSet", args: [value === null ? null : "••••"] });
      password = value;
    },
    async pickCaCert() {
      const picked = pickedCas.shift() ?? null;
      if (picked) known.set(picked.path, picked);
      return picked;
    },
    async workspaceCaCertsGet() {
      const root = ws.root();
      if (!root) throw new Error("no workspace is open");
      return clone(workspaceCas.get(root) ?? []);
    },
    async workspaceCaCertsSet(paths: string[]) {
      const root = ws.root();
      if (!root) throw new Error("no workspace is open");
      calls.push({ op: "workspaceCaCertsSet", args: [[...paths]] });
      workspaceCas.set(root, paths.map(infoFor));
      return clone(workspaceCas.get(root) ?? []);
    },
    async cookiesList() {
      return clone(cookies);
    },
    async cookieSet(cookie: CookieRow) {
      calls.push({ op: "cookieSet", args: [clone(cookie)] });
      if (!cookie.name.trim() || !cookie.domain.trim())
        throw new Error("a cookie needs a name and domain");
      const same = (c: CookieRow) =>
        c.domain === cookie.domain && c.path === cookie.path && c.name === cookie.name;
      cookies = [...cookies.filter((c) => !same(c)), clone(cookie)];
    },
    async cookieDelete(domain: string, path: string, name: string) {
      calls.push({ op: "cookieDelete", args: [domain, path, name] });
      cookies = cookies.filter((c) => !(c.domain === domain && c.path === path && c.name === name));
    },
    async cookiesClear() {
      calls.push({ op: "cookiesClear", args: [] });
      cookies = [];
    },
  };

  /** Auth and default-header rows for the mock preview. */
  const previewExtras = (request: RequestDraft, user: KeyValueRow[]): PreviewHeader[] => {
    const overridden = (name: string) =>
      user.some((h) => h.enabled !== false && h.name.toLowerCase() === name.toLowerCase());
    const rows: PreviewHeader[] = [];
    if (ws.hasRequest(request.id)) {
      const defaults = new Map<string, PreviewHeader>();
      const add = (headers: KeyValueRow[] | undefined, origin: string) => {
        for (const h of headers ?? []) {
          if (h.enabled === false || !h.name) continue;
          defaults.set(h.name.toLowerCase(), {
            name: h.name,
            value: h.value ?? "",
            source: "default",
            effective: !overridden(h.name),
            origin,
          });
        }
      };
      add(ws.collection()?.headers, "Collection");
      for (const f of ws.folderChain(request.id)) add(f.headers, `Folder › ${f.name}`);
      rows.push(...defaults.values());
    }
    const { auth, origin } = effectiveAuth(request);
    const originLabel =
      origin.kind === "folder"
        ? `Folder › ${origin.name}`
        : origin.kind === "collection"
          ? "Collection"
          : "Request";
    const authHeader = (name: string, value: string) =>
      rows.push({ name, value, source: "auth", effective: !overridden(name), origin: originLabel });
    if (auth.type === "basic")
      authHeader("Authorization", `Basic ${btoa(`${auth.username ?? ""}:${auth.password ?? ""}`)}`);
    if (auth.type === "bearer") authHeader("Authorization", `Bearer ${auth.token ?? ""}`);
    if (auth.type === "apiKey" && auth.in !== "query" && auth.key)
      authHeader(auth.key, auth.value ?? "");
    return rows;
  };

  const external = {
    calls,
    /** Answers for the next `pickBodyFile` calls (`null` = cancelled). */
    queueBodyFile: (file: PickedFile | null) => pickedBodyFiles.push(file),
    /** Answers for the next `pickCaCert` calls. */
    queueCaCert: (info: CaCertInfo | null) => pickedCas.push(info),
    setFile: (path: string, file: { size: number; exists?: boolean }) => files.set(path, file),
    /** Simulates a new app session: picked outside files are no longer allowed. */
    forgetAllowed: () => allowed.clear(),
    addCookie: (cookie: CookieRow) => {
      cookies = [...cookies, clone(cookie)];
    },
    setEnvironmentProxy: (value: string | null) => {
      environmentProxy = value;
    },
    password: () => password,
    fileStatus,
  };

  return { api, external, previewExtras, bodyFilePaths, fileStatus };
}
