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
