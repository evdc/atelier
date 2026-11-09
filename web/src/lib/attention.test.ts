import { describe, expect, test } from "vitest";
import type { SessionRow } from "../gen/SessionRow";
import type { SessionStatus } from "../gen/SessionStatus";
import { attention, COALESCE_MS, type AttentionInput, type AttentionMemo } from "./attention";

const row = (id: string, status: SessionStatus, archived = false): SessionRow =>
  ({ id, status, archived, title: id }) as unknown as SessionRow;

type Viewing = AttentionInput["viewing"];
const away: Viewing = { session: null, visible: false };
const onScreen = (id: string): Viewing => ({ session: id, visible: true });

function step(prev: AttentionMemo | null, rows: SessionRow[], now: number, viewing: Viewing = away) {
  return attention(prev, { rows, now, viewing });
}

describe("attention", () => {
  test("the first snapshot sets the baseline and notifies nothing", () => {
    const r = step(null, [row("a", "waiting"), row("b", "failed")], 0);
    expect(r.notify).toEqual([]);
    expect(r.waiting).toBe(1);
    expect(r.memo.status).toEqual({ a: "waiting", b: "failed" });
  });

  test("a transition into waiting or failed notifies once", () => {
    let m = step(null, [row("a", "running")], 0).memo;
    const r1 = step(m, [row("a", "waiting")], 1_000);
    expect(r1.notify.map((s) => s.id)).toEqual(["a"]);
    m = r1.memo;
    // Still waiting on the next update: not a transition.
    expect(step(m, [row("a", "waiting")], 2_000).notify).toEqual([]);
  });

  test("failed is notified too, and running -> idle is not", () => {
    const m = step(null, [row("a", "running"), row("b", "running")], 0).memo;
    const r = step(m, [row("a", "failed"), row("b", "idle")], 1_000);
    expect(r.notify.map((s) => s.id)).toEqual(["a"]);
  });

  test("a session that appears already waiting counts as a transition", () => {
    const m = step(null, [], 0).memo;
    expect(step(m, [row("new", "waiting")], 1_000).notify.map((s) => s.id)).toEqual(["new"]);
  });

  test("no notification for the session on screen while the document is visible", () => {
    const m = step(null, [row("a", "running")], 0).memo;
    expect(step(m, [row("a", "waiting")], 1_000, onScreen("a")).notify).toEqual([]);
    // Another session on screen does not hide this one.
    expect(step(m, [row("a", "waiting")], 1_000, onScreen("b")).notify.map((s) => s.id)).toEqual(["a"]);
  });

  test("a hidden document notifies even for the session on screen", () => {
    const m = step(null, [row("a", "running")], 0).memo;
    const hidden = { session: "a", visible: false };
    expect(step(m, [row("a", "waiting")], 1_000, hidden).notify.map((s) => s.id)).toEqual(["a"]);
  });

  test("coalesces: at most one notification per session per 30 s", () => {
    let m = step(null, [row("a", "running")], 0).memo;
    const first = step(m, [row("a", "waiting")], 1_000);
    expect(first.notify).toHaveLength(1);
    m = first.memo;
    // Back to running and waiting again inside the window: suppressed.
    m = step(m, [row("a", "running")], 2_000).memo;
    expect(step(m, [row("a", "waiting")], 1_000 + COALESCE_MS - 1).notify).toEqual([]);
    // Past the window: notified again.
    expect(step(m, [row("a", "waiting")], 1_000 + COALESCE_MS).notify.map((s) => s.id)).toEqual(["a"]);
  });

  test("coalescing is per session", () => {
    let m = step(null, [row("a", "running"), row("b", "running")], 0).memo;
    m = step(m, [row("a", "waiting"), row("b", "running")], 1_000).memo;
    const r = step(m, [row("a", "running"), row("b", "waiting")], 2_000);
    expect(r.notify.map((s) => s.id)).toEqual(["b"]);
  });

  test("archived sessions are ignored for transitions and the badge", () => {
    const m = step(null, [row("a", "running")], 0).memo;
    const r = step(m, [row("a", "waiting", true)], 1_000);
    expect(r.notify).toEqual([]);
    expect(r.waiting).toBe(0);
  });

  test("badge counts waiting sessions only", () => {
    const r = step(null, [row("a", "waiting"), row("b", "waiting"), row("c", "failed"), row("d", "running")], 0);
    expect(r.waiting).toBe(2);
  });

  test("memo drops sessions that disappeared", () => {
    let m = step(null, [row("a", "waiting")], 0).memo;
    m = step(m, [], 1_000).memo;
    expect(m.status).toEqual({});
  });
});
