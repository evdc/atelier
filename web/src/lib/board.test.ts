import { describe, expect, it } from "vitest";
import type { BoardConfig } from "../gen/BoardConfig";
import type { TicketRow } from "../gen/TicketRow";
import { columns, overlay, settle, withMove, type Pending } from "./board";

const config: BoardConfig = {
  statuses: ["open", "in-progress", "review", "qa", "done"],
  columns: [
    { title: "Open", statuses: ["open"], collapsed: false },
    { title: "Doing", statuses: ["in-progress"], collapsed: false },
    { title: "Check", statuses: ["review", "qa"], collapsed: false },
    { title: "Done", statuses: ["done"], collapsed: true },
  ],
};
const t = (id: string, status: string): TicketRow => ({
  key: `p/${id}`, project: "p", path: `.atelier/tickets/${id}.md`, id, title: id, status, size: "", milestone: "",
  area: "", depends: [], checks_done: 0, checks_total: 0, updated_at: 0,
});
const ids = (c: { tickets: TicketRow[] }) => c.tickets.map((x) => x.id);

describe("columns", () => {
  it("maps each status to its column, several statuses to one", () => {
    const cols = columns(config, [t("A-1", "open"), t("A-2", "qa"), t("A-3", "review"), t("A-4", "done")]);
    expect(cols.map((c) => [c.title, ids(c)])).toEqual([
      ["Open", ["A-1"]], ["Doing", []], ["Check", ["A-2", "A-3"]], ["Done", ["A-4"]],
    ]);
    expect(cols[2]!.status).toBe("review");
    expect(cols[3]!.collapsed).toBe(true);
  });
  it("puts unknown statuses in a trailing Other, only when needed", () => {
    expect(columns(config, [t("A-1", "open")]).map((c) => c.title)).not.toContain("Other");
    const cols = columns(config, [t("A-1", "open"), t("A-2", "blocked")]);
    const last = cols[cols.length - 1]!;
    expect(last.title).toBe("Other");
    expect(last.status).toBeNull();
    expect(ids(last)).toEqual(["A-2"]);
  });
  it("puts empty status in the first column", () => {
    expect(ids(columns(config, [t("A-1", "")])[0]!)).toEqual(["A-1"]);
  });
  it("orders naturally by id", () => {
    const cols = columns(config, [t("AT-10", "open"), t("AT-9", "open"), t("AT-100", "open")]);
    expect(ids(cols[0]!)).toEqual(["AT-9", "AT-10", "AT-100"]);
  });
  it("never hides a ticket, even with no columns", () => {
    const cols = columns({ statuses: [], columns: [] }, [t("A-1", ""), t("A-2", "x")]);
    expect(cols.map((c) => [c.title, ids(c)])).toEqual([["Other", ["A-1", "A-2"]]]);
  });
});

describe("pending moves", () => {
  const mv = (k: string, from: string, to: string): Pending => withMove(new Map(), `p/${k}`, { from, to });
  const status = (rows: readonly TicketRow[], id: string) => rows.find((r) => r.id === id)!.status;

  it("shows the target status while the row still has the old one", () => {
    const rows = [t("A-1", "open"), t("A-2", "open")];
    const out = overlay(rows, mv("A-1", "open", "in-progress"));
    expect(status(out, "A-1")).toBe("in-progress");
    expect(out[1]).toBe(rows[1]);
    expect(rows[0]!.status).toBe("open");
  });
  it("is the identity with nothing pending", () => {
    const rows = [t("A-1", "open")];
    expect(overlay(rows, new Map())).toBe(rows);
  });
  it("is confirmed once the row reads the target, then cleared", () => {
    const p = mv("A-1", "open", "done");
    const rows = [t("A-1", "done")];
    expect(status(overlay(rows, p), "A-1")).toBe("done");
    expect(settle(p, rows).size).toBe(0);
  });
  it("keeps the entry while the row is unchanged", () => {
    const p = mv("A-1", "open", "done");
    expect(settle(p, [t("A-1", "open")])).toBe(p);
  });
  it("reverts when the move is removed (error)", () => {
    const p = mv("A-1", "open", "done");
    const rows = [t("A-1", "open")];
    expect(status(overlay(rows, withMove(p, "p/A-1", null)), "A-1")).toBe("open");
  });
  it("lets a newer remote change win", () => {
    const p = mv("A-1", "open", "done");
    const rows = [t("A-1", "review")];
    expect(status(overlay(rows, p), "A-1")).toBe("review");
    expect(settle(p, rows).size).toBe(0);
  });
  it("forgets deleted tickets", () => {
    expect(settle(mv("A-1", "open", "done"), []).size).toBe(0);
  });
  it("shows an overlaid ticket in the target column", () => {
    const cols = columns(config, overlay([t("A-1", "open")], mv("A-1", "open", "in-progress")));
    expect(cols.map(ids)).toEqual([[], ["A-1"], [], []]);
  });
});
