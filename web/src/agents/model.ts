// The vendor-neutral transcript: what the session view renders.
//
// Items are append-mostly: an item, once added, only changes in place (a tool
// gets its result, a permission gets answered). That keeps rendering
// incremental: finished items never re-render.

import type { Image } from "../gen/Image";

export type Item =
  | { k: "user"; id: string; ts: number; text: string; images: Image[] }
  /** From another session. `text` is the sender's own words: the envelope the agent reads is never stored here. */
  | { k: "message"; id: string; ts: number; from: string; text: string; refs: number[] }
  | { k: "text"; id: string; ts: number; text: string; sub?: string }
  | { k: "thinking"; id: string; ts: number; text: string; sub?: string }
  | {
      k: "tool";
      id: string; // the vendor's tool_use id
      ts: number;
      name: string;
      input: unknown;
      result?: { text: string; error: boolean };
      sub?: string;
    }
  | {
      k: "permission";
      id: string; // request id
      ts: number;
      tool: string;
      input: unknown;
      state: "pending" | "allowed" | "denied" | "void";
      /** The vendor's offered rules ("always allow"), verbatim from the request. */
      suggestions: PermissionSuggestion[];
    }
  | { k: "turn"; id: string; ts: number; error: boolean; cost: number | null; ms: number | null }
  | { k: "note"; id: string; ts: number; text: string; tone: "quiet" | "warn" };
// `sub`: set when the item comes from a subagent (the parent's tool_use id).

/** One of Claude Code's `permission_suggestions`; only the fields the UI reads are typed. */
export interface PermissionSuggestion {
  type: string;
  behavior?: string;
  rules?: { toolName: string; ruleContent?: string }[];
  destination?: string;
}

export interface Transcript {
  items: Item[];
  /** The model the agent reported at its latest init. */
  model: string | null;
  /** Tokens the latest request sent as context (input + cache), for the header. */
  context: number | null;
  /** Index of tool items by tool_use id, for attaching results. */
  tools: Record<string, number>;
  /** Index of permission items by request id. */
  perms: Record<string, number>;
}

export const emptyTranscript = (): Transcript => ({ items: [], model: null, context: null, tools: {}, perms: {} });
