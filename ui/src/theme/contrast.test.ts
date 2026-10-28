import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { contrast, tokens } from "./contrast";

const css = readFileSync(join(__dirname, "tokens.css"), "utf8");
const light = tokens(css.slice(css.indexOf(":root {"), css.indexOf("@media")));
const darkStart = css.indexOf(':root[data-theme="dark"]');
const darkOverrides = tokens(css.slice(darkStart, css.indexOf("}", darkStart)));
// Dark overrides colours; anything it doesn't set cascades from `:root`.
const dark = { ...light, ...darkOverrides };

const BACKGROUNDS = ["bg", "bg-subtle", "bg-raised", "selection"];
/** Text colours: 4.5:1 on every background (WCAG 1.4.3, normal-size text). */
const TEXT = [
  "text",
  "text-muted",
  "success",
  "warning",
  "danger",
  "method-get",
  "method-post",
  "method-put",
  "method-patch",
  "method-delete",
  "method-other",
  "var",
  "var-helper",
  "var-unresolved",
  "syntax-string",
  "syntax-number",
  "syntax-keyword",
  "syntax-property",
  "syntax-comment",
];

function failures(t: Record<string, string>) {
  const out: string[] = [];
  const check = (fg: string, bg: string, min: number) => {
    const ratio = contrast(t[fg] as string, t[bg] as string);
    if (ratio < min) out.push(`${fg} on ${bg}: ${ratio.toFixed(2)} < ${min}`);
  };
  for (const bg of BACKGROUNDS) for (const fg of TEXT) check(fg, bg, 4.5);
  check("accent-text", "accent", 4.5);
  // Non-text UI: the accent (active tab, focus ring) and control borders, 3:1 (WCAG 1.4.11).
  for (const bg of ["bg", "bg-subtle", "bg-raised"]) {
    check("accent", bg, 3);
    check("border-strong", bg, 3);
  }
  return out;
}

describe("theme contrast (WCAG AA)", () => {
  it("parses both themes", () => {
    expect(Object.keys(light).length).toBeGreaterThan(20);
    expect(Object.keys(darkOverrides).every((k) => k in light)).toBe(true);
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21, 0);
  });

  it("light theme", () => {
    expect(failures(light)).toEqual([]);
  });

  it("dark theme", () => {
    expect(failures(dark)).toEqual([]);
  });
});
