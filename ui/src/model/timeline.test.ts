import { describe, expect, it } from "vitest";
import type { TimelineEntry } from "../ipc/bindings";
import { firstLine, formatAt, isBlock, timelineText } from "./timeline";

const entries: TimelineEntry[] = [
  { atMs: 0, kind: "info", text: "Connected to 127.0.0.1:443" },
  { atMs: 12.5, kind: "requestHeaders", text: "GET /x HTTP/1.1\nhost: a\nauthorization: ••••" },
  { atMs: 40, kind: "responseHeaders", text: "HTTP/1.1 200 OK\ncontent-type: text/plain" },
  { atMs: 1500, kind: "error", text: "connection reset" },
];

describe("timeline formatting", () => {
  it("formats timestamps", () => {
    expect(formatAt(12.5).trim()).toBe("12.5ms");
    expect(formatAt(1500).trim()).toBe("1.50s");
    expect(formatAt(null).trim()).toBe("");
  });

  it("collapses multi-line header blocks only", () => {
    expect(entries.map(isBlock)).toEqual([false, true, true, false]);
    expect(firstLine(entries[1] as TimelineEntry)).toBe("GET /x HTTP/1.1");
  });

  it("copies as curl -v style text", () => {
    expect(timelineText(entries)).toBe(
      [
        "[0.0ms] * Connected to 127.0.0.1:443",
        "[12.5ms] > GET /x HTTP/1.1",
        "         > host: a",
        "         > authorization: ••••",
        "[40.0ms] < HTTP/1.1 200 OK",
        "         < content-type: text/plain",
        "[1.50s] ! connection reset",
      ].join("\n"),
    );
  });
});
