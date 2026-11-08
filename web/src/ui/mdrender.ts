import { Marked, type Tokens } from "marked";

const ESC: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
export const esc = (s: string) => s.replace(/[&<>"']/g, (c) => ESC[c]!);

/** Only http(s) and mailto survive; everything else (javascript:, data:, relative) is dropped. */
export function safeHref(href: string): string | null {
  // Browsers ignore control chars/whitespace inside the scheme ("java\nscript:"), so test the normalized form.
  const norm = href.replace(/[\u0000- \u007f-\u009f]/g, "");
  return /^(https?:|mailto:)/i.test(norm) ? href.trim() : null;
}

// Raw HTML is never passed through: it is escaped and shown as text. Images are not fetched; shown as their alt text.
const md = new Marked({
  gfm: true,
  async: false,
  renderer: {
    html: ({ text }: Tokens.HTML | Tokens.Tag) => esc(text),
    image: ({ text }: Tokens.Image) => esc(text),
    link({ href, title, tokens }: Tokens.Link) {
      const inner = this.parser.parseInline(tokens);
      const safe = safeHref(href);
      if (!safe) return inner;
      const t = title ? ` title="${esc(title)}"` : "";
      return `<a href="${esc(safe)}"${t} target="_blank" rel="noreferrer noopener">${inner}</a>`;
    },
  },
});

export function renderMarkdown(src: string): string {
  return md.parse(src) as string;
}
