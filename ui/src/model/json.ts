/**
 * JSON lint and beautify for request bodies.
 *
 * Both understand `{{var}}` placeholders used as values (`{"n": {{count}}}`), which
 * aren't JSON but are valid templates. They work on tokens rather than parsed values,
 * so numbers, escapes and key order are kept exactly as written. Error positions are
 * computed here because WebKit's `JSON.parse` messages don't include them.
 */

export interface JsonIssue {
  message: string;
  /** Zero-based character offset. */
  offset: number;
  /** One-based line and column. */
  line: number;
  column: number;
}

type Token =
  | { kind: "punct"; text: "{" | "}" | "[" | "]" | ":" | ","; at: number }
  | { kind: "string" | "value"; text: string; at: number };

class JsonSyntaxError extends Error {
  constructor(
    message: string,
    readonly at: number,
  ) {
    super(message);
  }
}

const LITERAL = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/;

function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i] as string;
    if (c === " " || c === "\t" || c === "\n" || c === "\r") {
      i++;
    } else if (text.startsWith("{{", i)) {
      // `{{` is never valid JSON, so it always starts a template placeholder.
      const end = text.indexOf("}}", i + 2);
      if (end < 0) throw new JsonSyntaxError("Unterminated {{variable}}", i);
      tokens.push({ kind: "value", text: text.slice(i, end + 2), at: i });
      i = end + 2;
    } else if ("{}[]:,".includes(c)) {
      tokens.push({ kind: "punct", text: c as "{", at: i });
      i++;
    } else if (c === '"') {
      const start = i++;
      while (i < text.length && text[i] !== '"') {
        if (text[i] === "\\") i++;
        else if (text[i] === "\n") throw new JsonSyntaxError("Unterminated string", start);
        i++;
      }
      if (i >= text.length) throw new JsonSyntaxError("Unterminated string", start);
      i++;
      tokens.push({ kind: "string", text: text.slice(start, i), at: start });
    } else {
      const m = LITERAL.exec(text.slice(i));
      if (!m) throw new JsonSyntaxError(`Unexpected character ${JSON.stringify(c)}`, i);
      tokens.push({ kind: "value", text: m[0], at: i });
      i += m[0].length;
    }
  }
  return tokens;
}

/** Validates token structure; throws JsonSyntaxError at the first problem. */
function validate(tokens: Token[], end: number) {
  let pos = 0;
  const peek = () => tokens[pos];
  const fail = (message: string): never => {
    throw new JsonSyntaxError(message, peek()?.at ?? end);
  };
  const expectPunct = (p: string) => {
    const t = peek();
    if (t?.kind !== "punct" || t.text !== p) fail(`Expected "${p}"`);
    pos++;
  };

  const value = (): void => {
    const t = peek();
    if (!t) {
      fail("Unexpected end of input");
      return;
    }
    if (t.kind === "string" || t.kind === "value") {
      pos++;
      return;
    }
    if (t.text === "{") {
      pos++;
      if (peek()?.text === "}") {
        pos++;
        return;
      }
      for (;;) {
        const key = peek();
        if (key?.kind !== "string" && !(key?.kind === "value" && key.text.startsWith("{{"))) {
          fail(key?.text === "}" ? "Trailing comma" : "Expected a string key");
        }
        pos++;
        expectPunct(":");
        value();
        if (peek()?.text === ",") {
          pos++;
          continue;
        }
        expectPunct("}");
        return;
      }
    }
    if (t.text === "[") {
      pos++;
      if (peek()?.text === "]") {
        pos++;
        return;
      }
      for (;;) {
        if (peek()?.text === "]") fail("Trailing comma");
        value();
        if (peek()?.text === ",") {
          pos++;
          continue;
        }
        expectPunct("]");
        return;
      }
    }
    fail(`Unexpected "${t.text}"`);
  };

  value();
  if (pos < tokens.length) fail("Unexpected content after the end of the JSON value");
}

function position(text: string, offset: number): { line: number; column: number } {
  const before = text.slice(0, offset);
  const line = before.split("\n").length;
  return { line, column: offset - before.lastIndexOf("\n") };
}

/** Returns the first syntax problem, or null if the text is valid (template) JSON. Empty text is valid. */
export function lintJson(text: string): JsonIssue | null {
  if (text.trim() === "") return null;
  try {
    validate(tokenize(text), text.length);
    return null;
  } catch (e) {
    if (!(e instanceof JsonSyntaxError)) throw e;
    return { message: e.message, offset: e.at, ...position(text, e.at) };
  }
}

/** Re-indents valid (template) JSON. Returns null if the text has a syntax error. */
export function formatJson(text: string, indent = 2): string | null {
  if (lintJson(text)) return null;
  const tokens = tokenize(text);
  const pad = (depth: number) => `\n${" ".repeat(depth * indent)}`;
  let out = "";
  let depth = 0;
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i] as Token;
    const next = tokens[i + 1];
    if (t.text === "{" || t.text === "[") {
      const close = t.text === "{" ? "}" : "]";
      if (next?.text === close) {
        out += t.text + close;
        i++;
      } else {
        depth++;
        out += t.text + pad(depth);
      }
    } else if (t.text === "}" || t.text === "]") {
      depth--;
      out += pad(depth) + t.text;
    } else if (t.text === "," && t.kind === "punct") {
      out += `,${pad(depth)}`;
    } else if (t.text === ":" && t.kind === "punct") {
      out += ": ";
    } else {
      out += t.text;
    }
  }
  return `${out}\n`;
}
