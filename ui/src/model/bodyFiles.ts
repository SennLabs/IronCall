/** Files a request uploads (PLAN.md Phase 4): enabled multipart file parts and binary bodies. */

import type { MultipartPartDraft, RequestDraft } from "../ipc/api";

export function bodyFilePaths(request: RequestDraft): string[] {
  const body = request.body;
  if (body?.type === "binaryFile") {
    return typeof body.path === "string" && body.path ? [body.path] : [];
  }
  if (body?.type === "multipart" && Array.isArray(body.parts)) {
    return (body.parts as MultipartPartDraft[])
      .filter((p) => p.enabled !== false && p.type === "file" && p.path)
      .map((p) => p.path as string);
  }
  return [];
}

/** Upload progress as a 0–100 percentage, or `null` if the total is unknown. */
export function uploadPercent(uploaded: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((uploaded / total) * 100)));
}
