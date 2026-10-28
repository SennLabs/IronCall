/**
 * Syntax colours for CodeMirror from the theme tokens, so both themes keep WCAG AA
 * contrast (checked in `theme/contrast.test.ts`). Replaces basicSetup's light-only
 * default highlight style.
 */
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";

const style = HighlightStyle.define([
  { tag: [t.string, t.special(t.string), t.regexp], color: "var(--syntax-string)" },
  { tag: [t.number, t.integer, t.float], color: "var(--syntax-number)" },
  { tag: [t.bool, t.null, t.keyword, t.atom], color: "var(--syntax-keyword)" },
  { tag: [t.propertyName, t.attributeName, t.tagName], color: "var(--syntax-property)" },
  {
    tag: [t.comment, t.meta, t.processingInstruction],
    color: "var(--syntax-comment)",
    fontStyle: "italic",
  },
  { tag: t.invalid, color: "var(--danger)" },
]);

export const syntaxTheme = syntaxHighlighting(style);
