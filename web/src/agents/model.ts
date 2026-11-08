// The vendor-neutral transcript: what the session view renders.
//
// Items are append-mostly: an item, once added, only changes in place (a tool
// gets its result, a permission gets answered). That keeps rendering
// incremental: finished items never re-render.

export type Item =
  | { k: "user"; id: string; ts: number; text: string }
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
    }
  | { k: "turn"; id: string; ts: number; error: boolean; cost: number | null; ms: number | null }
  | { k: "note"; id: string; ts: number; text: string; tone: "quiet" | "warn" };
// `sub`: set when the item comes from a subagent (the parent's tool_use id).

export interface Transcript {
  items: Item[];
  /** Index of tool items by tool_use id, for attaching results. */
  tools: Record<string, number>;
  /** Index of permission items by request id. */
  perms: Record<string, number>;
}

export const emptyTranscript = (): Transcript => ({ items: [], tools: {}, perms: {} });
