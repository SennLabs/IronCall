/** TLS summary helpers (PLAN.md Phase 5). */

import type { CertificateInfo } from "../ipc/bindings";

export const EXPIRY_WARNING_DAYS = 30;

const DAY_MS = 24 * 60 * 60 * 1000;

/** Whole days until `notAfter` (negative if expired); `null` if unknown. */
export function daysUntilExpiry(notAfter: string | null, now: Date = new Date()): number | null {
  if (!notAfter) return null;
  const t = Date.parse(notAfter);
  if (Number.isNaN(t)) return null;
  return Math.floor((t - now.getTime()) / DAY_MS);
}

export type ExpiryStatus =
  | { level: "ok"; days: number }
  | { level: "warning"; days: number }
  | { level: "expired"; days: number }
  | { level: "notYetValid" }
  | { level: "unknown" };

export function expiryStatus(cert: CertificateInfo, now: Date = new Date()): ExpiryStatus {
  if (cert.notBefore) {
    const start = Date.parse(cert.notBefore);
    if (!Number.isNaN(start) && start > now.getTime()) return { level: "notYetValid" };
  }
  const days = daysUntilExpiry(cert.notAfter, now);
  if (days === null) return { level: "unknown" };
  if (days < 0) return { level: "expired", days };
  if (days < EXPIRY_WARNING_DAYS) return { level: "warning", days };
  return { level: "ok", days };
}

export function expiryText(status: ExpiryStatus): string {
  switch (status.level) {
    case "ok":
      return `valid for ${status.days} more days`;
    case "warning":
      return status.days === 0
        ? "expires today"
        : `expires in ${status.days} day${status.days === 1 ? "" : "s"}`;
    case "expired":
      return `expired ${-status.days} day${status.days === -1 ? "" : "s"} ago`;
    case "notYetValid":
      return "not valid yet";
    case "unknown":
      return "validity unknown";
  }
}
