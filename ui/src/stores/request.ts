/**
 * One tab's request draft (each tab owns a store; see `tabs.ts`).
 *
 * The draft is kept as an immutable object: every edit spreads the previous value, so
 * keys the UI doesn't know about survive and are saved back to disk later.
 */

import { type Accessor, createSignal } from "solid-js";
import type {
  AssertionDraft,
  AuthDraft,
  DraftBody,
  DraftSettings,
  ExtractionDraft,
  KeyValueRow,
  MultipartPartDraft,
  PathParamRow,
  RequestDraft,
  VarsDraft,
} from "../ipc/api";
import { applyUrlText, buildUrlText, syncPathParams } from "../model/url";

export type EditableBodyType =
  | "none"
  | "json"
  | "text"
  | "xml"
  | "formUrlencoded"
  | "multipart"
  | "binaryFile";

export const EDITABLE_BODY_TYPES: readonly EditableBodyType[] = [
  "none",
  "json",
  "text",
  "xml",
  "formUrlencoded",
  "multipart",
  "binaryFile",
];

const TEXT_TYPES = new Set(["json", "text", "xml"]);

/** Rows of a form-urlencoded body. */
export function bodyFields(draft: RequestDraft): KeyValueRow[] {
  const body = draft.body;
  return body?.type === "formUrlencoded" && Array.isArray(body.fields)
    ? (body.fields as KeyValueRow[])
    : [];
}

/** Parts of a multipart body. */
export function bodyParts(draft: RequestDraft): MultipartPartDraft[] {
  const body = draft.body;
  return body?.type === "multipart" && Array.isArray(body.parts)
    ? (body.parts as MultipartPartDraft[])
    : [];
}

/** File of a binary-file body. */
export function bodyFile(draft: RequestDraft): { path: string; contentType: string | null } {
  const body = draft.body;
  if (body?.type !== "binaryFile") return { path: "", contentType: null };
  return {
    path: typeof body.path === "string" ? body.path : "",
    contentType: typeof body.contentType === "string" ? body.contentType : null,
  };
}

export function bodyType(draft: RequestDraft): DraftBody["type"] {
  return draft.body?.type ?? "none";
}

export function bodyContent(draft: RequestDraft): string {
  const body = draft.body;
  return body && typeof body.content === "string" ? body.content : "";
}

export interface RequestStore {
  draft: Accessor<RequestDraft>;
  /** Text in the URL bar: `url` plus enabled params, exactly as typed. */
  urlText: Accessor<string>;
  setMethod(method: string): void;
  setUrlText(text: string): void;
  setParams(params: KeyValueRow[]): void;
  setPathParams(pathParams: PathParamRow[]): void;
  setHeaders(headers: KeyValueRow[]): void;
  /** Request-scope variables; an empty map removes the key. */
  setVars(vars: VarsDraft): void;
  /**
   * Switches the body type. Text kinds share their content; other kinds come back as
   * they were if the user switches away and back within this tab.
   */
  setBodyType(type: EditableBodyType): void;
  setBodyContent(content: string): void;
  setBodyFields(fields: KeyValueRow[]): void;
  setMultipartParts(parts: MultipartPartDraft[]): void;
  setBinaryFile(patch: { path?: string; contentType?: string | null }): void;
  /** Request auth; `inherit` removes the key (the default). */
  setAuth(auth: AuthDraft): void;
  /** Assertions; an empty list removes the key. */
  setAssertions(list: AssertionDraft[]): void;
  /** Extractions; an empty list removes the key. */
  setExtractions(list: ExtractionDraft[]): void;
  setSettings(patch: Partial<DraftSettings>): void;
  setName(name: string): void;
  /** Markdown description; empty clears it. */
  setDescription(description: string): void;
  /** Replaces the whole draft (e.g. a new scratch request). */
  load(draft: RequestDraft): void;
}

export function createRequestStore(initial: RequestDraft): RequestStore {
  const [draft, setDraft] = createSignal<RequestDraft>(initial);
  const [urlText, setUrlTextSignal] = createSignal(buildUrlText(initial.url, initial.params ?? []));

  const update = (patch: Partial<RequestDraft>) => setDraft((d) => ({ ...d, ...patch }));
  /** Bodies the user switched away from, by type, so switching back restores them. */
  const stashed = new Map<string, DraftBody>();
  const patchBody = (type: DraftBody["type"], patch: Record<string, unknown>) => {
    const body = draft().body;
    if (body?.type !== type) return;
    update({ body: { ...body, ...patch } as DraftBody });
  };

  return {
    draft,
    urlText,
    setMethod: (method) => update({ method: method.trim().toUpperCase() }),
    setUrlText(text) {
      // The URL bar keeps the user's text verbatim; only the model is re-derived.
      setUrlTextSignal(text);
      const d = draft();
      const { url, params } = applyUrlText(text, d.params ?? []);
      update({ url, params, pathParams: syncPathParams(d.pathParams ?? [], url) });
    },
    setParams(params) {
      update({ params });
      setUrlTextSignal(buildUrlText(draft().url, params));
    },
    setPathParams: (pathParams) => update({ pathParams }),
    setHeaders: (headers) => update({ headers }),
    setVars(vars) {
      if (Object.keys(vars).length > 0) {
        update({ vars });
        return;
      }
      const { vars: _, ...rest } = draft();
      setDraft(rest as RequestDraft);
    },
    setBodyType(type) {
      const d = draft();
      const current = d.body;
      if (current?.type === type) return;
      if (current && current.type !== "none") stashed.set(current.type, current);
      if (type === "none") {
        update({ body: { type: "none" } });
        return;
      }
      if (TEXT_TYPES.has(type)) {
        // Keep the content and any unknown keys when switching between text kinds.
        const previous = current && typeof current.content === "string" ? current : undefined;
        const base = previous ?? stashed.get(type) ?? {};
        update({ body: { ...base, type, content: bodyContent(d) } as DraftBody });
        return;
      }
      const restored = stashed.get(type);
      const fresh: Record<string, DraftBody> = {
        formUrlencoded: { type: "formUrlencoded", fields: [] },
        multipart: { type: "multipart", parts: [] },
        binaryFile: { type: "binaryFile", path: "" },
      };
      update({ body: restored ?? (fresh[type] as DraftBody) });
    },
    setBodyContent(content) {
      const body = draft().body;
      if (!body || body.type === "none" || typeof body.content !== "string") return;
      update({ body: { ...body, content } });
    },
    setBodyFields: (fields) => patchBody("formUrlencoded", { fields }),
    setMultipartParts: (parts) => patchBody("multipart", { parts }),
    setBinaryFile: (patch) => patchBody("binaryFile", patch),
    setAuth(auth) {
      if (auth.type === "inherit" && Object.keys(auth).length === 1) {
        const { auth: _, ...rest } = draft();
        setDraft(rest as RequestDraft);
        return;
      }
      update({ auth });
    },
    setAssertions(list) {
      if (list.length > 0) {
        update({ assert: list });
        return;
      }
      const { assert: _, ...rest } = draft();
      setDraft(rest as RequestDraft);
    },
    setExtractions(list) {
      if (list.length > 0) {
        update({ extract: list });
        return;
      }
      const { extract: _, ...rest } = draft();
      setDraft(rest as RequestDraft);
    },
    setSettings: (patch) => update({ settings: { ...(draft().settings ?? {}), ...patch } }),
    setName: (name) => update({ name }),
    setDescription: (description) => update({ description: description || null }),
    load(next) {
      setDraft(next);
      setUrlTextSignal(buildUrlText(next.url, next.params ?? []));
    },
  };
}
