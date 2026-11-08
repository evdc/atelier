import { onMount } from "solid-js";
import { HIGHLIGHT_MAX_LINES, highlightLine } from "../../agents/tools";

export interface CodeLine {
  text: string;
  /** Diff marker; tints the row. */
  op?: " " | "+" | "-";
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");
const CHUNK = 80;

function rowHtml(l: CodeLine, hl: boolean): string {
  const body = hl
    ? highlightLine(l.text)
        .map((t) => (t.kind ? `<i class="t-${t.kind}">${esc(t.text)}</i>` : esc(t.text)))
        .join("")
    : esc(l.text);
  const cls = l.op === "+" ? "add" : l.op === "-" ? "del" : "";
  const mark = l.op === "+" ? "+" : l.op === "-" ? "−" : l.op ? " " : "";
  return `<div class="ln ${cls}">${l.op ? `<s>${mark}</s>` : ""}${body || " "}</div>`;
}

/**
 * Code block rendered in frame-sized chunks, so mounting a long file never
 * holds the main thread. Only mounted when its <details> is open.
 */
export function Code(props: { lines: CodeLine[] }) {
  let box!: HTMLPreElement;
  onMount(() => {
    const lines = props.lines;
    const hl = lines.length <= HIGHLIGHT_MAX_LINES;
    let at = 0;
    let raf = 0;
    const step = () => {
      const end = Math.min(at + CHUNK, lines.length);
      let html = "";
      for (; at < end; at++) html += rowHtml(lines[at]!, hl);
      box.insertAdjacentHTML("beforeend", html);
      if (at < lines.length) raf = requestAnimationFrame(step);
    };
    step();
    return () => cancelAnimationFrame(raf);
  });
  return <pre class="code" ref={box} />;
}
