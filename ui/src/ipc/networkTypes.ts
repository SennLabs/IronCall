/**
 * Phase 4 IPC types (`src-tauri/src/network.rs`): auth inheritance, file bodies, proxy,
 * CA certificates and cookies.
 *
 * Plain types come from the generated bindings. Auth and multipart parts live inside
 * files the UI edits, so they use draft types that keep unknown keys.
 */

export type {
  ApiKeyLocation,
  AuthOrigin,
  BodyFileStatus,
  CaCertInfo,
  CookieRow,
  NetworkSettings,
  PickedFile,
  ProxyMode,
  ProxySettings,
  SameSite,
} from "./bindings";

import type { ApiKeyLocation, AuthOrigin } from "./bindings";

type Open = { [key: string]: unknown };

/** Auth as stored in a request, folder or collection file (`type` is the tag). */
export type AuthDraft =
  | ({ type: "inherit" } & Open)
  | ({ type: "none" } & Open)
  | ({ type: "basic"; username?: string; password?: string } & Open)
  | ({ type: "bearer"; token?: string } & Open)
  | ({ type: "apiKey"; key?: string; value?: string; in?: ApiKeyLocation } & Open);

export type AuthType = AuthDraft["type"];

export interface EffectiveAuthDraft {
  auth: AuthDraft;
  origin: AuthOrigin;
}

/** One part of a multipart/form-data body. */
export type MultipartPartDraft = {
  name: string;
  type?: "text" | "file";
  value?: string | null;
  path?: string | null;
  contentType?: string | null;
  enabled?: boolean;
} & Open;
