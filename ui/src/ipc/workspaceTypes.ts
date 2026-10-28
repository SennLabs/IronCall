/**
 * Phase 2 IPC types (`src-tauri/src/workspace.rs`).
 *
 * Plain types come straight from the generated bindings. Types that carry files the UI
 * edits (requests, folders, the collection) use the draft types from `api.ts`, so
 * unknown keys survive editing; `api.ts` casts at the boundary (same JSON).
 */

import type { KeyValueRow, RequestDraft } from "./api";
import type { AuthDraft } from "./networkTypes";

type Open = { [key: string]: unknown };

// Identical to the generated bindings: re-exported so there's one source of truth.
export type {
  ItemId,
  NewItemKind,
  NodeKind,
  RecentWorkspace,
  ScratchFolder,
  ScratchLayout,
  Session,
  TabRef,
  TreeNode,
  WorkspaceChanged,
  WorkspaceInfo,
} from "./bindings";

import type { ItemId } from "./bindings";

export interface LoadedRequest {
  request: RequestDraft;
  hash: string;
  readOnly: boolean;
}

/** `_folder.yaml` / `ironcall.yaml` as the UI edits them (description only in Phase 2). */
export type FolderDraft = {
  schema: string;
  id: ItemId;
  name: string;
  description?: string | null;
  /** Default headers for every request inside. */
  headers?: KeyValueRow[];
  /** `inherit` (or absent) at the collection means no auth. */
  auth?: AuthDraft;
  order?: ItemId[];
} & Open;

export type CollectionDraft = FolderDraft;

export interface LoadedFolder {
  folder: FolderDraft;
  hash: string | null;
}

export interface LoadedCollection {
  collection: CollectionDraft;
  hash: string;
}

export type SaveOutcome =
  | { outcome: "saved"; hash: string }
  | { outcome: "conflict"; disk: LoadedRequest | null };

/** Thrown by workspace commands, carrying the backend's message. */
export class WorkspaceFailure extends Error {
  /** A save was refused because the file changed on disk. */
  readonly conflict: boolean;
  constructor(message: string, conflict = false) {
    super(message);
    this.name = "WorkspaceFailure";
    this.conflict = conflict;
  }
}
