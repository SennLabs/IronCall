import { describe, expect, it } from "vitest";
import type { CertificateInfo } from "../ipc/bindings";
import { daysUntilExpiry, expiryStatus, expiryText } from "./tls";

const now = new Date("2026-10-05T12:00:00Z");
const cert = (
  notAfter: string | null,
  notBefore: string | null = "2026-01-01T00:00:00Z",
): CertificateInfo => ({
  subject: "CN=x",
  issuer: "CN=ca",
  notBefore,
  notAfter,
  sans: [],
  chainLength: 1,
});

describe("certificate expiry", () => {
  it("is fine with 30 or more days left", () => {
    expect(expiryStatus(cert("2026-11-05T12:00:00Z"), now)).toEqual({ level: "ok", days: 31 });
  });

  it("warns under 30 days", () => {
    const s = expiryStatus(cert("2026-10-20T12:00:00Z"), now);
    expect(s).toEqual({ level: "warning", days: 15 });
    expect(expiryText(s)).toBe("expires in 15 days");
    expect(expiryText(expiryStatus(cert("2026-10-05T18:00:00Z"), now))).toBe("expires today");
  });

  it("reports expired and not-yet-valid certificates", () => {
    expect(expiryStatus(cert("2026-10-01T12:00:00Z"), now)).toEqual({ level: "expired", days: -4 });
    expect(expiryStatus(cert("2027-10-01T00:00:00Z", "2026-12-01T00:00:00Z"), now)).toEqual({
      level: "notYetValid",
    });
  });

  it("handles missing or invalid dates", () => {
    expect(daysUntilExpiry(null, now)).toBeNull();
    expect(daysUntilExpiry("not a date", now)).toBeNull();
    expect(expiryStatus(cert(null), now)).toEqual({ level: "unknown" });
  });
});
