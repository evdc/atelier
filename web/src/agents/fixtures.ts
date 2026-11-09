// Dev-only: a realistic synthetic session, for measuring transcript rendering
// without a server. Reached via `?fixture=<n>` (see main.tsx).

import { createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import type { Event } from "../gen/Event";
import type { SessionRow } from "../gen/SessionRow";
import { fold } from "./fold";
import { emptyTranscript } from "./model";
import type { LiveTranscript } from "./transcript";

const PARA =
  "The projection keeps its state in its own tables, so a rebuild from the log yields the same rows. " +
  "That means the fold can't consult the clock, and anything time-dependent has to arrive as an event.";

/** About `n` transcript items: a user prompt, then rounds of prose, tool calls with results, and turn ends. */
export function fixtureEvents(n: number): Event[] {
  const evs: Event[] = [];
  const push = (kind: string, body: unknown, actor = "agent:fixture") =>
    evs.push({ seq: evs.length + 1, stream: "fixture", ts: 1_700_000_000_000 + evs.length * 1000, kind, actor, cause: null, body });
  const asst = (content: unknown[]) => push("cc.assistant", { type: "assistant", message: { id: `m${evs.length}`, content } });
  let items = 0;
  for (let r = 0; items < n; r++) {
    push("user.message", { text: `Round ${r}: please look at crate ${r % 7} and tidy it up.` }, "user");
    items++;
    asst([{ type: "text", text: `${PARA}\n\n- first point ${r}\n- second point\n\n\`code_${r}()\` is the entry.` }]);
    items++;
    for (let k = 0; k < 6 && items < n; k++) {
      const id = `t${r}_${k}`;
      const [name, input] =
        k % 3 === 0
          ? ["Read", { file_path: `/work/src/mod_${r}_${k}.rs` }]
          : k % 3 === 1
            ? ["Bash", { command: `cargo test -p crate${k}` }]
            : ["Grep", { pattern: `fn item_${k}` }];
      asst([{ type: "tool_use", id, name, input }]);
      push("cc.user", {
        type: "user",
        message: { content: [{ type: "tool_result", tool_use_id: id, content: Array.from({ length: 30 }, (_, i) => `line ${i} of output`).join("\n") }] },
      });
      items++;
    }
    push("turn.ended", { is_error: false, cost_usd: 0.12, duration_ms: 8000 });
    items++;
  }
  return evs;
}

export function fixtureSession(): SessionRow {
  return { id: "fixture", stream: "fixture", title: "fixture", workspace: { path: "/work" }, status: "idle", alive: false, pending: 0 } as unknown as SessionRow;
}

export function fixtureLive(n: number): LiveTranscript {
  const [t, setT] = createStore(emptyTranscript());
  setT(produce((d) => fixtureEvents(n).forEach((e) => fold(d, e))));
  const [streaming] = createSignal("");
  const [ready] = createSignal(true);
  return { t, streaming, ready };
}
