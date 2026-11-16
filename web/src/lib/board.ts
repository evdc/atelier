// Which ticket goes in which column. Pure; the board UI and drag-and-drop both build on it.

import type { BoardConfig } from "../gen/BoardConfig";
import type { TicketRow } from "../gen/TicketRow";

export interface Column {
  title: string;
  /** What dropping a ticket here sets (the column's first status); null for "Other", which accepts no drops. */
  status: string | null;
  /** Every status this column shows. */
  statuses: string[];
  collapsed: boolean;
  tickets: TicketRow[];
}

/** `AT-9` before `AT-10`. */
export const byNaturalId = (a: { id: string }, b: { id: string }) =>
  a.id.localeCompare(b.id, "en", { numeric: true });

/**
 * A ticket goes to the column holding its status; an empty status to the first column. A status no column holds
 * lands in a trailing "Other" (only when non-empty), so no config can hide a ticket.
 */
export function columns(config: BoardConfig, tickets: readonly TicketRow[]): Column[] {
  const out: Column[] = config.columns.map((c) => ({
    title: c.title,
    status: c.statuses[0] ?? null,
    statuses: c.statuses,
    collapsed: c.collapsed,
    tickets: [],
  }));
  const home = new Map<string, Column>();
  for (const c of out) for (const s of c.statuses) if (!home.has(s)) home.set(s, c);
  const other: Column = { title: "Other", status: null, statuses: [], collapsed: false, tickets: [] };
  for (const t of tickets) {
    const c = t.status === "" ? (out[0] ?? other) : (home.get(t.status) ?? other);
    c.tickets.push(t);
  }
  if (other.tickets.length) out.push(other);
  for (const c of out) c.tickets.sort(byNaturalId);
  return out;
}

/** A move the user made that the server hasn't confirmed: the status the card had, and the one we asked for. */
export interface PendingMove {
  from: string;
  to: string;
}
export type Pending = ReadonlyMap<string, PendingMove>;

/**
 * Show pending moves on top of the rows, keyed by `TicketRow.key`. A move applies only while the row still has
 * its `from` status: once the row reads `to` it is confirmed, and if someone else changed it to anything else
 * their change wins, so the overlay never hides a newer remote edit.
 */
export function overlay(tickets: readonly TicketRow[], pending: Pending): readonly TicketRow[] {
  if (!pending.size) return tickets;
  return tickets.map((t) => {
    const p = pending.get(t.key);
    return p && t.status === p.from ? { ...t, status: p.to } : t;
  });
}

/** Drop entries the rows have outgrown (confirmed, changed elsewhere, or deleted). Returns `pending` itself if none. */
export function settle(pending: Pending, tickets: readonly TicketRow[]): Pending {
  if (!pending.size) return pending;
  const status = new Map(tickets.map((t) => [t.key, t.status]));
  let next: Map<string, PendingMove> | undefined;
  for (const [k, p] of pending) {
    if (status.get(k) === p.from) continue;
    next ??= new Map(pending);
    next.delete(k);
  }
  return next ?? pending;
}

/** `pending` with one more move, or without `key`. */
export function withMove(pending: Pending, key: string, move: PendingMove | null): Pending {
  const next = new Map(pending);
  if (move) next.set(key, move);
  else next.delete(key);
  return next;
}
