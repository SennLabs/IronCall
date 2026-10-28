import { describe, expect, it } from "vitest";
import { formatBytes, formatMs, statusClass } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(20 * 1024 * 1024)).toBe("20 MB");
  });

  it("formats durations", () => {
    expect(formatMs(0.42)).toBe("0.42 ms");
    expect(formatMs(5.12)).toBe("5.1 ms");
    expect(formatMs(250.4)).toBe("250 ms");
    expect(formatMs(1500)).toBe("1.50 s");
    expect(formatMs(null)).toBe("–");
  });

  it("classifies status codes", () => {
    expect([101, 204, 304, 404, 503].map(statusClass)).toEqual([
      "info",
      "success",
      "redirect",
      "client",
      "server",
    ]);
  });
});
