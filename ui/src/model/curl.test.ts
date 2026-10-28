import { beforeEach, describe, expect, it } from "vitest";
import { mockDraft } from "../ipc/mock";
import { mockCurlParse, shellWords } from "../ipc/mockImport";
import {
  adoptParsed,
  countPlan,
  curlNoticeText,
  DEFAULT_REQUEST_NAME,
  defaultDialect,
  EXPORT_PREFS_KEY,
  isEmptyDraft,
  loadExportPrefs,
  looksLikeCurl,
  saveExportPrefs,
} from "./curl";

describe("looksLikeCurl", () => {
  it("matches curl and curl.exe commands only", () => {
    expect(looksLikeCurl("curl https://x.io")).toBe(true);
    expect(looksLikeCurl("  curl.exe -X POST https://x.io")).toBe(true);
    expect(looksLikeCurl("CURL https://x.io")).toBe(true);
    expect(looksLikeCurl("https://x.io/curl")).toBe(false);
    expect(looksLikeCurl("curly")).toBe(false);
  });
});

describe("isEmptyDraft", () => {
  it("is true for a fresh request", () => {
    expect(isEmptyDraft(mockDraft())).toBe(true);
    expect(isEmptyDraft(mockDraft({ body: { type: "json", content: "  " } }))).toBe(true);
    expect(isEmptyDraft(mockDraft({ headers: [{ name: "", value: "" }] }))).toBe(true);
  });

  it("is false once anything was entered", () => {
    expect(isEmptyDraft(mockDraft({ url: "https://x.io" }))).toBe(false);
    expect(isEmptyDraft(mockDraft({ method: "POST" }))).toBe(false);
    expect(isEmptyDraft(mockDraft({ headers: [{ name: "Accept", value: "" }] }))).toBe(false);
    expect(isEmptyDraft(mockDraft({ body: { type: "json", content: "{}" } }))).toBe(false);
    expect(isEmptyDraft(mockDraft({ auth: { type: "none" } }))).toBe(false);
  });
});

describe("adoptParsed", () => {
  const parsed = mockDraft({
    id: "PARSED",
    name: "POST /users",
    method: "POST",
    url: "https://x.io/users",
  });

  it("keeps the tab's ID and takes the parsed name if the tab still has the default", () => {
    const current = mockDraft({ id: "TAB", name: DEFAULT_REQUEST_NAME, extraKey: 1 });
    const out = adoptParsed(current, parsed);
    expect(out.id).toBe("TAB");
    expect(out.name).toBe("POST /users");
    expect(out.url).toBe("https://x.io/users");
  });

  it("keeps a name the user chose", () => {
    const out = adoptParsed(mockDraft({ id: "TAB", name: "Create user" }), parsed);
    expect(out.name).toBe("Create user");
  });
});

describe("curlNoticeText", () => {
  it("lists ignored flags once each, then other warnings", () => {
    expect(curlNoticeText([])).toBe("Imported from cURL");
    expect(
      curlNoticeText([
        { message: "--cert is not supported", location: "--cert" },
        { message: "-o is not supported", location: "-o" },
        { message: "-o again", location: "-o" },
        { message: "body file read from disk", location: null },
      ]),
    ).toBe("Imported from cURL; ignored: --cert, -o; body file read from disk");
  });
});

describe("export preferences", () => {
  beforeEach(() => localStorage.clear());

  it("defaults to PowerShell on Windows and bash elsewhere, masked mode", () => {
    expect(defaultDialect("windows")).toBe("powerShell");
    expect(defaultDialect("other")).toBe("posix");
    expect(loadExportPrefs("windows")).toEqual({ dialect: "powerShell", mode: "masked" });
  });

  it("remembers dialect and mode", () => {
    saveExportPrefs({ dialect: "powerShell", mode: "variables" });
    expect(loadExportPrefs("other")).toEqual({ dialect: "powerShell", mode: "variables" });
  });

  it("never remembers 'resolved with secrets'", () => {
    saveExportPrefs({ dialect: "posix", mode: "resolved" });
    expect(localStorage.getItem(EXPORT_PREFS_KEY)).not.toContain("resolved");
    expect(loadExportPrefs("other").mode).toBe("masked");
    // Even if something else wrote it.
    localStorage.setItem(EXPORT_PREFS_KEY, JSON.stringify({ dialect: "posix", mode: "resolved" }));
    expect(loadExportPrefs("other").mode).toBe("masked");
  });
});

describe("countPlan", () => {
  it("counts folders and requests recursively", () => {
    const leaf = (name: string) => ({ name, method: "GET", url: "", children: [] });
    expect(
      countPlan([
        { name: "pets", method: null, url: null, children: [leaf("a"), leaf("b")] },
        leaf("health"),
      ]),
    ).toEqual({ folders: 1, requests: 3 });
  });
});

describe("mock cURL parser (shared by UI tests)", () => {
  it("splits POSIX and cmd quoting", () => {
    expect(shellWords(`curl 'https://x.io/a b' -H "Accept: */*" \\\n  --compressed`)).toEqual([
      "curl",
      "https://x.io/a b",
      "-H",
      "Accept: */*",
      "--compressed",
    ]);
    expect(shellWords(`curl ^"https://x.io/^" ^\n  -H ^"X: 1^"`)).toEqual([
      "curl",
      "https://x.io/",
      "-H",
      "X: 1",
    ]);
  });

  it("parses method, URL, query, headers and body, and warns about unknown flags", () => {
    const r = mockCurlParse(
      `curl -X PUT 'https://x.io/users/1?notify=true' -H 'Content-Type: application/json' -d '{"a":1}' --cert c.pem -s`,
    );
    expect(r.request.method).toBe("PUT");
    expect(r.request.url).toBe("https://x.io/users/1");
    expect(r.request.params).toEqual([{ name: "notify", value: "true", enabled: true }]);
    expect(r.request.body).toEqual({ type: "json", content: '{"a":1}' });
    expect(r.warnings.map((w) => w.location)).toEqual(["--cert", "-s"]);
  });
});
