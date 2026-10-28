/** XML beautify for request bodies, using the webview's DOMParser. */

const INDENT = "  ";

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return escapeText(s).replace(/"/g, "&quot;");
}

function serialize(node: Node, depth: number, out: string[]) {
  const pad = INDENT.repeat(depth);
  switch (node.nodeType) {
    case Node.ELEMENT_NODE: {
      const el = node as Element;
      const attrs = Array.from(el.attributes)
        .map((a) => ` ${a.name}="${escapeAttr(a.value)}"`)
        .join("");
      const children = Array.from(el.childNodes).filter(
        (c) => !(c.nodeType === Node.TEXT_NODE && (c.textContent ?? "").trim() === ""),
      );
      if (children.length === 0) {
        out.push(`${pad}<${el.tagName}${attrs}/>`);
      } else if (children.length === 1 && children[0]?.nodeType === Node.TEXT_NODE) {
        out.push(
          `${pad}<${el.tagName}${attrs}>${escapeText((children[0].textContent ?? "").trim())}</${el.tagName}>`,
        );
      } else {
        out.push(`${pad}<${el.tagName}${attrs}>`);
        for (const c of children) serialize(c, depth + 1, out);
        out.push(`${pad}</${el.tagName}>`);
      }
      break;
    }
    case Node.TEXT_NODE:
      out.push(pad + escapeText((node.textContent ?? "").trim()));
      break;
    case Node.CDATA_SECTION_NODE:
      out.push(`${pad}<![CDATA[${node.textContent ?? ""}]]>`);
      break;
    case Node.COMMENT_NODE:
      out.push(`${pad}<!--${node.textContent ?? ""}-->`);
      break;
    case Node.PROCESSING_INSTRUCTION_NODE: {
      const pi = node as ProcessingInstruction;
      out.push(`${pad}<?${pi.target} ${pi.data}?>`);
      break;
    }
  }
}

/** Returns an error message if the XML is malformed, else null. Empty text is valid. */
export function lintXml(text: string): string | null {
  if (text.trim() === "") return null;
  const doc = new DOMParser().parseFromString(text, "application/xml");
  const error = doc.getElementsByTagName("parsererror")[0];
  return error ? (error.textContent ?? "Invalid XML").trim().split("\n")[0] || "Invalid XML" : null;
}

/** Re-indents XML. Returns null if it doesn't parse. Keeps an XML declaration if present. */
export function formatXml(text: string): string | null {
  if (lintXml(text)) return null;
  const doc = new DOMParser().parseFromString(text, "application/xml");
  const out: string[] = [];
  const decl = /^\s*(<\?xml[^?]*\?>)/.exec(text);
  if (decl?.[1]) out.push(decl[1]);
  for (const child of Array.from(doc.childNodes)) serialize(child, 0, out);
  return `${out.join("\n")}\n`;
}
