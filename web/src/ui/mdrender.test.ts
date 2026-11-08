import { describe, expect, it } from "vitest";
import { renderMarkdown, safeHref } from "./mdrender";

describe("renderMarkdown sanitizer", () => {
  it("escapes script tags", () => {
    const h = renderMarkdown("hi <script>alert(1)</script>");
    expect(h).not.toContain("<script");
    expect(h).toContain("&lt;script&gt;");
  });
  it("escapes block-level html and on* attributes", () => {
    const h = renderMarkdown('<img src=x onerror="alert(1)">\n\n<div onclick="x()">a</div>');
    expect(h).not.toMatch(/<img|<div/);
    expect(h).toContain("&lt;img");
  });
  it("drops javascript: links but keeps text", () => {
    for (const src of ["[x](javascript:alert(1))", "[x](JaVaScRiPt:alert(1))", "[x](data:text/html;base64,AAAA)", "[x](/rel)"]) {
      const h = renderMarkdown(src);
      expect(h).not.toContain("<a");
      expect(h).toContain("x");
    }
  });
  it("allows http(s) and mailto, opened safely", () => {
    const h = renderMarkdown("[a](https://e.com/?q=1&r=\"2\") [m](mailto:a@b.c)");
    expect(h).toContain('rel="noreferrer noopener"');
    expect(h).toContain('target="_blank"');
    expect(h).toContain("mailto:a@b.c");
    expect(h).not.toContain('"2"');
  });
  it("rejects control-char scheme obfuscation", () => {
    expect(safeHref("java\nscript:alert(1)")).toBeNull();
    expect(safeHref(" javascript:alert(1)")).toBeNull();
  });
  it("does not emit images", () => {
    expect(renderMarkdown("![alt](https://e.com/x.png)")).not.toContain("<img");
  });
  it("renders fences, lists and tables", () => {
    expect(renderMarkdown("```js\nlet a<b\n```")).toContain("<pre><code");
    expect(renderMarkdown("- a\n- b")).toContain("<ul>");
    expect(renderMarkdown("|a|b|\n|-|-|\n|1|2|")).toContain("<table>");
  });
});
