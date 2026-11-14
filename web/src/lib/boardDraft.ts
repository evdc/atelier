// Editing a board config: pure draft operations and validation. The panel holds a draft and replaces it with
// the result of one of these per edit; nothing here touches the server.

import type { BoardConfig } from "../gen/BoardConfig";
import type { TicketRow } from "../gen/TicketRow";

export const DEFAULTS: BoardConfig = {
  statuses: ["open", "in-progress", "done"],
  columns: ["open", "in-progress", "done"].map((s) => ({ title: s, statuses: [s], collapsed: false })),
};

export const clone = (c: BoardConfig): BoardConfig => structuredClone(c);

const without = <T>(xs: readonly T[], i: number) => xs.filter((_, j) => j !== i);

/** `xs` with the item at `i` moved by `by` places; unchanged at the ends. */
export function move<T>(xs: readonly T[], i: number, by: -1 | 1): T[] {
  const j = i + by;
  if (j < 0 || j >= xs.length) return [...xs];
  const out = [...xs];
  [out[i], out[j]] = [out[j]!, out[i]!];
  return out;
}

export const addColumn = (c: BoardConfig, title: string, status: string): BoardConfig => ({
  statuses: c.statuses.includes(status) ? c.statuses : [...c.statuses, status],
  // A status lives in one column, so a new column takes it over.
  columns: [...c.columns.map((col) => ({ ...col, statuses: col.statuses.filter((s) => s !== status) })), { title, statuses: [status], collapsed: false }],
});

export const removeColumn = (c: BoardConfig, i: number): BoardConfig => ({ ...c, columns: without(c.columns, i) });
export const moveColumn = (c: BoardConfig, i: number, by: -1 | 1): BoardConfig => ({ ...c, columns: move(c.columns, i, by) });
export const renameColumn = (c: BoardConfig, i: number, title: string): BoardConfig =>
  patch(c, i, (col) => ({ ...col, title }));
export const toggleCollapsed = (c: BoardConfig, i: number): BoardConfig =>
  patch(c, i, (col) => ({ ...col, collapsed: !col.collapsed }));

const patch = (c: BoardConfig, i: number, f: (col: BoardConfig["columns"][number]) => BoardConfig["columns"][number]): BoardConfig => ({
  ...c,
  columns: c.columns.map((col, j) => (j === i ? f(col) : col)),
});

/** Puts `status` in column `i` (taking it from whichever column held it) and lists it if it is new. */
export const addChip = (c: BoardConfig, i: number, status: string): BoardConfig => ({
  statuses: c.statuses.includes(status) ? c.statuses : [...c.statuses, status],
  columns: c.columns.map((col, j) =>
    j === i
      ? { ...col, statuses: col.statuses.includes(status) ? col.statuses : [...col.statuses, status] }
      : { ...col, statuses: col.statuses.filter((s) => s !== status) },
  ),
});
export const removeChip = (c: BoardConfig, i: number, k: number): BoardConfig =>
  patch(c, i, (col) => ({ ...col, statuses: without(col.statuses, k) }));
export const moveChip = (c: BoardConfig, i: number, k: number, by: -1 | 1): BoardConfig =>
  patch(c, i, (col) => ({ ...col, statuses: move(col.statuses, k, by) }));

export const addStatus = (c: BoardConfig, status: string): BoardConfig =>
  c.statuses.includes(status) ? c : { ...c, statuses: [...c.statuses, status] };

/** Renames in the list and in every column. Tickets are not rewritten; that is the caller's to explain. */
export const renameStatus = (c: BoardConfig, from: string, to: string): BoardConfig => {
  const re = (s: string) => (s === from ? to : s);
  return { statuses: c.statuses.map(re), columns: c.columns.map((col) => ({ ...col, statuses: col.statuses.map(re) })) };
};

/** Drops the status from the list and from the column holding it. */
export const removeStatus = (c: BoardConfig, status: string): BoardConfig => ({
  statuses: c.statuses.filter((s) => s !== status),
  columns: c.columns.map((col) => ({ ...col, statuses: col.statuses.filter((s) => s !== status) })),
});

/**
 * The problems `board.update` would refuse the draft for: the rules of `board::effective` in the server
 * (same wording), plus empty or repeated names, which the server doesn't check.
 */
export function problems(c: BoardConfig): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const s of c.statuses) {
    if (seen.has(s)) out.push(`status \`${s}\` is listed more than once in statuses`);
    seen.add(s);
  }
  const listed = new Set(c.statuses);
  const home = new Map<string, string>();
  for (const col of c.columns) {
    const own: string[] = [];
    const inCol = new Set<string>();
    for (const s of col.statuses) {
      if (inCol.has(s)) {
        out.push(`status \`${s}\` is listed more than once in column \`${col.title}\``);
        continue;
      }
      inCol.add(s);
      const first = home.get(s);
      if (first !== undefined) out.push(`status \`${s}\` is in more than one column; kept in \`${first}\``);
      else {
        home.set(s, col.title);
        own.push(s);
      }
    }
    if (!own.length) {
      out.push(`column \`${col.title}\` has no statuses; dropped`);
      continue;
    }
    for (const s of own)
      if (!listed.has(s)) {
        out.push(`status \`${s}\` is used by column \`${col.title}\` but not listed in statuses; added`);
        listed.add(s);
      }
  }
  const titles = new Set<string>();
  for (const col of c.columns) {
    if (!col.title.trim()) out.push("a column has no title");
    else if (titles.has(col.title)) out.push(`column title \`${col.title}\` is used more than once`);
    titles.add(col.title);
  }
  if (c.statuses.some((s) => !s.trim())) out.push("a status is empty");
  return out;
}

/** Statuses tickets have that the draft doesn't list, with how many tickets; they would show under Other. */
export function orphaned(c: BoardConfig, tickets: readonly TicketRow[]): { status: string; count: number }[] {
  const known = new Set([...c.statuses, ...c.columns.flatMap((col) => col.statuses)]);
  const n = new Map<string, number>();
  for (const t of tickets) if (t.status !== "" && !known.has(t.status)) n.set(t.status, (n.get(t.status) ?? 0) + 1);
  return [...n].map(([status, count]) => ({ status, count }));
}

/** How many tickets have `status` (for the warning beside a status that is about to go). */
export const usedBy = (tickets: readonly TicketRow[], status: string) => tickets.filter((t) => t.status === status).length;
