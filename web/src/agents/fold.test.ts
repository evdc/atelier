import { describe, expect, it } from "vitest";
import type { Event } from "../gen/Event";
import { fold, streamDelta } from "./fold";
import { emptyTranscript } from "./model";

let seq = 0;
const ev = (kind: string, body: unknown, actor = "agent:s"): Event => ({
  seq: ++seq,
  stream: "p/x/s/s",
  ts: seq,
  kind,
  actor,
  cause: null,
  body,
});

describe("fold", () => {
  it("builds a transcript from a Claude Code turn", () => {
    const t = emptyTranscript();
    const log = [
      ev("user.message", { text: "list files" }, "user"),
      ev("cc.assistant", { type: "assistant", message: { id: "m1", content: [{ type: "thinking", thinking: "" }] } }),
      ev("cc.assistant", { type: "assistant", message: { id: "m1", content: [{ type: "text", text: "Sure." }] } }),
      ev("cc.assistant", {
        type: "assistant",
        message: { id: "m1", content: [{ type: "tool_use", id: "tu1", name: "Bash", input: { command: "ls" } }] },
      }),
      ev("permission.requested", { request_id: "r1", tool: "Bash", input: { command: "ls" }, raw: {} }),
      ev("permission.answered", { request_id: "r1", decision: { behavior: "allow" } }, "user"),
      ev("cc.user", { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "tu1", content: "a\nb" }] } }),
      ev("turn.ended", { is_error: false, cost_usd: 0.01, duration_ms: 1200, raw: {} }),
    ];
    log.forEach((e) => fold(t, e));

    expect(t.items.map((i) => i.k)).toEqual(["user", "text", "tool", "permission", "turn"]);
    const tool = t.items[2];
    expect(tool?.k === "tool" && tool.result).toEqual({ text: "a\nb", error: false });
    const perm = t.items[3];
    expect(perm?.k === "permission" && perm.state).toBe("allowed");
  });

  it("voids pending requests when the process exits", () => {
    const t = emptyTranscript();
    fold(t, ev("permission.requested", { request_id: "r", tool: "Bash", input: {}, raw: {} }));
    fold(t, ev("process.exited", { reason: "stopped", code: 0 }, "atelier"));
    expect(t.items[0]?.k === "permission" && t.items[0].state).toBe("void");
  });

  it("ignores unknown kinds", () => {
    const t = emptyTranscript();
    fold(t, ev("cc.rate_limit_event", {}));
    fold(t, ev("future.thing", {}));
    expect(t.items).toEqual([]);
  });

  it("takes the model from the latest init and the context from the latest request", () => {
    const t = emptyTranscript();
    const usage = (input: number, cache: number) => ({ input_tokens: input, cache_creation_input_tokens: 0, cache_read_input_tokens: cache, output_tokens: 900 });
    fold(t, ev("cc.system", { type: "system", subtype: "init", model: "claude-sonnet-5-5" }));
    fold(t, ev("cc.system", { type: "system", subtype: "hook_started" }));
    expect(t.model).toBe("claude-sonnet-5-5");
    expect(t.context).toBeNull();

    fold(t, ev("cc.assistant", { type: "assistant", message: { id: "m1", content: [{ type: "text", text: "a" }], usage: usage(2, 134778) } }));
    expect(t.context).toBe(134780);
    // A subagent's request doesn't stand in for the session's context.
    fold(t, ev("cc.assistant", { type: "assistant", parent_tool_use_id: "tu9", message: { id: "m2", content: [{ type: "text", text: "b" }], usage: usage(2, 5000) } }));
    expect(t.context).toBe(134780);
    // The turn total is a sum over all requests: not a context size.
    fold(t, ev("turn.ended", { is_error: false, raw: { usage: usage(3, 3448408) } }));
    expect(t.context).toBe(134780);

    fold(t, ev("cc.system", { type: "system", subtype: "init", model: "claude-opus-5-5" }));
    expect(t.model).toBe("claude-opus-5-5");
  });

  it("extracts streamed text", () => {
    expect(streamDelta("cc.stream_event", { type: "content_block_delta", delta: { type: "text_delta", text: "hi" } })).toEqual({ text: "hi" });
    expect(streamDelta("cc.stream_event", { type: "content_block_start" })).toEqual({ reset: true });
    expect(streamDelta("zz.x", {})).toEqual({});
  });
});
