/**
 * Phase 7 IPC types (`src-tauri/src/run.rs`, `ironcall-core` runner): assertion and
 * extraction results, runtime variables and collection runs. All plain generated types;
 * the editable assertion and extraction drafts live in `api.ts` with `RequestDraft`.
 */

export type {
  AssertionResult,
  AssertOp,
  AssertTarget,
  EnvChoice,
  ExtractedView,
  ExtractSource,
  ItemResult,
  Outcome,
  Progress,
  ResponseChecks,
  RunItem,
  RunOptions,
  RunProgress,
  RunReport,
  RunTarget,
  RuntimeVarView,
} from "./bindings";
