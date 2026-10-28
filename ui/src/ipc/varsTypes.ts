/**
 * Phase 3 IPC types (`src-tauri/src/vars.rs`): variables, environments and secrets.
 *
 * Plain types come from the generated bindings. Files the UI edits (environments,
 * globals) use draft types so unknown keys survive editing; `api.ts` casts at the
 * boundary (same JSON).
 */

export type {
  DynamicHelper,
  EnvironmentSummary,
  MissingSecret,
  SecretBackend,
  SecretBackendStatus,
  SecretScope,
  VariableContext,
  VarSource,
  VisibleVar,
} from "./bindings";

import type { ItemId } from "./bindings";

type Open = { [key: string]: unknown };

/** Detailed variable form (`apiKey: {secret: true}`), as in `VariableSpec`. */
export type VariableSpecDraft = {
  value?: string | null;
  secret?: boolean;
  description?: string | null;
} & Open;

/** A variable as stored: a plain string, or the detailed form. */
export type VariableDraft = string | VariableSpecDraft;

/** Variables at one scope, in file order. */
export type VarsDraft = Record<string, VariableDraft>;

export type EnvironmentDraft = {
  schema: string;
  id: ItemId;
  name: string;
  vars?: VarsDraft;
} & Open;

export interface LoadedEnvironment {
  environment: EnvironmentDraft;
  hash: string;
}

export interface LoadedGlobals {
  vars: VarsDraft;
  hash: string | null;
}

/** Thrown by variable, environment and secret commands. */
export class VarsFailure extends Error {
  /** The encrypted secrets file must be unlocked first. */
  readonly locked: boolean;
  /** A save was refused because the file changed on disk. */
  readonly conflict: boolean;
  constructor(message: string, locked = false, conflict = false) {
    super(message);
    this.name = "VarsFailure";
    this.locked = locked;
    this.conflict = conflict;
  }
}
