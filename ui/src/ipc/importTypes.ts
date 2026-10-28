/**
 * cURL import/export and OpenAPI import (PLAN.md Phase 6), typed for the UI. The
 * generated types are re-exported where they need no editing; the parsed request is
 * draft-typed like everywhere else.
 */

import type { RequestDraft } from "./api";

export type {
  CurlDialect,
  CurlExport,
  CurlExportMode,
  ImportWarning,
  OpenApiImportOptions,
  OpenApiImportResult,
  OpenApiPreview,
  PlanNode,
  SpecVersion,
} from "./bindings";

import type { ImportWarning } from "./bindings";

/** `curl_parse` result with a draft-typed request (same JSON). */
export interface CurlParseResultDraft {
  request: RequestDraft;
  warnings: ImportWarning[];
}
