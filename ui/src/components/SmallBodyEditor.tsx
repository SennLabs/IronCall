import { json } from "@codemirror/lang-json";
import { xml } from "@codemirror/lang-xml";
import { type Diagnostic, linter, lintGutter } from "@codemirror/lint";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  MatchDecorator,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import { basicSetup } from "codemirror";
import { createEffect, on, onCleanup, onMount } from "solid-js";
import type { VariableContext } from "../ipc/api";
import { syntaxTheme } from "./syntaxTheme";
import { refreshVars, varExtensions } from "./vars/varExtensions";

/** Highlights `{{var}}` and `{{$helper}}` references (PLAN.md §5). */
const variableMatcher = new MatchDecorator({
  regexp: /\{\{\s*\$?[\w.-]+\s*\}\}/g,
  decoration: Decoration.mark({ class: "cm-ic-var" }),
});

export const variableHighlight = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = variableMatcher.createDeco(view);
    }
    update(update: ViewUpdate) {
      this.decorations = variableMatcher.updateDeco(update, this.decorations);
    }
  },
  { decorations: (v) => v.decorations },
);

const theme = EditorView.theme({
  "&": { height: "100%", backgroundColor: "var(--bg)", color: "var(--text)" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", fontSize: "12px" },
  ".cm-gutters": {
    backgroundColor: "var(--bg-subtle)",
    color: "var(--text-muted)",
    border: "none",
  },
  ".cm-activeLine, .cm-activeLineGutter": {
    backgroundColor: "color-mix(in srgb, var(--accent) 6%, transparent)",
  },
  ".cm-cursor": { borderLeftColor: "var(--text)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
    backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent) !important",
  },
  ".cm-panels": { backgroundColor: "var(--bg-subtle)", color: "var(--text)" },
  ".cm-ic-var": {
    color: "var(--accent)",
    backgroundColor: "color-mix(in srgb, var(--accent) 15%, transparent)",
    borderRadius: "3px",
  },
});

export type EditorLanguage = "json" | "xml" | "text";

/** Lint problem, as character offsets into the document. */
export interface EditorIssue {
  from: number;
  to: number;
  message: string;
}

export interface SmallBodyEditorProps {
  value: string;
  language?: EditorLanguage;
  readOnly?: boolean;
  /** Returns problems to mark inline (non-blocking). */
  lint?: (text: string) => EditorIssue[];
  onChange?: (value: string) => void;
  /**
   * The tab's variables: enables state-aware `{{var}}` highlighting, autocomplete and
   * hover. Without it, references get a plain highlight.
   */
  variables?: () => VariableContext | undefined;
  /** Called once the editor has been created. */
  onReady?: (view: EditorView) => void;
  ariaLabel?: string;
}

function languageExtension(language: EditorLanguage | undefined): Extension {
  if (language === "json") return json();
  if (language === "xml") return xml();
  return [];
}

/**
 * CodeMirror 6 editor for request bodies and small responses (up to the size limit in
 * ADR 0003). Ctrl+F opens CodeMirror's search panel.
 */
export function SmallBodyEditor(props: SmallBodyEditorProps) {
  let host!: HTMLDivElement;
  let view: EditorView | undefined;
  const language = new Compartment();
  const readOnly = new Compartment();
  const lint = new Compartment();
  // Set while applying a prop change, so it isn't echoed back through onChange.
  let applying = false;

  const lintExtension = (): Extension => {
    const source = props.lint;
    if (!source) return [];
    return [
      lintGutter(),
      linter(
        (v): Diagnostic[] =>
          source(v.state.doc.toString()).map((i) => ({ ...i, severity: "error" as const })),
        { delay: 300 },
      ),
    ];
  };

  onMount(() => {
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: props.value,
        extensions: [
          basicSetup,
          language.of(languageExtension(props.language)),
          readOnly.of(EditorState.readOnly.of(props.readOnly ?? false)),
          lint.of(lintExtension()),
          props.variables ? varExtensions(() => props.variables?.()) : variableHighlight,
          theme,
          syntaxTheme,
          EditorView.contentAttributes.of({ "aria-label": props.ariaLabel ?? "Body" }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !applying) props.onChange?.(u.state.doc.toString());
          }),
        ],
      }),
    });
    props.onReady?.(view);
    onCleanup(() => view?.destroy());
  });

  createEffect(
    on(
      () => props.value,
      (value) => {
        if (!view || view.state.doc.toString() === value) return;
        applying = true;
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
        applying = false;
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => props.variables?.(),
      () => view?.dispatch({ effects: refreshVars.of(null) }),
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => props.language,
      (l) => view?.dispatch({ effects: language.reconfigure(languageExtension(l)) }),
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => props.readOnly,
      (r) => view?.dispatch({ effects: readOnly.reconfigure(EditorState.readOnly.of(r ?? false)) }),
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => props.lint,
      () => view?.dispatch({ effects: lint.reconfigure(lintExtension()) }),
      { defer: true },
    ),
  );

  return <div ref={host} style={{ height: "100%" }} />;
}
