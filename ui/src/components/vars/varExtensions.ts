/**
 * `{{variable}}` support for CodeMirror (request bodies): state-aware highlighting,
 * `{{` autocomplete and hover tooltips, driven by the tab's `VariableContext`.
 */

import {
  autocompletion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { type Extension, RangeSetBuilder, StateEffect } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  hoverTooltip,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type { VariableContext } from "../../ipc/api";
import { completions, findRefs, hoverInfo, refState } from "../../model/variables";

/** Dispatch to re-run highlighting after the context changes. */
export const refreshVars = StateEffect.define<null>();

const marks = {
  defined: Decoration.mark({ class: "cm-ic-var" }),
  helper: Decoration.mark({ class: "cm-ic-var cm-ic-helper" }),
  unresolved: Decoration.mark({ class: "cm-ic-var cm-ic-unresolved" }),
  missing: Decoration.mark({ class: "cm-ic-var cm-ic-missing" }),
  unknown: Decoration.mark({ class: "cm-ic-var" }),
};

function build(view: EditorView, ctx: VariableContext | undefined): DecorationSet {
  const b = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    const text = view.state.doc.sliceString(from, to);
    for (const r of findRefs(text)) b.add(from + r.from, from + r.to, marks[refState(r.name, ctx)]);
  }
  return b.finish();
}

export const varTheme = EditorView.theme({
  ".cm-ic-var": {
    color: "var(--var)",
    backgroundColor: "color-mix(in srgb, var(--var) 14%, transparent)",
    borderRadius: "3px",
  },
  ".cm-ic-helper": {
    color: "var(--var-helper)",
    backgroundColor: "color-mix(in srgb, var(--var-helper) 14%, transparent)",
  },
  ".cm-ic-unresolved": {
    color: "var(--var-unresolved)",
    backgroundColor: "color-mix(in srgb, var(--var-unresolved) 14%, transparent)",
    textDecoration: "underline wavy var(--var-unresolved)",
  },
  ".cm-ic-missing": {
    color: "var(--warning)",
    backgroundColor: "color-mix(in srgb, var(--warning) 16%, transparent)",
  },
  ".cm-tooltip.cm-ic-tip": {
    padding: "6px 8px",
    fontSize: "12px",
    maxWidth: "420px",
    backgroundColor: "var(--bg-raised)",
    color: "var(--text)",
    border: "1px solid var(--border)",
  },
  ".cm-ic-tip-title": { fontFamily: "var(--font-mono)", color: "var(--var)", fontWeight: "600" },
  ".cm-ic-tip-value": { fontFamily: "var(--font-mono)", wordBreak: "break-all" },
  ".cm-ic-tip-muted": { color: "var(--text-muted)" },
  ".cm-ic-tip-problem": { color: "var(--var-unresolved)" },
});

/** Completion source for `{{…`; exported for tests. */
export function varCompletionSource(ctx: () => VariableContext | undefined) {
  return (c: CompletionContext): CompletionResult | null => {
    const m = c.matchBefore(/\{\{\s*\$?[\w.-]*/);
    if (!m) return null;
    const partial = /\$?[\w.-]*$/.exec(m.text)?.[0] ?? "";
    const from = m.to - partial.length;
    const after = c.state.doc.sliceString(c.pos, c.pos + 2);
    return {
      from,
      options: completions(ctx(), partial).map((x) => ({
        label: x.name,
        detail: x.detail,
        info: x.source,
        type: x.helper ? "function" : "variable",
        apply: after === "}}" ? x.name : `${x.name}}}`,
      })),
      validFor: /^\$?[\w.-]*$/,
    };
  };
}

function tooltipDom(name: string, ctx: VariableContext | undefined): HTMLElement {
  const info = hoverInfo(name, ctx);
  const dom = document.createElement("div");
  const line = (cls: string, text: string) => {
    if (!text) return;
    const el = document.createElement("div");
    el.className = cls;
    el.textContent = text;
    dom.appendChild(el);
  };
  line("cm-ic-tip-title", info.title);
  line("cm-ic-tip-value", info.value);
  if (info.source) line("cm-ic-tip-muted", `from ${info.source}`);
  if (info.overrides.length) line("cm-ic-tip-muted", `overrides ${info.overrides.join(", ")}`);
  if (info.problem) line("cm-ic-tip-problem", info.problem);
  return dom;
}

/** All variable features for an editor; `ctx` is read on every update. */
export function varExtensions(ctx: () => VariableContext | undefined): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view, ctx());
      }
      update(u: ViewUpdate) {
        const refreshed = u.transactions.some((t) => t.effects.some((e) => e.is(refreshVars)));
        if (u.docChanged || u.viewportChanged || refreshed) this.decorations = build(u.view, ctx());
      }
    },
    { decorations: (v) => v.decorations },
  );
  const hover = hoverTooltip((view, pos) => {
    const line = view.state.doc.lineAt(pos);
    const ref = findRefs(line.text).find(
      (r) => pos >= line.from + r.from && pos < line.from + r.to,
    );
    if (!ref) return null;
    return {
      pos: line.from + ref.from,
      end: line.from + ref.to,
      above: true,
      create: () => ({
        dom: Object.assign(tooltipDom(ref.name, ctx()), { className: "cm-ic-tip" }),
      }),
    };
  });
  return [
    plugin,
    hover,
    varTheme,
    autocompletion({ override: [varCompletionSource(ctx)], activateOnTyping: true }),
  ];
}
