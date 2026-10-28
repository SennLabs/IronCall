/**
 * The only module components use to talk to the backend.
 *
 * Wraps the generated bindings: commands that return `{status: "ok"|"error"}` are
 * unwrapped and errors are thrown, so callers use plain async/await. Tests and
 * browser-only dev swap the implementation with `setApi` (see `mock.ts`).
 */

import { openUrl } from "@tauri-apps/plugin-opener";
import {
  type AssertOp,
  type AssertTarget,
  type BodyRow,
  type CollectionFile_Deserialize,
  commands,
  type DerivedBody,
  type EnvChoice,
  type EnvironmentFile_Deserialize,
  type ExtractSource,
  events,
  type FolderFile_Deserialize,
  type GeneralSettings,
  type HistoryPage,
  type HistoryQuery,
  type HistorySettings,
  type HttpVersion,
  type RecentSend,
  type RequestFile_Deserialize,
  type RequestFile_Serialize,
  type RequestPreview,
  type ResponseBodyInfo,
  type ResponseSummary,
  type RunItem,
  type RunOptions,
  type RunProgress,
  type RunReport,
  type RunTarget,
  type RuntimeVarView,
  type SearchDirection,
  type SearchResult,
  type SendError,
  type SendProgress,
  type Variable_Deserialize,
  type VarsError,
  type WorkspaceError,
} from "./bindings";
import type { BodySearchOutcome, HistoryEntryDraft } from "./historyTypes";
import type {
  CurlDialect,
  CurlExport,
  CurlExportMode,
  CurlParseResultDraft,
  OpenApiImportOptions,
  OpenApiImportResult,
  OpenApiPreview,
} from "./importTypes";
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
import type {
  EnvironmentDraft,
  EnvironmentSummary,
  LoadedEnvironment,
  LoadedGlobals,
  MissingSecret,
  SecretBackend,
  SecretBackendStatus,
  SecretScope,
  VariableContext,
  VarsDraft,
} from "./varsTypes";
import { VarsFailure } from "./varsTypes";
import type {
  CollectionDraft,
  FolderDraft,
  ItemId,
  LoadedCollection,
  LoadedFolder,
  LoadedRequest,
  NewItemKind,
  RecentWorkspace,
  SaveOutcome,
  ScratchFolder,
  ScratchLayout,
  Session,
  TreeNode,
  WorkspaceChanged,
  WorkspaceInfo,
} from "./workspaceTypes";
import { WorkspaceFailure } from "./workspaceTypes";

export * from "./historyTypes";
export * from "./importTypes";
export * from "./networkTypes";
export * from "./runTypes";
export * from "./settingsTypes";
export * from "./varsTypes";
export * from "./workspaceTypes";

/**
 * Keys the UI doesn't know about. They must survive every edit (Phase 2 saves the
 * draft back to disk), so objects are always updated by spreading.
 */
type Open = { [key: string]: unknown };

export type KeyValueRow = { name: string; value?: string; enabled?: boolean } & Open;
export type PathParamRow = { name: string; value?: string } & Open;

export type DraftBody =
  | ({ type: "none" } & Open)
  | ({ type: "json" | "text" | "xml"; content?: string } & Open)
  | ({ type: "formUrlencoded" | "multipart" | "binaryFile" } & Open);

/** A declarative check on the response (PLAN.md Phase 7). */
export type AssertionDraft = {
  target: AssertTarget;
  /** JSONPath, for `body`. */
  path?: string | null;
  /** Header name, for `header`. */
  name?: string | null;
  op: AssertOp;
  /** Typed: `201` and `"201"` differ for body values. Absent for exists/notExists. */
  value?: unknown;
  enabled?: boolean;
} & Open;

/** Saves part of the response into a runtime variable (PLAN.md Phase 7). */
export type ExtractionDraft = {
  var: string;
  from: ExtractSource;
  path?: string | null;
  name?: string | null;
  pattern?: string | null;
  sensitive?: boolean;
} & Open;

export type DraftSettings = {
  timeoutMs?: number | null;
  followRedirects?: boolean | null;
  maxRedirects?: number | null;
  verifyTls?: boolean | null;
  httpVersion?: HttpVersion | null;
  freshConnection?: boolean | null;
  encodeUrl?: boolean | null;
  bypassProxy?: boolean | null;
} & Open;

/**
 * A request as the UI holds and edits it: the same JSON as the backend's `RequestFile`,
 * typed for editing. The generated bindings intersect every model type with a
 * `JsonValue` index signature, which makes them impractical to update in TypeScript,
 * so the UI uses these types and converts only at the IPC boundary.
 */
export type RequestDraft = {
  schema: string;
  id: string;
  name: string;
  description?: string | null;
  method: string;
  url: string;
  pathParams?: PathParamRow[];
  params?: KeyValueRow[];
  /** Request-scope variables (never secret). */
  vars?: VarsDraft;
  headers?: KeyValueRow[];
  /** Absent means `inherit` (PLAN.md Phase 4). */
  auth?: AuthDraft;
  body?: DraftBody;
  extract?: ExtractionDraft[];
  assert?: AssertionDraft[];
  settings?: DraftSettings;
} & Open;

/** A `SendError` for failures that happen in the UI (no timeline, no history entry). */
export function sendError(
  kind: SendError["kind"],
  message: string,
  detail: string | null = null,
): SendError {
  return { kind, message, detail, timeline: [], historyId: null };
}

/** Thrown by `sendRequest` / `previewRequest` with the backend's structured error. */
export class SendFailure extends Error {
  constructor(readonly error: SendError) {
    super(error.message);
    this.name = "SendFailure";
  }
}

export interface Api {
  newScratchRequest(): Promise<RequestDraft>;
  /** The request as it would be sent; secret values are masked unless `revealSecrets`. */
  previewRequest(request: RequestDraft, revealSecrets?: boolean): Promise<RequestPreview>;
  sendRequest(sendId: string, request: RequestDraft): Promise<ResponseSummary>;
  cancelSend(sendId: string): Promise<void>;
  getBodyText(bodyId: number, pretty: boolean): Promise<string>;
  getBodyLines(bodyId: number, from: number, count: number): Promise<BodyRow[]>;
  /**
   * Finds the first match starting at or after byte `fromOffset` (forward), or the last
   * match ending before it (backward). Case-insensitive search folds Unicode. Starting a
   * new search on a body cancels the previous one, which resolves with `cancelled: true`.
   */
  searchBody(
    bodyId: number,
    query: string,
    fromOffset: number,
    ignoreCase: boolean,
    direction?: SearchDirection,
  ): Promise<BodySearchOutcome>;
  /** Pretty-prints a JSON/XML body of any size into a new body (close it when done). */
  bodyPretty(bodyId: number): Promise<DerivedBody>;
  /** Native "Save as" for a response body; the saved path, or `null` if cancelled. */
  bodySaveAs(bodyId: number, suggestedName: string): Promise<string | null>;
  closeBody(bodyId: number): Promise<void>;
  recentSends(): Promise<RecentSend[]>;
  /** Subscribes to send progress; returns an unsubscribe function. */
  onSendProgress(cb: (progress: SendProgress) => void): Promise<() => void>;

  // Workspaces (Phase 2). Failures throw `WorkspaceFailure`.
  workspaceCreate(path: string, name: string): Promise<WorkspaceInfo>;
  workspaceOpen(path: string): Promise<WorkspaceInfo>;
  /** Creates the first-run sample workspace (calls the built-in echo server) and opens it. */
  workspaceCreateSample(path: string): Promise<WorkspaceInfo>;
  workspaceClose(): Promise<void>;
  workspaceCurrent(): Promise<WorkspaceInfo | null>;
  recentWorkspaces(): Promise<RecentWorkspace[]>;
  forgetRecentWorkspace(path: string): Promise<void>;

  itemCreate(
    parent: ItemId,
    kind: NewItemKind,
    name: string,
    index: number | null,
  ): Promise<TreeNode>;
  itemRename(id: ItemId, name: string): Promise<void>;
  itemRenameFileToMatch(id: ItemId): Promise<void>;
  itemDuplicate(id: ItemId): Promise<TreeNode>;
  itemDelete(id: ItemId): Promise<void>;
  itemMove(id: ItemId, parent: ItemId, index: number): Promise<void>;
  itemReveal(id: ItemId): Promise<void>;
  itemOpenExternal(id: ItemId): Promise<void>;

  requestLoad(id: ItemId): Promise<LoadedRequest>;
  requestSave(request: RequestDraft, baseHash: string | null, force: boolean): Promise<SaveOutcome>;
  folderLoad(id: ItemId): Promise<LoadedFolder>;
  folderSave(folder: FolderDraft, baseHash: string | null): Promise<string>;
  collectionLoad(): Promise<LoadedCollection>;
  collectionSave(collection: CollectionDraft, baseHash: string | null): Promise<string>;

  scratchList(): Promise<RequestDraft[]>;
  scratchSave(request: RequestDraft): Promise<void>;
  scratchDelete(id: ItemId): Promise<void>;
  scratchSaveToWorkspace(request: RequestDraft, parent: ItemId): Promise<TreeNode>;
  /** Scratch folders and which folder each scratch request is in. */
  scratchLayout(): Promise<ScratchLayout>;
  scratchSaveLayout(layout: ScratchLayout): Promise<void>;
  /** Adds a scratch folder (at the end) under `parent`, or at the top level. */
  scratchFolderCreate(name: string, parent: ItemId | null): Promise<ScratchFolder>;
  sessionLoad(): Promise<Session>;
  sessionSave(session: Session): Promise<void>;
  onWorkspaceChanged(cb: (change: WorkspaceChanged) => void): Promise<() => void>;

  // Variables, environments and secrets (Phase 3). Failures throw `VarsFailure`.
  variablesFor(request: RequestDraft, reveal: boolean): Promise<VariableContext>;
  environmentsList(): Promise<EnvironmentSummary[]>;
  environmentCreate(name: string): Promise<EnvironmentSummary>;
  environmentDuplicate(id: ItemId): Promise<EnvironmentSummary>;
  environmentRename(id: ItemId, name: string): Promise<void>;
  environmentDelete(id: ItemId): Promise<void>;
  environmentLoad(id: ItemId): Promise<LoadedEnvironment>;
  environmentSave(environment: EnvironmentDraft, baseHash: string | null): Promise<string>;
  activeEnvironmentGet(): Promise<ItemId | null>;
  activeEnvironmentSet(id: ItemId | null): Promise<void>;
  globalsLoad(): Promise<LoadedGlobals>;
  globalsSave(vars: VarsDraft, baseHash: string | null): Promise<string>;
  secretBackendStatus(): Promise<SecretBackendStatus>;
  secretBackendSet(backend: SecretBackend, passphrase: string | null): Promise<SecretBackendStatus>;
  secretsUnlock(passphrase: string): Promise<void>;
  secretSet(scope: SecretScope, name: string, value: string): Promise<void>;
  secretClear(scope: SecretScope, name: string): Promise<void>;
  secretReveal(scope: SecretScope, name: string): Promise<string | null>;
  secretsMissing(): Promise<MissingSecret[]>;

  // Auth, file bodies, network settings and cookies (Phase 4). Failures throw `Error`.
  /** The auth the request will use after inheritance. */
  authEffective(request: RequestDraft): Promise<EffectiveAuthDraft>;
  /** Native file picker for a body file; the file is allowed for this session. */
  pickBodyFile(): Promise<PickedFile | null>;
  /** Status of every file the request would upload. */
  bodyFilesCheck(request: RequestDraft): Promise<BodyFileStatus[]>;
  networkSettingsGet(): Promise<NetworkSettings>;
  networkSettingsSet(proxy: ProxySettings, caPaths: string[]): Promise<NetworkSettings>;
  /** Stores the manual proxy password in the OS keychain; `null` clears it. */
  proxyPasswordSet(password: string | null): Promise<void>;
  /** Native file picker for a CA certificate. */
  pickCaCert(): Promise<CaCertInfo | null>;
  workspaceCaCertsGet(): Promise<CaCertInfo[]>;
  workspaceCaCertsSet(paths: string[]): Promise<CaCertInfo[]>;
  cookiesList(): Promise<CookieRow[]>;
  cookieSet(cookie: CookieRow): Promise<void>;
  cookieDelete(domain: string, path: string, name: string): Promise<void>;
  cookiesClear(): Promise<void>;

  // History (Phase 5). Failures throw `Error`.
  historyList(query: HistoryQuery): Promise<HistoryPage>;
  historyGet(id: string): Promise<HistoryEntryDraft>;
  /** Loads an entry's stored body into the body store; `null` if none was stored. */
  historyOpenBody(id: string): Promise<ResponseBodyInfo | null>;
  historyDelete(id: string): Promise<void>;
  /** Deletes every entry, or only one workspace's. */
  historyClear(workspace: ItemId | null): Promise<void>;
  historySettingsGet(): Promise<HistorySettings>;
  historySettingsSet(settings: HistorySettings): Promise<void>;
  /** Whether the open workspace's response bodies are stored in history. */
  workspaceHistoryBodiesGet(): Promise<boolean>;
  workspaceHistoryBodiesSet(storeBodies: boolean): Promise<void>;

  // cURL and OpenAPI (Phase 6). Failures throw `Error`.
  /** Parses a cURL command (bash/POSIX or Chrome's cmd format) into a new request. */
  curlParse(command: string): Promise<CurlParseResultDraft>;
  /** Cheap check for the URL bar's paste handler. */
  curlDetect(text: string): Promise<boolean>;
  /** A cURL command for a request; `resolved` includes secret values. */
  curlExport(
    request: RequestDraft,
    dialect: CurlDialect,
    mode: CurlExportMode,
  ): Promise<CurlExport>;
  /** Native file picker for a spec, then a preview; `null` if cancelled. */
  openapiPickAndPreview(): Promise<OpenApiPreview | null>;
  openapiPreviewText(text: string): Promise<OpenApiPreview>;
  /** Writes a previewed plan into the open workspace. */
  openapiImport(options: OpenApiImportOptions): Promise<OpenApiImportResult>;

  // Assertions, runtime variables and the collection runner (Phase 7). Failures throw
  // `VarsFailure`.
  /** The requests a run would send, in order. */
  runItems(target: RunTarget): Promise<RunItem[]>;
  /** Runs the requests under `target`; progress arrives through `onRunProgress`. */
  runCollection(
    runId: string,
    target: RunTarget,
    environment: EnvChoice,
    options: RunOptions,
  ): Promise<RunReport>;
  runCancel(runId: string): Promise<void>;
  /** A finished run's results as Markdown; `null` if it's no longer kept. */
  runReportMarkdown(runId: string): Promise<string | null>;
  onRunProgress(cb: (progress: RunProgress) => void): Promise<() => void>;
  /** Session runtime variables; sensitive values masked unless `reveal`. */
  runtimeVars(reveal: boolean): Promise<RuntimeVarView[]>;
  /** Removes one runtime variable, or all with `null`. */
  runtimeVarsClear(name: string | null): Promise<void>;
  /** Saves a runtime variable into the active environment (secret if sensitive). */
  runtimeVarSave(name: string): Promise<void>;

  // General settings (Phase 8). Failures throw `Error`.
  settingsGet(): Promise<GeneralSettings>;
  /** Validates and saves; resolves with what was stored. */
  settingsSet(settings: GeneralSettings): Promise<GeneralSettings>;
  /** Whole-window zoom (1 = 100%). */
  setZoom(scale: number): Promise<void>;
  /** Version, platform, settings summary and recent log lines, scrubbed of secrets. */
  diagnosticInfo(): Promise<string>;
  openLogsFolder(): Promise<void>;
  /** Records an uncaught UI error in the app log (scrubbed by the backend). */
  logFrontendError(message: string, stack: string | null): Promise<void>;

  // Native dialogs.
  /** Folder picker; `null` if cancelled. */
  pickFolder(title: string): Promise<string | null>;
  /** OK/Cancel question; true for OK. */
  confirm(message: string, options: ConfirmOptions): Promise<boolean>;
  /** Three-way question, e.g. Save / Don't save / Cancel. */
  choose(message: string, options: ChooseOptions): Promise<"yes" | "no" | "cancel">;
  /** Opens a link in the system browser. */
  openUrl(url: string): Promise<void>;
  /**
   * Runs `allow` when the user closes the window; the window closes only if it resolves
   * true. Returns an unsubscribe function.
   */
  onCloseRequested(allow: () => Promise<boolean>): Promise<() => void>;
}

export interface ConfirmOptions {
  title: string;
  okLabel: string;
  cancelLabel?: string;
  kind?: "info" | "warning" | "error";
}

export interface ChooseOptions {
  title: string;
  yes: string;
  no: string;
  cancel: string;
  kind?: "info" | "warning" | "error";
}

/**
 * Draft → the backend's `RequestFile` (Deserialize shape). The JSON is identical; only
 * the static type changes. The object is passed through, so unknown keys are kept.
 */
export function toWire(request: RequestDraft): RequestFile_Deserialize {
  return request as unknown as RequestFile_Deserialize;
}

/** The backend's `RequestFile` (Serialize shape) → draft. See `toWire`. */
export function fromWire(request: RequestFile_Serialize): RequestDraft {
  return request as unknown as RequestDraft;
}

/** Maps the backend's `SearchResult` to the UI's `{hit, cancelled}` shape. */
export function toSearchOutcome(result: SearchResult): BodySearchOutcome {
  switch (result.outcome) {
    case "found":
      return { hit: result.hit, cancelled: false };
    case "notFound":
      return { hit: null, cancelled: false };
    case "cancelled":
      return { hit: null, cancelled: true };
  }
}

async function unwrap<T, E>(
  result: Promise<{ status: "ok"; data: T } | { status: "error"; error: E }>,
  toError: (e: E) => Error,
): Promise<T> {
  const r = await result;
  if (r.status === "error") throw toError(r.error);
  return r.data;
}

const sendFailure = (e: SendError) => new SendFailure(e);
const plain = (e: string) => new Error(e);

const workspaceFailure = (e: WorkspaceError) => new WorkspaceFailure(e.message, e.conflict);
const varsFailure = (e: VarsError) => new VarsFailure(e.message, e.locked, e.conflict);

/** Unwraps a variables/secrets command result (draft-typed where files are involved). */
const vr = <T>(
  result: Promise<{ status: "ok"; data: unknown } | { status: "error"; error: VarsError }>,
) => unwrap(result, varsFailure) as Promise<T>;

/** Unwraps a workspace command result into the UI's draft-typed shape (same JSON). */
const ws = <T>(
  result: Promise<{ status: "ok"; data: unknown } | { status: "error"; error: WorkspaceError }>,
) => unwrap(result, workspaceFailure) as Promise<T>;

/** Draft → generated file type (Deserialize shape); the JSON is identical. */
const wire = <T>(file: object) => file as unknown as T;

export const tauriApi: Api = {
  newScratchRequest: async () => fromWire(await commands.newScratchRequest()),
  previewRequest: (request, revealSecrets = false) =>
    unwrap(commands.previewRequest(toWire(request), revealSecrets), sendFailure),
  sendRequest: (sendId, request) =>
    unwrap(commands.sendRequest(sendId, toWire(request)), sendFailure),
  cancelSend: (sendId) => commands.cancelSend(sendId),
  getBodyText: (bodyId, pretty) => unwrap(commands.getBodyText(bodyId, pretty), plain),
  getBodyLines: (bodyId, from, count) => unwrap(commands.getBodyLines(bodyId, from, count), plain),
  searchBody: async (bodyId, query, fromOffset, ignoreCase, direction = "forward") =>
    toSearchOutcome(
      await unwrap(commands.searchBody(bodyId, query, fromOffset, direction, ignoreCase), plain),
    ),
  bodyPretty: (bodyId) => unwrap(commands.bodyPretty(bodyId), plain),
  bodySaveAs: (bodyId, name) => unwrap(commands.bodySaveAs(bodyId, name), plain),
  closeBody: (bodyId) => commands.closeBody(bodyId),
  recentSends: () => commands.recentSends(),
  onSendProgress: (cb) => events.sendProgress.listen((e) => cb(e.payload)),

  workspaceCreate: (path, name) => ws(commands.workspaceCreate(path, name)),
  workspaceOpen: (path) => ws(commands.workspaceOpen(path)),
  workspaceCreateSample: (path) => ws(commands.workspaceCreateSample(path)),
  workspaceClose: () => commands.workspaceClose(),
  workspaceCurrent: () => commands.workspaceCurrent(),
  recentWorkspaces: () => commands.recentWorkspaces(),
  forgetRecentWorkspace: (path) => commands.forgetRecentWorkspace(path),
  itemCreate: (parent, kind, name, index) => ws(commands.itemCreate(parent, kind, name, index)),
  itemRename: (id, name) => ws(commands.itemRename(id, name)),
  itemRenameFileToMatch: (id) => ws(commands.itemRenameFileToMatch(id)),
  itemDuplicate: (id) => ws(commands.itemDuplicate(id)),
  itemDelete: (id) => ws(commands.itemDelete(id)),
  itemMove: (id, parent, index) => ws(commands.itemMove(id, parent, index)),
  itemReveal: (id) => ws(commands.itemReveal(id)),
  itemOpenExternal: (id) => ws(commands.itemOpenExternal(id)),
  requestLoad: (id) => ws(commands.requestLoad(id)),
  requestSave: (request, baseHash, force) =>
    ws(commands.requestSave(toWire(request), baseHash, force)),
  folderLoad: (id) => ws(commands.folderLoad(id)),
  folderSave: (folder, baseHash) =>
    ws(commands.folderSave(wire<FolderFile_Deserialize>(folder), baseHash)),
  collectionLoad: () => ws(commands.collectionLoad()),
  collectionSave: (collection, baseHash) =>
    ws(commands.collectionSave(wire<CollectionFile_Deserialize>(collection), baseHash)),
  scratchList: () => ws(commands.scratchList()),
  scratchSave: (request) => ws(commands.scratchSave(toWire(request))),
  scratchDelete: (id) => ws(commands.scratchDelete(id)),
  scratchSaveToWorkspace: (request, parent) =>
    ws(commands.scratchSaveToWorkspace(toWire(request), parent)),
  scratchLayout: () => ws(commands.scratchLayout()),
  scratchSaveLayout: (layout) => ws(commands.scratchSaveLayout(layout)),
  scratchFolderCreate: (name, parent) => ws(commands.scratchFolderCreate(name, parent)),
  sessionLoad: () => commands.sessionLoad(),
  sessionSave: (session) => ws(commands.sessionSave(session)),
  onWorkspaceChanged: (cb) => events.workspaceChanged.listen((e) => cb(e.payload)),

  variablesFor: (request, reveal) => vr(commands.variablesFor(toWire(request), reveal)),
  environmentsList: () => vr(commands.environmentsList()),
  environmentCreate: (name) => vr(commands.environmentCreate(name)),
  environmentDuplicate: (id) => vr(commands.environmentDuplicate(id)),
  environmentRename: (id, name) => vr(commands.environmentRename(id, name)),
  environmentDelete: (id) => vr(commands.environmentDelete(id)),
  environmentLoad: (id) => vr(commands.environmentLoad(id)),
  environmentSave: (environment, baseHash) =>
    vr(commands.environmentSave(wire<EnvironmentFile_Deserialize>(environment), baseHash)),
  activeEnvironmentGet: () => commands.activeEnvironmentGet(),
  activeEnvironmentSet: (id) => vr(commands.activeEnvironmentSet(id)),
  globalsLoad: () => vr(commands.globalsLoad()),
  globalsSave: (vars, baseHash) =>
    vr(commands.globalsSave(wire<{ [key: string]: Variable_Deserialize }>(vars), baseHash)),
  secretBackendStatus: () => vr(commands.secretBackendStatus()),
  secretBackendSet: (backend, passphrase) => vr(commands.secretBackendSet(backend, passphrase)),
  secretsUnlock: (passphrase) => vr(commands.secretsUnlock(passphrase)),
  secretSet: (scope, name, value) => vr(commands.secretSet(scope, name, value)),
  secretClear: (scope, name) => vr(commands.secretClear(scope, name)),
  secretReveal: (scope, name) => vr(commands.secretReveal(scope, name)),
  secretsMissing: () => vr(commands.secretsMissing()),

  authEffective: async (request) =>
    (await unwrap(commands.authEffective(toWire(request)), plain)) as unknown as EffectiveAuthDraft,
  pickBodyFile: () => unwrap(commands.pickBodyFile(), plain),
  bodyFilesCheck: (request) => unwrap(commands.bodyFilesCheck(toWire(request)), plain),
  networkSettingsGet: () => unwrap(commands.networkSettingsGet(), plain),
  networkSettingsSet: (proxy, caPaths) =>
    unwrap(commands.networkSettingsSet(proxy, caPaths), plain),
  proxyPasswordSet: async (password) => {
    await unwrap(commands.proxyPasswordSet(password), plain);
  },
  pickCaCert: () => unwrap(commands.pickCaCert(), plain),
  workspaceCaCertsGet: () => unwrap(commands.workspaceCaCertsGet(), plain),
  workspaceCaCertsSet: (paths) => unwrap(commands.workspaceCaCertsSet(paths), plain),
  cookiesList: () => unwrap(commands.cookiesList(), plain),
  cookieSet: async (cookie) => {
    await unwrap(commands.cookieSet(cookie), plain);
  },
  cookieDelete: async (domain, path, name) => {
    await unwrap(commands.cookieDelete(domain, path, name), plain);
  },
  cookiesClear: async () => {
    await unwrap(commands.cookiesClear(), plain);
  },

  historyList: (query) => unwrap(commands.historyList(query), plain),
  historyGet: async (id) =>
    (await unwrap(commands.historyGet(id), plain)) as unknown as HistoryEntryDraft,
  historyOpenBody: (id) => unwrap(commands.historyOpenBody(id), plain),
  historyDelete: async (id) => {
    await unwrap(commands.historyDelete(id), plain);
  },
  historyClear: async (workspace) => {
    await unwrap(commands.historyClear(workspace), plain);
  },
  historySettingsGet: () => unwrap(commands.historySettingsGet(), plain),
  historySettingsSet: async (settings) => {
    await unwrap(commands.historySettingsSet(settings), plain);
  },
  workspaceHistoryBodiesGet: () => unwrap(commands.workspaceHistoryBodiesGet(), plain),
  workspaceHistoryBodiesSet: async (storeBodies) => {
    await unwrap(commands.workspaceHistoryBodiesSet(storeBodies), plain);
  },

  curlParse: async (command) =>
    (await unwrap(commands.curlParse(command), plain)) as unknown as CurlParseResultDraft,
  curlDetect: (text) => commands.curlDetect(text),
  curlExport: (request, dialect, mode) =>
    unwrap(commands.curlExport(toWire(request), dialect, mode), plain),
  openapiPickAndPreview: () => unwrap(commands.openapiPickAndPreview(), plain),
  openapiPreviewText: (text) => unwrap(commands.openapiPreviewText(text), plain),
  openapiImport: (options) => unwrap(commands.openapiImport(options), plain),

  runItems: (target) => vr(commands.runItems(target)),
  runCollection: (runId, target, environment, options) =>
    vr(commands.runCollection(runId, target, environment, options)),
  runCancel: (runId) => commands.runCancel(runId),
  runReportMarkdown: (runId) => commands.runReportMarkdown(runId),
  onRunProgress: (cb) => events.runProgress.listen((e) => cb(e.payload)),
  runtimeVars: (reveal) => commands.runtimeVars(reveal),
  runtimeVarsClear: (name) => commands.runtimeVarsClear(name),
  runtimeVarSave: async (name) => {
    await vr(commands.runtimeVarSave(name));
  },

  settingsGet: () => commands.settingsGet(),
  settingsSet: (settings) => unwrap(commands.settingsSet(settings), plain),
  async setZoom(scale) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(scale);
  },
  diagnosticInfo: () => commands.diagnosticInfo(),
  openLogsFolder: async () => {
    await unwrap(commands.openLogsFolder(), plain);
  },
  logFrontendError: (message, stack) => commands.logFrontendError(message, stack),

  async onCloseRequested(allow) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    // Tauri awaits the handler and closes unless it called preventDefault.
    return getCurrentWindow().onCloseRequested(async (event) => {
      if (!(await allow())) event.preventDefault();
    });
  },
  // Picked in Rust: the workspace commands only accept folders from this picker.
  pickFolder: (title) => ws(commands.workspacePickFolder(title)),
  async confirm(message, options) {
    const { ask } = await import("@tauri-apps/plugin-dialog");
    return ask(message, {
      title: options.title,
      kind: options.kind ?? "warning",
      okLabel: options.okLabel,
      cancelLabel: options.cancelLabel ?? "Cancel",
    });
  },
  async choose(message, options) {
    const { message: show } = await import("@tauri-apps/plugin-dialog");
    const result = await show(message, {
      title: options.title,
      kind: options.kind ?? "warning",
      buttons: { yes: options.yes, no: options.no, cancel: options.cancel },
    });
    // Custom buttons return the clicked label; default ones return Yes/No/Cancel.
    if (result === options.yes || result === "Yes") return "yes";
    if (result === options.no || result === "No") return "no";
    return "cancel";
  },
  async openUrl(url) {
    // Only http(s)/mailto/tel are allowed (capabilities/default.json).
    await openUrl(url);
  },
};

let current: Api = tauriApi;

/** The active backend. */
export const api: Api = new Proxy({} as Api, {
  get: (_, key: keyof Api) => current[key],
});

/** Replaces the backend, e.g. with a mock in tests or when running outside Tauri. */
export function setApi(impl: Api) {
  current = impl;
}

/** True inside the Tauri webview (false in a plain browser or jsdom). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Unique ID for a send, chosen by the UI so it can cancel before the send returns. */
export function newSendId(): string {
  return crypto.randomUUID();
}
