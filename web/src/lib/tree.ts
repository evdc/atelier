import type { SessionRow } from "../gen/SessionRow";

export type Node = {
  row: SessionRow;
  depth: number;
  /** Descendants in `rows`, shown or not. */
  kids: number;
  /** Descendants left out because this parent is collapsed. */
  hidden: number;
};

/**
 * Orders `rows` so each spawned session follows its parent, one level of indent per depth. Siblings keep the
 * incoming order. A row whose parent isn't in `rows` (another list, or gone) stands at the top level.
 * Children of an archived parent are collapsed into a count by default, those of a live parent shown;
 * `toggled` holds the parents the user flipped from that default.
 */
export function nest(rows: SessionRow[], toggled: ReadonlySet<string> = new Set()): Node[] {
  const ids = new Set(rows.map((r) => r.id));
  const kids = new Map<string, SessionRow[]>();
  const roots: SessionRow[] = [];
  for (const r of rows) {
    if (r.parent && ids.has(r.parent) && r.parent !== r.id) {
      const list = kids.get(r.parent);
      if (list) list.push(r);
      else kids.set(r.parent, [r]);
    } else roots.push(r);
  }
  const out: Node[] = [];
  const seen = new Set<string>();
  const count = (id: string): number => (kids.get(id) ?? []).reduce((n, c) => n + 1 + (seen.has(c.id) ? 0 : count(c.id)), 0);
  const walk = (r: SessionRow, depth: number) => {
    if (seen.has(r.id)) return; // a cycle can't be logged, but never loop on one
    seen.add(r.id);
    const n = count(r.id);
    const collapsed = !!r.archived !== toggled.has(r.id);
    out.push({ row: r, depth, kids: n, hidden: collapsed ? n : 0 });
    if (!collapsed) for (const c of kids.get(r.id) ?? []) walk(c, depth + 1);
  };
  for (const r of roots) walk(r, 0);
  return out;
}

/**
 * Splits nested nodes into whole trees by whether any member satisfies `pick`, so a child is never
 * separated from its parent by a grouping (e.g. a running child under a finished parent).
 */
export function splitTrees(nodes: Node[], pick: (r: SessionRow) => boolean): [Node[], Node[]] {
  const yes: Node[] = [];
  const no: Node[] = [];
  let tree: Node[] = [];
  const flush = () => {
    if (tree.length) (tree.some((n) => pick(n.row)) ? yes : no).push(...tree);
    tree = [];
  };
  for (const n of nodes) {
    if (n.depth === 0) flush();
    tree.push(n);
  }
  flush();
  return [yes, no];
}
