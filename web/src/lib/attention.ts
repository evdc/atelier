// Which sessions just started needing you, and how many do. Pure: the caller
// passes the rows, what is on screen and the clock, and keeps the memo between
// calls. The side effects (notifications, dock badge) live in notify.ts.

import type { SessionRow } from "../gen/SessionRow";
import type { SessionStatus } from "../gen/SessionStatus";

/** At most one notification per session in this window. */
export const COALESCE_MS = 30_000;

/** What the previous call saw: each live session's status, and when it was last notified. */
export interface AttentionMemo {
  status: Record<string, SessionStatus>;
  notifiedAt: Record<string, number>;
}

export interface AttentionInput {
  rows: readonly SessionRow[];
  now: number;
  /** The session on screen (if any), and whether the document is visible. */
  viewing: { session: string | null; visible: boolean };
}

export interface AttentionResult {
  memo: AttentionMemo;
  /** Sessions to notify about now, in row order. */
  notify: SessionRow[];
  /** Sessions waiting on you right now (the dock badge). */
  waiting: number;
}

const NEEDS_YOU: ReadonlySet<SessionStatus> = new Set<SessionStatus>(["waiting", "failed"]);

/** `prev` is null on the first snapshot: sessions that already need you then were not
 *  just asked anything, so they set the baseline and notify nothing. */
export function attention(prev: AttentionMemo | null, input: AttentionInput): AttentionResult {
  const live = input.rows.filter((r) => !r.archived);
  const status: Record<string, SessionStatus> = {};
  const notifiedAt: Record<string, number> = {};
  const notify: SessionRow[] = [];

  for (const r of live) {
    status[r.id] = r.status;
    const last = prev?.notifiedAt[r.id];
    if (last !== undefined) notifiedAt[r.id] = last;

    const entered = prev !== null && NEEDS_YOU.has(r.status) && prev.status[r.id] !== r.status;
    const onScreen = input.viewing.visible && input.viewing.session === r.id;
    const coalesced = last !== undefined && input.now - last < COALESCE_MS;
    if (entered && !onScreen && !coalesced) {
      notify.push(r);
      notifiedAt[r.id] = input.now;
    }
  }

  const waiting = live.filter((r) => r.status === "waiting").length;
  return { memo: { status, notifiedAt }, notify, waiting };
}
