import { describe, expect, it } from "vitest";
import type { BoardConfig } from "../gen/BoardConfig";
import type { TicketRow } from "../gen/TicketRow";
import { columns } from "./board";

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
