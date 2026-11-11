// Presentation helpers for tool calls (vendor-neutral tool names).

/** One line describing a tool call: the argument a person would look for. */
export function summarize(name: string, input: unknown, cwd: string): string {
  const i = (input ?? {}) as Record<string, unknown>;
  const str = (k: string) => (typeof i[k] === "string" ? (i[k] as string) : "");
  const rel = (p: string) => (cwd && p.startsWith(cwd + "/") ? p.slice(cwd.length + 1) : p);
  switch (name) {
    case "Bash":
      return str("command");
    case "Read":
    case "Write":
    case "Edit":
    case "NotebookEdit":
      return rel(str("file_path") || str("notebook_path"));
    case "Grep":
    case "Glob":
      return str("pattern");
    case "WebFetch":
      return str("url");
    case "WebSearch":
      return str("query");
    case "Task":
    case "Agent":
      return str("description");
    default: {
      const first = Object.values(i).find((v) => typeof v === "string");
      return typeof first === "string" ? first : "";
    }
  }
}

/** The id in a `spawn` result ("started <id> '<title>' in …"), once the call has returned. */
export function spawnedId(name: string, result: { text: string; error: boolean } | undefined): string | null {
  if (name !== "mcp__atelier__spawn" || !result || result.error) return null;
  return /^started (\S+) /.exec(result.text)?.[1] ?? null;
}

/** What a `send` call was addressed to and said: the target as typed (id or prefix) and the text's first line. */
export function sendSummary(name: string, input: unknown): { to: string; line: string } | null {
  if (name !== "mcp__atelier__send") return null;
  const i = (input ?? {}) as { to?: unknown; text?: unknown };
  if (typeof i.to !== "string") return null;
  const line = (typeof i.text === "string" ? i.text : "").split("\n").find((l) => l.trim()) ?? "";
  return { to: i.to, line: line.trim() };
}

// ---- Rich tool views (pure; components live in ui/tools/) ----

export interface DiffLine {
  op: " " | "+" | "-";
  text: string;
}

export const splitLines = (s: string): string[] => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));

/** Above this many cells the LCS table is skipped and the middle is shown as delete-all/insert-all. */
const LCS_CELLS = 250_000;

/** Line diff of two strings: shared head/tail trimmed, LCS on the middle. */
export function diffLines(a: string, b: string): DiffLine[] {
  const x = splitLines(a);
  const y = splitLines(b);
  let head = 0;
  while (head < x.length && head < y.length && x[head] === y[head]) head++;
  let tail = 0;
  while (tail < x.length - head && tail < y.length - head && x[x.length - 1 - tail] === y[y.length - 1 - tail]) tail++;
  const mx = x.slice(head, x.length - tail);
  const my = y.slice(head, y.length - tail);
  const out: DiffLine[] = x.slice(0, head).map((text) => ({ op: " ", text }));

  if (mx.length * my.length > LCS_CELLS) {
    for (const text of mx) out.push({ op: "-", text });
    for (const text of my) out.push({ op: "+", text });
  } else {
    const w = my.length + 1;
    const t = new Uint32Array((mx.length + 1) * w);
    for (let i = mx.length - 1; i >= 0; i--)
      for (let j = my.length - 1; j >= 0; j--)
        t[i * w + j] = mx[i] === my[j] ? t[(i + 1) * w + j + 1]! + 1 : Math.max(t[(i + 1) * w + j]!, t[i * w + j + 1]!);
    let i = 0;
    let j = 0;
    while (i < mx.length || j < my.length) {
      if (i < mx.length && j < my.length && mx[i] === my[j]) out.push({ op: " ", text: mx[i++]! }), j++;
      else if (j >= my.length || (i < mx.length && t[(i + 1) * w + j]! >= t[i * w + j + 1]!)) out.push({ op: "-", text: mx[i++]! });
      else out.push({ op: "+", text: my[j++]! });
    }
  }
  for (const text of x.slice(x.length - tail)) out.push({ op: " ", text });
  return out;
}

export const diffCounts = (d: DiffLine[]) => ({
  add: d.filter((l) => l.op === "+").length,
  del: d.filter((l) => l.op === "-").length,
});

export interface Todo {
  content: string;
  status: string;
}

export function todos(input: unknown): Todo[] {
  const list = (input as { todos?: unknown })?.todos;
  if (!Array.isArray(list)) return [];
  return list.map((t) => ({ content: String(t?.content ?? t?.activeForm ?? ""), status: String(t?.status ?? "pending") }));
}

/** Exit code from a failed Bash result, if the CLI reported one. */
export const exitCode = (text: string): number | null => {
  const m = /Exit code (\d+)/.exec(text);
  return m ? Number(m[1]) : null;
};

/** The right-hand figure in a tool's summary line (`+48 −3`, `120 lines`, `7 matches`). */
export function toolStat(name: string, input: unknown, result: { text: string; error: boolean } | undefined): string {
  const i = (input ?? {}) as Record<string, unknown>;
  switch (name) {
    case "Edit": {
      const { add, del } = diffCounts(diffLines(String(i.old_string ?? ""), String(i.new_string ?? "")));
      return `+${add} −${del}${i.replace_all ? " all" : ""}`;
    }
    case "Write": {
      const n = splitLines(String(i.content ?? "")).length;
      return `${n} ${n === 1 ? "line" : "lines"}`;
    }
    case "Read": {
      const off = Number(i.offset) || 0;
      const lim = Number(i.limit) || 0;
      return off || lim ? `${off || 1}–${lim ? (off || 1) + lim - 1 : ""}` : "";
    }
    case "Grep":
    case "Glob": {
      if (!result || result.error) return "";
      const n = result.text.split("\n").filter((l) => l.trim() && !/^No (files|matches) found/.test(l)).length;
      return `${n} ${name === "Glob" ? (n === 1 ? "file" : "files") : n === 1 ? "match" : "matches"}`;
    }
    case "Bash": {
      if (!result?.error) return "";
      const c = exitCode(result.text);
      return c == null ? "failed" : `exit ${c}`;
    }
    case "TodoWrite": {
      const t = todos(input);
      return t.length ? `${t.filter((x) => x.status === "completed").length}/${t.length}` : "";
    }
    default:
      return "";
  }
}

/** Last `n` lines, and how many were dropped. */
export function tail(text: string, n: number): { text: string; dropped: number } {
  const lines = text.split("\n");
  return lines.length <= n ? { text, dropped: 0 } : { text: lines.slice(-n).join("\n"), dropped: lines.length - n };
}

// ---- Tiny highlighter: one regex, per line, language-agnostic ----

export type TokenKind = "kw" | "fn" | "str" | "com" | "num";
export interface Token {
  kind: TokenKind | null;
  text: string;
}

const KEYWORDS =
  "as async await break case catch class const continue def default do else enum export extends fn for from function if impl import in interface let loop match mod new null None nil pub return self static struct super switch this throw trait true false True False try type use var while with yield";
const TOKEN_RE = new RegExp(
  [
    String.raw`(?<com>\/\/.*|#.*|\/\*.*?\*\/)`,
    String.raw`(?<str>"(?:[^"\\]|\\.)*"?|'(?:[^'\\]|\\.)*'?|\`(?:[^\`\\]|\\.)*\`?)`,
    String.raw`(?<num>\b\d[\d_.]*\b)`,
    String.raw`(?<call>[A-Za-z_]\w*(?=\())`,
    String.raw`(?<id>[A-Za-z_]\w*)`,
  ].join("|"),
  "g",
);
const KW = new Set(KEYWORDS.split(" "));

export function highlightLine(line: string): Token[] {
  const out: Token[] = [];
  let at = 0;
  for (const m of line.matchAll(TOKEN_RE)) {
    const g = m.groups!;
    let kind: TokenKind | null = null;
    if (g.com) kind = "com";
    else if (g.str) kind = "str";
    else if (g.num) kind = "num";
    else if (g.call) kind = KW.has(g.call) ? "kw" : "fn";
    else if (g.id) kind = KW.has(g.id) ? "kw" : null;
    if (!kind) continue;
    if (m.index! > at) out.push({ kind: null, text: line.slice(at, m.index) });
    out.push({ kind, text: m[0] });
    at = m.index! + m[0].length;
  }
  if (at < line.length) out.push({ kind: null, text: line.slice(at) });
  return out;
}

/** Highlighting is skipped beyond this many lines. */
export const HIGHLIGHT_MAX_LINES = 2000;
