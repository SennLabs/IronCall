/**
 * In-memory cURL and OpenAPI import/export for tests and browser-only dev (PLAN.md
 * Phase 6). Deliberately small: the real parsers live in `ironcall-import`. The OpenAPI
 * mock only reads JSON specs, and only `info`, `servers`/`host`, `tags` and `paths`.
 */

import { applyUrlText, syncPathParams } from "../model/url";
import type { Api, ItemId, KeyValueRow, RequestDraft } from "./api";
import type {
  CurlDialect,
  CurlExport,
  CurlParseResultDraft,
  ImportWarning,
  OpenApiImportOptions,
  OpenApiPreview,
  PlanNode,
  SpecVersion,
} from "./importTypes";

export interface MockImportSeed {
  /** Spec text the mock file picker "opens" (`null` = cancelled). */
  pickedSpec?: string | null;
}

type ImportApi = Pick<
  Api,
  | "curlParse"
  | "curlDetect"
  | "curlExport"
  | "openapiPickAndPreview"
  | "openapiPreviewText"
  | "openapiImport"
>;

/** What `openapiImport` needs from the rest of the mock backend. */
type Backend = Pick<
  Api,
  | "itemCreate"
  | "requestLoad"
  | "requestSave"
  | "environmentCreate"
  | "environmentLoad"
  | "environmentSave"
  | "activeEnvironmentSet"
  | "workspaceCurrent"
>;

let nextId = 1;
const newId = () => `01MOCKIMP${String(nextId++).padStart(17, "0")}`;

// ---------------------------------------------------------------------------------
// cURL

/** Splits a shell command into words: POSIX quoting, or Windows cmd `^` escapes. */
export function shellWords(command: string): string[] {
  const cmd = /\^\r?\n|\^"/.test(command);
  const text = cmd
    ? command.replace(/\^\r?\n/g, " ").replace(/\^(.)/g, "$1")
    : command.replace(/\\\r?\n/g, " ");
  const words: string[] = [];
  let word = "";
  let inWord = false;
  let quote: "'" | '"' | null = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i] as string;
    if (quote) {
      if (c === quote) quote = null;
      else if (c === "\\" && quote === '"' && !cmd && i + 1 < text.length) word += text[++i];
      else word += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      inWord = true;
    } else if (/\s/.test(c)) {
      if (inWord) words.push(word);
      word = "";
      inWord = false;
    } else if (c === "\\" && !cmd && i + 1 < text.length) {
      word += text[++i];
      inWord = true;
    } else {
      word += c;
      inWord = true;
    }
  }
  if (inWord) words.push(word);
  return words;
}

const DATA_FLAGS = new Set(["-d", "--data", "--data-raw", "--data-binary", "--data-urlencode"]);
/** Flags the mock doesn't support that take a value (skipped with a warning). */
const UNSUPPORTED_WITH_VALUE = new Set([
  "-o",
  "--output",
  "--cert",
  "--key",
  "--cacert",
  "--connect-timeout",
  "-w",
  "--write-out",
]);

function looksLikeJson(text: string): boolean {
  const t = text.trim();
  if (!(t.startsWith("{") || t.startsWith("["))) return false;
  try {
    JSON.parse(t);
    return true;
  } catch {
    return false;
  }
}

export function mockCurlParse(command: string): CurlParseResultDraft {
  const words = shellWords(command.trim());
  if (!words[0] || !/^curl(\.exe)?$/i.test(words[0])) {
    throw new Error("not a cURL command (it should start with `curl`)");
  }
  const warnings: ImportWarning[] = [];
  const headers: KeyValueRow[] = [];
  const data: string[] = [];
  let method: string | null = null;
  let url = "";
  let getData = false;
  let auth: RequestDraft["auth"];
  const settings: NonNullable<RequestDraft["settings"]> = {};

  for (let i = 1; i < words.length; i++) {
    const w = words[i] as string;
    const value = () => words[++i] ?? "";
    if (w === "-X" || w === "--request") method = value().toUpperCase();
    else if (w === "-H" || w === "--header") {
      const h = value();
      const at = h.indexOf(":");
      if (at > 0)
        headers.push({ name: h.slice(0, at).trim(), value: h.slice(at + 1).trim(), enabled: true });
    } else if (DATA_FLAGS.has(w)) data.push(value());
    else if (w === "--url") url = value();
    else if (w === "-u" || w === "--user") {
      const [username, ...rest] = value().split(":");
      auth = { type: "basic", username: username ?? "", password: rest.join(":") };
    } else if (w === "-A" || w === "--user-agent") {
      headers.push({ name: "User-Agent", value: value(), enabled: true });
    } else if (w === "-b" || w === "--cookie") {
      headers.push({ name: "Cookie", value: value(), enabled: true });
    } else if (w === "-G" || w === "--get") getData = true;
    else if (w === "-k" || w === "--insecure") settings.verifyTls = false;
    else if (w === "-L" || w === "--location") settings.followRedirects = true;
    else if (w === "--compressed") {
      // Decompression is always on.
    } else if (UNSUPPORTED_WITH_VALUE.has(w)) {
      value();
      warnings.push({ message: `${w} is not supported and was ignored`, location: w });
    } else if (w.startsWith("-")) {
      warnings.push({ message: `${w} is not supported and was ignored`, location: w });
    } else if (!url) url = w;
  }

  const draft: RequestDraft = {
    schema: "ironcall/request@1",
    id: newId(),
    name: "",
    method: method ?? (data.length && !getData ? "POST" : "GET"),
    url: "",
  };
  const split = applyUrlText(url, []);
  draft.url = split.url;
  if (getData && data.length) {
    draft.params = [...split.params, ...applyUrlText(`?${data.join("&")}`, []).params];
  } else if (split.params.length) {
    draft.params = split.params;
  }
  const pathParams = syncPathParams([], draft.url);
  if (pathParams.length) draft.pathParams = pathParams;
  if (headers.length) draft.headers = headers;
  if (auth) draft.auth = auth;
  if (Object.keys(settings).length) draft.settings = settings;
  if (data.length && !getData) {
    const body = data.join("&");
    draft.body = looksLikeJson(body)
      ? { type: "json", content: body }
      : { type: "text", content: body };
  }
  const path = draft.url.replace(/^[a-z]+:\/\/[^/]*/i, "") || "/";
  draft.name = `${draft.method} ${path}`;
  return { request: draft, warnings };
}

const quotePosix = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;
const quotePs = (s: string) => `'${s.replace(/'/g, "''")}'`;

export function mockCurlExport(request: RequestDraft, dialect: CurlDialect): CurlExport {
  const q = dialect === "posix" ? quotePosix : quotePs;
  const query = (request.params ?? [])
    .filter((p) => p.enabled !== false)
    .map((p) => (p.value ? `${p.name}=${p.value}` : p.name))
    .join("&");
  const url = query ? `${request.url}?${query}` : request.url;
  const parts = [dialect === "posix" ? "curl" : "curl.exe"];
  if (request.method !== "GET") parts.push(`-X ${request.method}`);
  parts.push(q(url));
  for (const h of request.headers ?? []) {
    if (h.enabled !== false && h.name) parts.push(`-H ${q(`${h.name}: ${h.value ?? ""}`)}`);
  }
  const body = request.body;
  if (body && typeof body.content === "string" && body.content) {
    parts.push(`--data-raw ${q(body.content)}`);
  }
  const join = dialect === "posix" ? " \\\n  " : " `\n  ";
  return { command: parts.join(join), warnings: [] };
}

// ---------------------------------------------------------------------------------
// OpenAPI

interface Plan {
  title: string;
  /** Folder name → requests ("" = top level). */
  groups: Map<string, RequestDraft[]>;
  servers: string[];
}

const METHODS = ["get", "post", "put", "patch", "delete", "head", "options"];

function planFrom(text: string): { preview: OpenApiPreview; plan: Plan } {
  let spec: Record<string, unknown>;
  try {
    spec = JSON.parse(text) as Record<string, unknown>;
  } catch {
    throw new Error("the mock only reads JSON specs");
  }
  const version: SpecVersion =
    typeof spec.swagger === "string"
      ? "swagger2"
      : String(spec.openapi ?? "").startsWith("3.1")
        ? "openApi31"
        : "openApi30";
  if (!spec.swagger && !spec.openapi) throw new Error("not an OpenAPI or Swagger document");
  const info = (spec.info ?? {}) as { title?: string; description?: string };
  const servers =
    version === "swagger2"
      ? spec.host
        ? [`https://${spec.host}${(spec.basePath as string) ?? ""}`]
        : []
      : ((spec.servers as { url: string }[] | undefined) ?? []).map((s) => s.url);
  const warnings: ImportWarning[] = [];
  const groups = new Map<string, RequestDraft[]>();
  for (const [path, ops] of Object.entries(
    (spec.paths ?? {}) as Record<string, Record<string, unknown>>,
  )) {
    for (const [method, op] of Object.entries(ops)) {
      if (!METHODS.includes(method)) continue;
      const o = op as { summary?: string; operationId?: string; tags?: string[] };
      const folder = o.tags?.[0] ?? "";
      const url = `{{baseUrl}}${path.replace(/\{([^}]+)\}/g, ":$1")}`;
      const draft: RequestDraft = {
        schema: "ironcall/request@1",
        id: newId(),
        name: o.summary ?? o.operationId ?? `${method.toUpperCase()} ${path}`,
        method: method.toUpperCase(),
        url,
      };
      const list = groups.get(folder) ?? [];
      list.push(draft);
      groups.set(folder, list);
      if (JSON.stringify(op).includes('"$ref":"http')) {
        warnings.push({
          message: "external $ref not followed",
          location: `${method.toUpperCase()} ${path}`,
        });
      }
    }
  }
  const tree: PlanNode[] = [];
  const leaf = (r: RequestDraft): PlanNode => ({
    name: r.name,
    method: r.method,
    url: r.url,
    children: [],
  });
  for (const [folder, requests] of groups) {
    if (folder) tree.push({ name: folder, method: null, url: null, children: requests.map(leaf) });
    else tree.push(...requests.map(leaf));
  }
  const requestCount = [...groups.values()].reduce((n, l) => n + l.length, 0);
  const preview: OpenApiPreview = {
    planId: newId(),
    version,
    title: info.title ?? "Imported API",
    description: info.description ?? null,
    tree,
    requestCount,
    servers,
    auth: [],
    warnings,
  };
  return { preview, plan: { title: preview.title, groups, servers } };
}

export function createMockImport(backend: () => Backend, seed: MockImportSeed = {}) {
  const plans = new Map<string, Plan>();
  const calls: { op: string; args: unknown[] }[] = [];
  const state = { pickedSpec: seed.pickedSpec ?? null };

  const preview = (text: string) => {
    const { preview, plan } = planFrom(text);
    plans.clear();
    plans.set(preview.planId, plan);
    return preview;
  };

  const api: ImportApi = {
    async curlParse(command) {
      calls.push({ op: "curlParse", args: [command] });
      return mockCurlParse(command);
    },
    async curlDetect(text) {
      return /^\s*curl(\.exe)?\s/i.test(text);
    },
    async curlExport(request, dialect, mode) {
      calls.push({ op: "curlExport", args: [structuredClone(request), dialect, mode] });
      return mockCurlExport(request, dialect);
    },
    async openapiPickAndPreview() {
      calls.push({ op: "openapiPickAndPreview", args: [] });
      return state.pickedSpec === null ? null : preview(state.pickedSpec);
    },
    async openapiPreviewText(text) {
      calls.push({ op: "openapiPreviewText", args: [text] });
      return preview(text);
    },
    async openapiImport(options: OpenApiImportOptions) {
      calls.push({ op: "openapiImport", args: [structuredClone(options)] });
      const plan = plans.get(options.planId);
      if (!plan) throw new Error("this import preview has expired; preview the spec again");
      const b = backend();
      if (!(await b.workspaceCurrent())) throw new Error("no workspace is open");
      let foldersCreated = 0;
      let requestsCreated = 0;
      let top: ItemId | null = null;
      let parent = options.parent;
      if (options.wrapInFolder) {
        top = (await b.itemCreate(parent, "folder", plan.title, null)).id;
        parent = top;
        foldersCreated++;
      }
      for (const [folder, requests] of plan.groups) {
        let target = parent;
        if (folder) {
          target = (await b.itemCreate(parent, "folder", folder, null)).id;
          foldersCreated++;
        }
        for (const r of requests) {
          const node = await b.itemCreate(target, "request", r.name, null);
          const loaded = await b.requestLoad(node.id);
          await b.requestSave(
            { ...loaded.request, method: r.method, url: r.url },
            loaded.hash,
            false,
          );
          requestsCreated++;
        }
      }
      let environment: ItemId | null = null;
      if (options.createEnvironment) {
        const env = await b.environmentCreate(plan.title);
        environment = env.id;
        const loaded = await b.environmentLoad(env.id);
        await b.environmentSave(
          {
            ...loaded.environment,
            vars: { ...loaded.environment.vars, baseUrl: plan.servers[0] ?? "" },
          },
          loaded.hash,
        );
        if (options.activateEnvironment) await b.activeEnvironmentSet(env.id);
      }
      plans.delete(options.planId);
      return { folder: top, requestsCreated, foldersCreated, environment, warnings: [] };
    },
  };

  /** Test helpers. */
  const external = {
    calls,
    /** Sets what the mock spec file picker returns (`null` = cancelled). */
    setPickedSpec(text: string | null) {
      state.pickedSpec = text;
    },
  };

  return { api, external };
}
