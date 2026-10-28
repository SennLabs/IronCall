import { describe, expect, it } from "vitest";
import { bodyUrl, detectPlatform, previewUrl, schemeUrl } from "./scheme";

describe("schemeUrl", () => {
  it("uses http://<scheme>.localhost on Windows", () => {
    expect(schemeUrl("body/3", "windows")).toBe("http://ironcall.localhost/body/3");
  });

  it("uses <scheme>://localhost elsewhere and strips leading slashes", () => {
    expect(schemeUrl("/body/3", "other")).toBe("ironcall://localhost/body/3");
  });

  it("detects Windows from the user agent", () => {
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/140")).toBe("windows");
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe("other");
  });

  it("builds body and isolated preview URLs per platform", () => {
    expect(bodyUrl(7, "other")).toBe("ironcall://localhost/body/7");
    expect(previewUrl(7, "other")).toBe("ironcall-preview://localhost/body/7");
    expect(previewUrl(7, "windows")).toBe("http://ironcall-preview.localhost/body/7");
  });
});
