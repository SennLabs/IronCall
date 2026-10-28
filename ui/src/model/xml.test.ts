import { describe, expect, it } from "vitest";
import { formatXml, lintXml } from "./xml";

describe("xml", () => {
  it("re-indents nested elements and keeps attributes and the declaration", () => {
    expect(formatXml('<?xml version="1.0"?><a x="1"><b>hi</b><c/><!--n--></a>')).toBe(
      '<?xml version="1.0"?>\n<a x="1">\n  <b>hi</b>\n  <c/>\n  <!--n-->\n</a>\n',
    );
  });

  it("keeps {{vars}} in text and attributes", () => {
    expect(formatXml('<a id="{{id}}"><b>{{name}}</b></a>')).toBe(
      '<a id="{{id}}">\n  <b>{{name}}</b>\n</a>\n',
    );
  });

  it("reports malformed XML", () => {
    expect(lintXml("<a><b></a>")).not.toBeNull();
    expect(formatXml("<a>")).toBeNull();
    expect(lintXml("")).toBeNull();
  });
});
