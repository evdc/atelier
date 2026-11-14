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
