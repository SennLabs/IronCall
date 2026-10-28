/**
 * In-memory backend for tests and for running the UI in a plain browser (`npm run dev`
 * without Tauri). Behaviour is deliberately simple; it is not a model of the engine.
 */

import { type Api, type RequestDraft, SendFailure, sendError } from "./api";
import type {
  BodyKind,
  BodyRow,
  GeneralSettings,
  RequestPreview,
  ResponseSummary,
  SendProgress,
  TimelineEntry,
} from "./bindings";
import { createMockHistory, type MockHistorySeed } from "./mockHistory";
import { createMockImport, type MockImportSeed } from "./mockImport";
import { createMockNetwork, type MockNetworkSeed } from "./mockNetwork";
import { createMockRun, type MockRunSeed } from "./mockRun";
import { createMockSettings } from "./mockSettings";
import { createMockVars, type MockVarsSeed } from "./mockVars";
import { createMockWorkspaces, type MockWorkspaceSeed } from "./mockWorkspace";
import type { ScratchLayout, Session } from "./workspaceTypes";

export type { MockHistorySeed } from "./mockHistory";
export type { MockImportSeed } from "./mockImport";
export type { MockNetworkSeed } from "./mockNetwork";
export type { MockRunSeed } from "./mockRun";
export type { MockVarsSeed } from "./mockVars";
export type { MockItem, MockWorkspaceSeed } from "./mockWorkspace";

export interface MockApiOptions {
  /** Builds the response for a send. Defaults to echoing the request as JSON. */
  respond?: (request: RequestDraft) => ResponseSummary | SendFailure;
  /** Delay before a send resolves, in ms. */
  latencyMs?: number;
  /** Workspaces on the "disk", by folder path. */
  workspaces?: Record<string, MockWorkspaceSeed>;
  /** Recent workspace paths (defaults to all seeded workspaces). */
  recent?: string[];
  session?: Session;
  scratch?: RequestDraft[];
  /** Scratch folders and placement. */
  scratchLayout?: ScratchLayout;
  /** Variables, environments and secrets (Phase 3). */
  vars?: MockVarsSeed;
  /** Proxy, CAs, cookies and body files (Phase 4). */
  network?: MockNetworkSeed;
  /** History entries and settings (Phase 5). */
  history?: MockHistorySeed;
  /** cURL and OpenAPI import (Phase 6). */
  import?: MockImportSeed;
  /** Runtime variables (Phase 7). */
  run?: MockRunSeed;
  /** General settings (Phase 8). */
  settings?: Partial<GeneralSettings>;
  /** Delay before a body search resolves, in ms (to exercise cancellation). */
  searchLatencyMs?: number;
  /** What the "Save as" dialog returns (`null` = cancelled). Defaults to `/tmp/<name>`. */
  saveAsPath?: string | null;
}

let nextId = 1;

export function mockDraft(overrides: Partial<RequestDraft> = {}): RequestDraft {
  return {
    schema: "ironcall/request@1",
    id: `01MOCK${String(nextId++).padStart(20, "0")}`,
    name: "Untitled request",
    method: "GET",
    url: "",
    ...overrides,
  };
}

export function mockPreview(request: RequestDraft): RequestPreview {
  const query = (request.params ?? [])
    .filter((p) => p.enabled !== false)
    .map((p) => (p.value ? `${p.name}=${p.value}` : p.name))
    .join("&");
  let url = request.url;
  for (const p of request.pathParams ?? []) {
    url = url.replace(`:${p.name}`, p.value ?? "").replace(`{${p.name}}`, p.value ?? "");
  }
  const body = typeof request.body?.content === "string" ? request.body.content : "";
  const userHeaders = (request.headers ?? []).filter((h) => h.enabled !== false && h.name);
  const has = (name: string) => userHeaders.some((h) => h.name.toLowerCase() === name);
  const derived: RequestPreview["headers"] = [];
  const contentTypes: Record<string, string> = {
    json: "application/json",
    xml: "application/xml",
    text: "text/plain",
  };
  const contentType = contentTypes[request.body?.type ?? "none"];
  if (contentType) {
    derived.push({
      name: "Content-Type",
      value: contentType,
      source: "derived",
      effective: !has("content-type"),
      origin: null,
    });
  }
  return {
    method: request.method,
    url: query ? `${url}?${query}` : url,
    headers: [
      ...userHeaders.map((h) => ({
        name: h.name,
        value: h.value ?? "",
        source: "user" as const,
        effective: true,
        origin: null,
      })),
      ...derived,
      {
        name: "User-Agent",
        value: `IronCall/${__APP_VERSION__}`,
        source: "engine",
        effective: !has("user-agent"),
        origin: null,
      },
      {
        name: "Accept-Encoding",
        value: "gzip, deflate, br, zstd",
        source: "engine",
        effective: !has("accept-encoding"),
        origin: null,
      },
    ],
    bodySize: new TextEncoder().encode(body).length,
  };
}

/** A plausible `curl -v`-style timeline for mock responses. */
export function mockTimeline(request: RequestDraft, status = 200): TimelineEntry[] {
  return [
    { atMs: 0, kind: "info", text: "Resolving api.example.com" },
    { atMs: 3.2, kind: "info", text: "Connected to 203.0.113.7:443" },
    {
      atMs: 15.6,
      kind: "tls",
      text: "TLSv1.3 TLS13_AES_128_GCM_SHA256, ALPN h2\nCertificate: CN=api.example.com",
    },
    {
      atMs: 16,
      kind: "requestHeaders",
      text: `${request.method} ${request.url} HTTP/1.1\nuser-agent: IronCall/${__APP_VERSION__}\naccept: */*`,
    },
    {
      atMs: 57.7,
      kind: "responseHeaders",
      text: `HTTP/1.1 ${status} OK\ncontent-type: application/json`,
    },
    { atMs: 63.2, kind: "responseBody", text: "Received 42 bytes" },
  ];
}

export function mockResponse(
  request: RequestDraft,
  text: string,
  overrides: Partial<ResponseSummary> = {},
): ResponseSummary & { mockText: string } {
  const size = new TextEncoder().encode(text).length;
  return {
    sendId: "",
    status: 200,
    statusText: "OK",
    httpVersion: "HTTP/1.1",
    headers: [{ name: "content-type", value: "application/json" }],
    body: {
      bodyId: text ? nextId++ : null,
      kind: text ? "json" : "empty",
      contentType: "application/json",
      charset: "utf-8",
      charsetLossy: false,
      size,
      wireSize: size,
      rowCount: text.split("\n").length,
      prettyAvailable: !!text,
    },
    timings: {
      dns: { kind: "measured", ms: 3.2 },
      connect: { kind: "measured", ms: 5.1 },
      tls: { kind: "measured", ms: 12.4 },
      ttfbMs: 41.7,
      downloadMs: 0.8,
      totalMs: 63.2,
    },
    redirects: [],
    request: mockPreview(request),
    connectionReused: false,
    tls: {
      version: "TLSv1.3",
      cipher: "TLS13_AES_128_GCM_SHA256",
      alpn: "h2",
      certificate: {
        subject: "CN=api.example.com",
        issuer: "CN=Example CA",
        notBefore: "2026-01-01T00:00:00Z",
        notAfter: "2027-01-01T00:00:00Z",
        sans: ["api.example.com"],
        chainLength: 2,
      },
      verified: true,
    },
    informational: [],
    timeline: mockTimeline(request),
    historyId: null,
    checks: null,
    ...overrides,
    mockText: text,
  };
}

export type MockApi = Api & {
  bodies: Map<number, string>;
  emitProgress: (p: SendProgress) => void;
  /** Dialog answers, call log and "external program" helpers (see `mockWorkspace.ts`). */
  disk: ReturnType<typeof createMockWorkspaces>["external"];
  /** Variables/secrets test helpers (see `mockVars.ts`). */
  vars: ReturnType<typeof createMockVars>["external"];
  /** Network, files and cookies test helpers (see `mockNetwork.ts`). */
  network: ReturnType<typeof createMockNetwork>["external"];
  /** History test helpers (see `mockHistory.ts`). */
  history: ReturnType<typeof createMockHistory>["external"];
  /** Import/export test helpers (see `mockImport.ts`). */
  imports: ReturnType<typeof createMockImport>["external"];
  /** Runtime variable and run helpers (see `mockRun.ts`). */
  run: ReturnType<typeof createMockRun>["external"];
  /** Settings helpers (see `mockSettings.ts`). */
  settings: ReturnType<typeof createMockSettings>["external"];
  /** Calls to body helpers (`bodyPretty`, `bodySaveAs`, `searchBody`). */
  bodyCalls: { op: string; args: unknown[] }[];
};

export function createMockApi(options: MockApiOptions = {}): MockApi {
  const bodies = new Map<number, string>();
  const cancelled = new Set<string>();
  const listeners = new Set<(p: SendProgress) => void>();
  const latency = options.latencyMs ?? 0;

  const rowsOf = (id: number): BodyRow[] =>
    (bodies.get(id) ?? "").split("\n").map((text, line) => ({ line, continuation: false, text }));

  const respond =
    options.respond ??
    ((request: RequestDraft) =>
      mockResponse(request, JSON.stringify({ method: request.method, url: request.url }, null, 2)));

  const workspaces = createMockWorkspaces(options.workspaces, {
    session: options.session,
    scratch: options.scratch,
    scratchLayout: options.scratchLayout,
    recent: options.recent,
  });

  const vars = createMockVars(workspaces.internals, options.vars);
  const network = createMockNetwork(workspaces.internals, options.network);
  const bodyKinds = new Map<number, BodyKind>();
  const bodyCalls: { op: string; args: unknown[] }[] = [];
  const putBody = (text: string, kind: BodyKind) => {
    const id = nextId++;
    bodies.set(id, text);
    bodyKinds.set(id, kind);
    return id;
  };
  const history = createMockHistory(workspaces.internals, { put: putBody }, options.history);
  /** Latest search generation per body; older searches resolve as cancelled. */
  const searches = new Map<number, number>();

  const preview = (request: RequestDraft, reveal: boolean): RequestPreview => {
    const resolved = vars.resolveRequest(request, reveal);
    const base = mockPreview(resolved);
    const extras = network.previewExtras(resolved, resolved.headers ?? []);
    const user = base.headers.filter((h) => h.source === "user");
    return {
      ...base,
      headers: [...user, ...extras, ...base.headers.filter((h) => h.source !== "user")],
    };
  };

  // OpenAPI import writes through the same mock operations the UI uses.
  const imports = createMockImport(() => mock, options.import);
  const run = createMockRun(() => mock, options.run);
  const settings = createMockSettings(options.settings);

  const mock: MockApi = {
    ...workspaces.api,
    ...vars.api,
    ...network.api,
    ...history.api,
    ...imports.api,
    ...run.api,
    ...settings.api,
    async curlExport(request, dialect, mode) {
      // Variables mode exports as authored; the others resolve (masking secrets unless
      // `resolved`), like the backend.
      const target =
        mode === "variables" ? request : vars.resolveRequest(request, mode === "resolved");
      return imports.api.curlExport(target, dialect, mode);
    },
    imports: imports.external,
    run: run.external,
    settings: settings.external,
    history: history.external,
    bodyCalls,
    disk: workspaces.external,
    vars: vars.external,
    network: network.external,
    bodies,
    emitProgress: (p) => {
      for (const l of listeners) l(p);
    },
    async workspaceCreateSample(path) {
      // A small stand-in for the backend's sample (see `ironcall-store/src/sample.rs`).
      const info = await mock.workspaceCreate(path, "IronCall sample");
      for (const [index, [folder, requests]] of (
        [
          ["Basics", ["Echo a GET", "Send JSON", "Query with JSONPath"]],
          ["Chaining", ["Get a token", "Call with the token"]],
        ] as const
      ).entries()) {
        const f = await mock.itemCreate(info.id, "folder", folder, index);
        for (const [i, name] of requests.entries()) await mock.itemCreate(f.id, "request", name, i);
      }
      return (await mock.workspaceCurrent()) ?? info;
    },
    newScratchRequest: async () => mockDraft(),
    previewRequest: async (request, revealSecrets = false) => preview(request, revealSecrets),
    async sendRequest(sendId, request) {
      const emit = (p: Omit<SendProgress, "sendId">) => {
        for (const l of listeners) l({ sendId, ...p });
      };
      emit({ stage: "connecting", bytes: 0, uploaded: 0, uploadTotal: null });
      const fileStatuses = network.bodyFilePaths(request).map(network.fileStatus);
      const blocked = fileStatuses.find((f) => !f.exists || !f.allowed);
      if (blocked) {
        throw new SendFailure(
          sendError(
            "invalidRequest",
            blocked.exists
              ? `${blocked.path} is outside the workspace; re-select it to allow it this session`
              : `${blocked.path} doesn't exist`,
          ),
        );
      }
      const uploadTotal = fileStatuses.reduce((n, f) => n + (f.size ?? 0), 0);
      if (uploadTotal > 0) {
        emit({ stage: "uploading", bytes: 0, uploaded: Math.floor(uploadTotal / 2), uploadTotal });
      }
      await new Promise((r) => setTimeout(r, latency));
      if (cancelled.has(sendId)) {
        const error = sendError("cancelled", "Request cancelled");
        const historyId = history.external.record(request, { error });
        throw new SendFailure({ ...error, historyId });
      }
      const result = respond(vars.resolveRequest(request, true));
      if (result instanceof SendFailure) {
        const historyId = history.external.record(request, { error: result.error });
        throw new SendFailure({ ...result.error, historyId });
      }
      const { mockText, ...summary } = result as ResponseSummary & { mockText?: string };
      if (summary.body.bodyId !== null && mockText !== undefined) {
        bodies.set(summary.body.bodyId, mockText);
        bodyKinds.set(summary.body.bodyId, summary.body.kind);
      }
      const historyId = history.external.record(request, {
        response: summary,
        bodyText: mockText,
      });
      run.applyChecks(summary.checks);
      return { ...summary, sendId, historyId };
    },
    async cancelSend(sendId) {
      cancelled.add(sendId);
    },
    async getBodyText(bodyId, pretty) {
      const text = bodies.get(bodyId);
      if (text === undefined) throw new Error("unknown body");
      if (!pretty) return text;
      try {
        return JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        return text;
      }
    },
    async getBodyLines(bodyId, from, count) {
      return rowsOf(bodyId).slice(from, from + count);
    },
    async searchBody(bodyId, query, fromOffset, ignoreCase, direction = "forward") {
      bodyCalls.push({
        op: "searchBody",
        args: [bodyId, query, fromOffset, ignoreCase, direction],
      });
      const generation = (searches.get(bodyId) ?? 0) + 1;
      searches.set(bodyId, generation);
      if (options.searchLatencyMs) {
        await new Promise((r) => setTimeout(r, options.searchLatencyMs));
      }
      if (searches.get(bodyId) !== generation) return { hit: null, cancelled: true };
      const text = bodies.get(bodyId) ?? "";
      const hay = ignoreCase ? text.toLowerCase() : text;
      const needle = ignoreCase ? query.toLowerCase() : query;
      if (!needle) return { hit: null, cancelled: false };
      // Backward: the last match *ending* before `fromOffset`, like the backend.
      const offset =
        direction === "forward"
          ? hay.indexOf(needle, fromOffset)
          : fromOffset - needle.length < 0
            ? -1
            : hay.lastIndexOf(needle, fromOffset - needle.length);
      if (offset < 0) return { hit: null, cancelled: false };
      const before = text.slice(0, offset);
      const row = before.split("\n").length - 1;
      return {
        hit: { row, offset, column: offset - (before.lastIndexOf("\n") + 1) },
        cancelled: false,
      };
    },
    async bodyPretty(bodyId) {
      bodyCalls.push({ op: "bodyPretty", args: [bodyId] });
      const text = bodies.get(bodyId);
      if (text === undefined) throw new Error("unknown body");
      let pretty = text;
      try {
        pretty = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        // Not JSON: keep as is (XML isn't reformatted by the mock).
      }
      const id = putBody(pretty, bodyKinds.get(bodyId) ?? "text");
      return {
        bodyId: id,
        size: new TextEncoder().encode(pretty).length,
        rowCount: pretty.split("\n").length,
      };
    },
    async bodySaveAs(bodyId, suggestedName) {
      bodyCalls.push({ op: "bodySaveAs", args: [bodyId, suggestedName] });
      if (!bodies.has(bodyId)) throw new Error("unknown body");
      return options.saveAsPath === undefined ? `/tmp/${suggestedName}` : options.saveAsPath;
    },
    async closeBody(bodyId) {
      bodies.delete(bodyId);
    },
    recentSends: async () => [],
    async onSendProgress(cb) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
  return mock;
}
