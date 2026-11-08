// Claude Code adapter: `cc.*` events → transcript items.
//
// CC emits one `assistant` event per content block (all sharing the message
// id), and tool results as `user` events carrying `tool_result` blocks.

import type { Event } from "../gen/Event";
import type { Adapter, StreamDelta } from "./fold";
import type { Transcript } from "./model";

interface Block {
  type: string;
  text?: string;
  thinking?: string;
  id?: string;
  name?: string;
  input?: unknown;
  tool_use_id?: string;
  content?: string | { type: string; text?: string }[];
  is_error?: boolean;
}

interface CcMessage {
  type: string;
  subtype?: string;
  model?: string;
  message?: { id?: string; content?: Block[] | string; usage?: unknown };
  parent_tool_use_id?: string | null;
}

export const claude: Adapter = {
  ns: "cc",
  fold(t: Transcript, ev: Event) {
    const m = ev.body as CcMessage;
    const sub = m.parent_tool_use_id ?? undefined;
    const blocks = Array.isArray(m.message?.content) ? m.message.content : [];

    if (ev.kind === "cc.assistant") {
      // A message's usage is that one request's; a subagent's requests have their own window.
      if (!sub) t.context = contextTokens(m.message?.usage) ?? t.context;
      blocks.forEach((b, i) => {
        const id = `e${ev.seq}.${i}`;
        if (b.type === "text" && b.text) {
          t.items.push({ k: "text", id, ts: ev.ts, text: b.text, sub });
        } else if (b.type === "thinking" && b.thinking) {
          t.items.push({ k: "thinking", id, ts: ev.ts, text: b.thinking, sub });
        } else if (b.type === "tool_use" && b.id) {
          t.tools[b.id] = t.items.length;
          t.items.push({ k: "tool", id: b.id, ts: ev.ts, name: b.name ?? "tool", input: b.input, sub });
        }
      });
    } else if (ev.kind === "cc.user") {
      for (const b of blocks) {
        if (b.type !== "tool_result" || !b.tool_use_id) continue;
        const item = t.items[t.tools[b.tool_use_id] ?? -1];
        if (item?.k === "tool") item.result = { text: resultText(b.content), error: !!b.is_error };
      }
    } else if (ev.kind === "cc.system" && m.subtype === "init") {
      if (m.model) t.model = m.model;
    }
    // Other cc.system (hooks…) and cc.rate_limit_event: not part of the transcript.
  },
  stream(kind, body): StreamDelta {
    if (kind !== "cc.stream_event") return {};
    const e = body as { type?: string; delta?: { type?: string; text?: string } };
    if (e.type === "content_block_start" || e.type === "message_stop") return { reset: true };
    if (e.type === "content_block_delta" && e.delta?.type === "text_delta") return { text: e.delta.text ?? "" };
    return {};
  },
};

/**
 * Context one API request sent: input plus both cache counts, from that request's
 * `usage`. Output tokens are excluded: they don't occupy the window yet.
 * Not the `result` message's usage: that sums cache reads over every request in
 * the turn, so a long tool-using turn reads as millions of tokens.
 */
export function contextTokens(usage: unknown): number | null {
  const u = usage as Record<string, unknown> | null | undefined;
  if (!u || typeof u.input_tokens !== "number") return null;
  const cache = ["cache_creation_input_tokens", "cache_read_input_tokens"].reduce(
    (sum, k) => sum + (typeof u[k] === "number" ? (u[k] as number) : 0),
    0,
  );
  return u.input_tokens + cache;
}

function resultText(c: Block["content"]): string {
  if (typeof c === "string") return c;
  if (!Array.isArray(c)) return "";
  return c.map((p) => (p.type === "text" ? p.text ?? "" : `[${p.type}]`)).join("\n");
}
