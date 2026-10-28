/**
 * History and large-body types (PLAN.md Phase 5) in the UI's editing shapes.
 * Plain types come straight from the bindings; entries carry a `RequestDraft`.
 */

import type { RequestDraft } from "./api";
import type { BodyHit, HistoryResponse, HistorySummary, RequestPreview } from "./bindings";

export type {
  CertificateInfo,
  DerivedBody,
  HistoryPage,
  HistoryQuery,
  HistoryResponse,
  HistorySettings,
  HistorySummary,
  InformationalResponse,
  SearchDirection,
  SearchResult,
  StatusClass,
  TimelineEntry,
  TimelineKind,
  TlsInfo,
} from "./bindings";

/** A history entry with the authored request as a draft (same JSON as the backend). */
export interface HistoryEntryDraft {
  summary: HistorySummary;
  /** The request as authored, with `{{variables}}` and sensitive literals masked. */
  request: RequestDraft;
  sent: RequestPreview | null;
  response: HistoryResponse | null;
  errorMessage: string | null;
}

/**
 * Result of a body search. `cancelled` is set when a newer search on the same body
 * superseded this one (the backend stops the old scan); the UI ignores such results.
 */
export interface BodySearchOutcome {
  hit: BodyHit | null;
  cancelled: boolean;
}
