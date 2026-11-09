// Event → transcript fold. Atelier's own kinds are handled here; vendor
// kinds (`cc.*`, later `codex.*`) are delegated to that vendor's adapter.
//
// Folds mutate a draft (inside a Solid `produce`) and must be pure functions
// of (draft, event): the same log always yields the same transcript.

import type { Event } from "../gen/Event";
import type { PermissionAnswered } from "../gen/PermissionAnswered";
import type { PermissionRequested } from "../gen/PermissionRequested";
import type { ProcessExited } from "../gen/ProcessExited";
import type { TurnEnded } from "../gen/TurnEnded";
import type { UserMessage } from "../gen/UserMessage";
import { claude } from "./claude";
import type { PermissionSuggestion, Transcript } from "./model";

export interface Adapter {
  /** Kind namespace, e.g. "cc". */
  ns: string;
  fold(t: Transcript, ev: Event): void;
  /** Interpret an ephemeral event: text to append to the streaming block,
   *  or `reset` when a new block starts. */
  stream?(kind: string, body: unknown): StreamDelta;
}

export interface StreamDelta {
  reset?: boolean;
  text?: string;
}

const adapters: Record<string, Adapter> = { [claude.ns]: claude };

const nsOf = (kind: string) => kind.slice(0, kind.indexOf("."));

export function streamDelta(kind: string, body: unknown): StreamDelta {
  return adapters[nsOf(kind)]?.stream?.(kind, body) ?? {};
}

export function fold(t: Transcript, ev: Event) {
  const id = `e${ev.seq}`;
  switch (ev.kind) {
    case "user.message":
      t.items.push({ k: "user", id, ts: ev.ts, text: (ev.body as UserMessage).text });
      return;
    case "user.interrupt":
      t.items.push({ k: "note", id, ts: ev.ts, text: "Interrupted", tone: "quiet" });
      return;
    case "permission.requested": {
      const b = ev.body as PermissionRequested;
      t.perms[b.request_id] = t.items.length;
      const suggestions = (b.raw as { permission_suggestions?: PermissionSuggestion[] } | null)?.permission_suggestions ?? [];
      t.items.push({
        k: "permission",
        id: b.request_id,
        ts: ev.ts,
        tool: b.tool,
        input: b.input,
        state: "pending",
        suggestions,
      });
      return;
    }
    case "permission.answered": {
      const b = ev.body as PermissionAnswered;
      const item = t.items[t.perms[b.request_id] ?? -1];
      if (item?.k === "permission") item.state = b.decision.behavior === "allow" ? "allowed" : "denied";
      return;
    }
    case "turn.ended": {
      const b = ev.body as TurnEnded;
      t.items.push({ k: "turn", id, ts: ev.ts, error: b.is_error, cost: b.cost_usd, ms: b.duration_ms });
      return;
    }
    case "process.exited": {
      const b = ev.body as ProcessExited;
      for (const i of Object.values(t.perms)) {
        const item = t.items[i];
        if (item?.k === "permission" && item.state === "pending") item.state = "void";
      }
      if (b.reason === "exited" && b.code !== 0) {
        const detail = b.stderr_tail ? `: ${b.stderr_tail.split("\n").slice(-3).join(" ")}` : "";
        t.items.push({ k: "note", id, ts: ev.ts, text: `Agent exited (code ${b.code ?? "?"})${detail}`, tone: "warn" });
      } else if (b.reason === "spawn-failed") {
        t.items.push({ k: "note", id, ts: ev.ts, text: `Couldn't start the agent: ${b.stderr_tail}`, tone: "warn" });
      } else if (b.reason === "server-restart") {
        t.items.push({ k: "note", id, ts: ev.ts, text: "Server restarted; the agent will resume on your next message", tone: "quiet" });
      }
      return;
    }
    default: {
      adapters[nsOf(ev.kind)]?.fold(t, ev);
    }
  }
}
