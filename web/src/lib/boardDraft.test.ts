import { describe, expect, it } from "vitest";
import type { BoardConfig } from "../gen/BoardConfig";
import type { TicketRow } from "../gen/TicketRow";
import * as d from "./boardDraft";

const cfg = (statuses: string[], columns: [string, string[]][]): BoardConfig => ({
  statuses,
  columns: columns.map(([title, s]) => ({ title, statuses: s, collapsed: false })),
});
const ticket = (id: string, status: string) => ({ id, status }) as TicketRow;

describe("problems (mirrors the server's board::effective cases)", () => {
  it("defaults and a valid config have none", () => {
    expect(d.problems(d.DEFAULTS)).toEqual([]);
    const c = cfg(["open", "in-progress", "review", "done"], [["To do", ["open"]], ["Doing", ["in-progress"]], ["Review", ["review"]], ["Done", ["done"]]]);
    expect(d.problems(c)).toEqual([]);
  });
  it("a column without statuses", () => {
    expect(d.problems(cfg(["open"], [["Empty", []], ["Open", ["open"]]]))).toEqual(["column `Empty` has no statuses; dropped"]);
  });
  it("a status missing from statuses", () => {
    expect(d.problems(cfg(["open"], [["A", ["open", "review"]]]))).toEqual([
      "status `review` is used by column `A` but not listed in statuses; added",
    ]);
  });
  it("a status in two columns", () => {
    expect(d.problems(cfg(["open", "done"], [["A", ["open", "done"]], ["B", ["done"]]]))).toEqual([
      "status `done` is in more than one column; kept in `A`",
      "column `B` has no statuses; dropped",
    ]);
  });
  it("a status listed twice", () => {
    expect(d.problems(cfg(["open", "open"], [["A", ["open"]]]))).toEqual(["status `open` is listed more than once in statuses"]);
    expect(d.problems(cfg(["open"], [["A", ["open", "open"]]]))).toEqual(["status `open` is listed more than once in column `A`"]);
  });
  it("empty or repeated titles (client only)", () => {
    expect(d.problems(cfg(["a", "b"], [["X", ["a"]], ["X", ["b"]]]))).toEqual(["column title `X` is used more than once"]);
    expect(d.problems(cfg(["a"], [[" ", ["a"]]]))).toEqual(["a column has no title"]);
  });
});

describe("draft operations", () => {
  const base = cfg(["open", "doing", "done"], [["Open", ["open"]], ["Doing", ["doing"]], ["Done", ["done"]]]);
  const titles = (c: BoardConfig) => c.columns.map((x) => x.title);

  it("never mutates its input", () => {
    const before = structuredClone(base);
    d.addColumn(base, "Review", "review");
    d.removeChip(base, 0, 0);
    d.renameStatus(base, "open", "todo");
    expect(base).toEqual(before);
  });
  it("adds a column with a new status, listing it", () => {
    const c = d.addColumn(base, "Review", "review");
    expect(titles(c)).toEqual(["Open", "Doing", "Done", "Review"]);
    expect(c.statuses).toEqual(["open", "doing", "done", "review"]);
    expect(d.problems(c)).toEqual([]);
  });
  it("adding a column for a status another column holds takes it over", () => {
    const c = d.addColumn(base, "Later", "open");
    expect(c.columns[0]!.statuses).toEqual([]);
    expect(c.columns[3]!.statuses).toEqual(["open"]);
  });
  it("removes and reorders columns; ends stay put", () => {
    expect(titles(d.removeColumn(base, 1))).toEqual(["Open", "Done"]);
    expect(titles(d.moveColumn(base, 2, -1))).toEqual(["Open", "Done", "Doing"]);
    expect(titles(d.moveColumn(base, 0, -1))).toEqual(["Open", "Doing", "Done"]);
    expect(titles(d.moveColumn(base, 2, 1))).toEqual(["Open", "Doing", "Done"]);
  });
  it("renames and collapses a column", () => {
    const c = d.toggleCollapsed(d.renameColumn(base, 0, "Backlog"), 0);
    expect(c.columns[0]).toMatchObject({ title: "Backlog", collapsed: true });
    expect(c.columns[1]!.collapsed).toBe(false);
  });
  it("adds a chip, moving it from another column and listing it if new", () => {
    const moved = d.addChip(base, 1, "open");
    expect(moved.columns[1]!.statuses).toEqual(["doing", "open"]);
    expect(moved.columns[0]!.statuses).toEqual([]);
    const fresh = d.addChip(base, 0, "blocked");
    expect(fresh.columns[0]!.statuses).toEqual(["open", "blocked"]);
    expect(fresh.statuses).toContain("blocked");
    expect(d.problems(fresh)).toEqual([]);
  });
  it("reorders chips, which changes the drop status", () => {
    const c = d.moveChip(d.addChip(base, 0, "blocked"), 0, 0, 1);
    expect(c.columns[0]!.statuses).toEqual(["blocked", "open"]);
  });
  it("removes a chip", () => {
    expect(d.removeChip(d.addChip(base, 0, "x"), 0, 1).columns[0]!.statuses).toEqual(["open"]);
  });
  it("renames a status everywhere in the config", () => {
    const c = d.renameStatus(base, "doing", "wip");
    expect(c.statuses).toEqual(["open", "wip", "done"]);
    expect(c.columns[1]!.statuses).toEqual(["wip"]);
    expect(d.problems(c)).toEqual([]);
  });
  it("removes a status from the list and its column", () => {
    const c = d.removeStatus(base, "doing");
    expect(c.statuses).toEqual(["open", "done"]);
    expect(c.columns[1]!.statuses).toEqual([]);
    expect(d.problems(c)).toEqual(["column `Doing` has no statuses; dropped"]);
  });
  it("addStatus ignores a known one", () => {
    expect(d.addStatus(base, "open")).toBe(base);
    expect(d.addStatus(base, "x").statuses).toEqual(["open", "doing", "done", "x"]);
  });
});

describe("orphaned", () => {
  const ts = [ticket("A-1", "review"), ticket("A-2", "review"), ticket("A-3", "open"), ticket("A-4", "")];
  it("counts tickets whose status the draft no longer lists", () => {
    expect(d.orphaned(d.DEFAULTS, ts)).toEqual([{ status: "review", count: 2 }]);
    expect(d.orphaned(d.addColumn(d.DEFAULTS, "Review", "review"), ts)).toEqual([]);
  });
  it("a renamed status orphans its tickets", () => {
    expect(d.orphaned(d.renameStatus(d.DEFAULTS, "open", "todo"), ts)).toEqual([
      { status: "review", count: 2 },
      { status: "open", count: 1 },
    ]);
  });
});

describe("clone", () => {
  it("copies a config held in a store (a Proxy), which structuredClone refuses", () => {
    const cfg: BoardConfig = { statuses: ["open", "done"], columns: [{ title: "To do", statuses: ["open"], collapsed: false }] };
    const proxied = new Proxy(cfg, {});
    expect(() => structuredClone(proxied)).toThrow();
    const c = d.clone(proxied);
    expect(c).toEqual(cfg);
    c.columns[0]!.statuses.push("done");
    expect(cfg.columns[0]!.statuses).toEqual(["open"]);
  });
});
