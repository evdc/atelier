// A watched server view as a reactive, keyed store.

import { createMemo, createSignal, onCleanup, type Accessor } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import type { ViewName } from "../gen/ViewName";
import type { Conn } from "./conn";

export interface View<Row> {
  /** Rows in display order. */
  list: Accessor<Row[]>;
  get(id: string): Row | undefined;
  /** The first snapshot has arrived. */
  ready: Accessor<boolean>;
}

/** Must be called inside a reactive owner (component or createRoot). */
export function createView<Row extends { id: string }>(
  conn: Conn,
  view: ViewName,
  scope: string | null,
  order: (a: Row, b: Row) => number,
): View<Row> {
  const [rows, setRows] = createStore<Record<string, Row>>({});
  const [ready, setReady] = createSignal(false);

  const stop = conn.watch<Row>(view, scope, {
    snapshot(all) {
      const next: Record<string, Row> = {};
      for (const r of all) next[r.id] = r;
      // reconcile keeps object identity for unchanged rows → no re-render.
      setRows(reconcile(next, { key: "id" }));
      setReady(true);
    },
    patch(upsert, remove) {
      for (const r of upsert) setRows(r.id, reconcile(r));
      if (remove.length) setRows(produce((d) => remove.forEach((id) => delete d[id])));
    },
  });
  onCleanup(stop);

  const list = createMemo(() => Object.values(rows).sort(order));
  return { list, get: (id) => rows[id], ready };
}

export const byRecent = <Row extends { updated_at: number }>(a: Row, b: Row) => b.updated_at - a.updated_at;
