import { describe, expect, it } from "vitest";
import type { SessionRow } from "../gen/SessionRow";
import { nest } from "./tree";

const row = (id: string, parent: string | null = null, archived = false) => ({ id, parent, archived }) as SessionRow;
const shape = (rows: SessionRow[], expanded?: Set<string>) => nest(rows, expanded).map((n) => `${n.depth}${n.row.id}${n.hidden || ""}`);

describe("nest", () => {
  it("puts children under their parent and keeps sibling order", () => {
    const rows = [row("c2", "a"), row("b"), row("a"), row("g", "c2"), row("c1", "a")];
    expect(shape(rows)).toEqual(["0b", "0a", "1c2", "2g", "1c1"]);
  });

  it("treats a row whose parent is elsewhere as top level", () => {
    expect(shape([row("x", "elsewhere"), row("y")])).toEqual(["0x", "0y"]);
  });

  it("collapses children of an archived parent into a count, until expanded", () => {
    const rows = [row("a", null, true), row("b", "a"), row("c", "b")];
    expect(shape(rows)).toEqual(["0a2"]);
    expect(shape(rows, new Set(["a"]))).toEqual(["0a", "1b", "2c"]);
  });

  it("survives a cycle", () => {
    expect(nest([row("a", "b"), row("b", "a")]).length).toBe(0); // no root: nothing to show, but no hang
  });
});
