/**
 * In-memory history for tests and browser-only dev (PLAN.md Phase 5). Records every
 * mock send, pages newest first, and filters roughly like the backend (substring search
 * over method, URL and name instead of FTS).
 */

import type { Api, ItemId, RequestDraft } from "./api";
import type {
  HistorySettings,
  ResponseBodyInfo,
  ResponseSummary,
  SendError,
  StatusClass,
} from "./bindings";
import type { HistoryEntryDraft } from "./historyTypes";

export interface MockHistorySeed {
  entries?: HistoryEntryDraft[];
  /** Stored body text by entry ID. */
  bodies?: Record<string, string>;
  settings?: HistorySettings;
  /** Per-workspace "store response bodies" flag, by workspace root (default true). */
  storeBodies?: boolean;
}

export interface MockHistoryWorkspace {
  root(): string | null;
  collection(): { id: string; name: string } | null;
}

type HistoryApi = Pick<
  Api,
  | "historyList"
  | "historyGet"
  | "historyOpenBody"
  | "historyDelete"
  | "historyClear"
  | "historySettingsGet"
  | "historySettingsSet"
  | "workspaceHistoryBodiesGet"
  | "workspaceHistoryBodiesSet"
>;

const DEFAULT_SETTINGS: HistorySettings = {
  maxEntries: 10_000,
  maxAgeDays: 90,
  maxTotalMb: 500,
  maxBodyKb: 1024,
};

const clone = <T>(v: T): T => structuredClone(v);

let counter = 0;
/** Increasing, ULID-like IDs so string order is time order. */
export function mockHistoryId(): string {
  counter += 1;
  return `01HIST${String(counter).padStart(20, "0")}`;
}

export function statusClassOf(status: number | null): StatusClass {
  if (status === null) return "failed";
  if (status < 300) return "success";
  if (status < 400) return "redirect";
  if (status < 500) return "clientError";
  return "serverError";
}

export function createMockHistory(
  ws: MockHistoryWorkspace,
  /** The main mock's body store, so opened bodies work with the usual body commands. */
  bodyStore: { put(text: string, kind: ResponseBodyInfo["kind"]): number },
  seed: MockHistorySeed = {},
) {
  let entries: HistoryEntryDraft[] = clone(seed.entries ?? []);
  const bodies = new Map(Object.entries(seed.bodies ?? {}));
  let settings = { ...DEFAULT_SETTINGS, ...seed.settings };
  const storeBodies = new Map<string, boolean>();
  const calls: { op: string; args: unknown[] }[] = [];

  const sorted = () => [...entries].sort((a, b) => (a.summary.id < b.summary.id ? 1 : -1));

  const api: HistoryApi = {
    async historyList(query) {
      calls.push({ op: "historyList", args: [clone(query)] });
      const text = query.text.trim().toLowerCase();
      const all = sorted().filter((e) => {
        const s = e.summary;
        if (query.workspace !== null && s.workspace !== query.workspace) return false;
        if (query.methods.length && !query.methods.includes(s.method)) return false;
        if (query.status.length && !query.status.includes(statusClassOf(s.status))) return false;
        if (query.before !== null && !(s.id < query.before)) return false;
        if (text && !`${s.method} ${s.url} ${s.requestName}`.toLowerCase().includes(text)) {
          return false;
        }
        return true;
      });
      const limit = Math.max(1, query.limit);
      const page = all.slice(0, limit);
      return {
        entries: page.map((e) => clone(e.summary)),
        next: all.length > limit ? (page[page.length - 1]?.summary.id ?? null) : null,
      };
    },
    async historyGet(id) {
      const e = entries.find((x) => x.summary.id === id);
      if (!e) throw new Error(`no history entry ${id}`);
      return clone(e);
    },
    async historyOpenBody(id) {
      const e = entries.find((x) => x.summary.id === id);
      const text = bodies.get(id);
      if (!e?.response || text === undefined) return null;
      const size = new TextEncoder().encode(text).length;
      const kind = e.response.bodyKind;
      return {
        bodyId: bodyStore.put(text, kind),
        kind,
        contentType: e.response.contentType,
        charset: "utf-8",
        charsetLossy: false,
        size,
        wireSize: size,
        rowCount: text.split("\n").length,
        prettyAvailable: kind === "json" || kind === "xml",
      };
    },
    async historyDelete(id) {
      calls.push({ op: "historyDelete", args: [id] });
      entries = entries.filter((e) => e.summary.id !== id);
      bodies.delete(id);
    },
    async historyClear(workspace) {
      calls.push({ op: "historyClear", args: [workspace] });
      const gone = entries.filter((e) => workspace === null || e.summary.workspace === workspace);
      for (const e of gone) bodies.delete(e.summary.id);
      entries = entries.filter((e) => !gone.includes(e));
    },
    async historySettingsGet() {
      return { ...settings };
    },
    async historySettingsSet(s) {
      calls.push({ op: "historySettingsSet", args: [clone(s)] });
      settings = { ...s };
      // Retention: count only (age and size aren't modelled).
      entries = sorted().slice(0, settings.maxEntries);
    },
    async workspaceHistoryBodiesGet() {
      const root = ws.root();
      if (!root) throw new Error("no workspace is open");
      return storeBodies.get(root) ?? seed.storeBodies ?? true;
    },
    async workspaceHistoryBodiesSet(value) {
      const root = ws.root();
      if (!root) throw new Error("no workspace is open");
      calls.push({ op: "workspaceHistoryBodiesSet", args: [value] });
      storeBodies.set(root, value);
    },
  };

  /** Records a send like the backend does; returns the new entry's ID. */
  const record = (
    request: RequestDraft,
    outcome: { response: ResponseSummary; bodyText?: string } | { error: SendError },
  ): string => {
    const id = mockHistoryId();
    const collection = ws.collection();
    const workspace: ItemId | null = collection?.id ?? null;
    const response = "response" in outcome ? outcome.response : null;
    const keepBody =
      response?.body.bodyId !== null &&
      "bodyText" in outcome &&
      outcome.bodyText !== undefined &&
      (ws.root() ? (storeBodies.get(ws.root() as string) ?? seed.storeBodies ?? true) : true);
    entries.push({
      summary: {
        id,
        at: new Date().toISOString(),
        workspace,
        workspaceName: collection?.name ?? null,
        requestId: request.id,
        requestName: request.name,
        method: request.method,
        url: response?.request.url ?? request.url,
        status: response?.status ?? null,
        error: "error" in outcome ? outcome.error.kind : null,
        totalMs: response?.timings.totalMs ?? null,
        size: response?.body.size ?? null,
        hasBody: !!keepBody,
      },
      request: clone(request),
      sent: response ? clone(response.request) : null,
      response: response
        ? {
            status: response.status,
            statusText: response.statusText,
            httpVersion: response.httpVersion,
            headers: clone(response.headers),
            timings: clone(response.timings),
            bodyKind: response.body.kind,
            contentType: response.body.contentType,
            size: response.body.size,
            bodyTruncated: false,
          }
        : null,
      errorMessage: "error" in outcome ? outcome.error.message : null,
    });
    if (keepBody && "bodyText" in outcome && outcome.bodyText !== undefined) {
      bodies.set(id, outcome.bodyText);
    }
    return id;
  };

  const external = {
    record,
    entries: () => clone(sorted()),
    calls,
  };

  return { api, external };
}
