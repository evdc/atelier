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

  it("shows the typed text of an imported user entry, but not meta or tool results", () => {
    const t = emptyTranscript();
    fold(t, ev("cc.user", { type: "user", message: { content: "first question" } }));
    fold(t, ev("cc.user", { type: "user", isMeta: true, message: { content: "caveat" } }));
    fold(t, ev("cc.user", { type: "user", message: { content: [{ type: "text", text: "  block form  " }] } }));
    fold(t, ev("cc.user", { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "x", content: "out" }] } }));
    expect(t.items.map((i) => (i.k === "user" ? i.text : i.k))).toEqual(["first question", "block form"]);
  });

  it("folds an agent message into its own item, with the sender's words and not the envelope", () => {
    const t = emptyTranscript();
    fold(t, ev("agent.message", { from: "s9", text: "check the WAL", mode: "tell", hops: 1, refs: [4, 7] }, "agent:s9"));
    // The envelope the agent read can come back as a `cc.user` entry; it must not read as the person's text.
    const envelope = '<atelier-message from="s9" title="WAL" project="p" mode="tell">\ncheck the WAL\n</atelier-message>\n(From another session…)';
    fold(t, ev("cc.user", { type: "user", message: { content: envelope } }));
    fold(t, ev("cc.user", { type: "user", message: { content: [{ type: "text", text: envelope }] } }));
    expect(t.items).toEqual([{ k: "message", id: `e${seq - 2}`, ts: seq - 2, from: "s9", text: "check the WAL", refs: [4, 7] }]);
  });

  it("voids pending requests when the process exits", () => {
    const t = emptyTranscript();
    fold(t, ev("permission.requested", { request_id: "r", tool: "Bash", input: {}, raw: {} }));
    fold(t, ev("process.exited", { reason: "stopped", code: 0 }, "atelier"));
    expect(t.items[0]?.k === "permission" && t.items[0].state).toBe("void");
  });

  it("keeps the request's permission suggestions for \"always allow\"", () => {
    const t = emptyTranscript();
    const rule = { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "npm test" }], behavior: "allow" };
    fold(t, ev("permission.requested", { request_id: "r", tool: "Bash", input: {}, raw: { permission_suggestions: [rule] } }));
    fold(t, ev("permission.requested", { request_id: "q", tool: "Bash", input: {}, raw: {} }));
    expect(t.items[0]?.k === "permission" && t.items[0].suggestions).toEqual([rule]);
    expect(t.items[1]?.k === "permission" && t.items[1].suggestions).toEqual([]);
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
