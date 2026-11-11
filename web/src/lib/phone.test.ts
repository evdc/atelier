import { describe, expect, it } from "vitest";
import type { SessionRow } from "../gen/SessionRow";
import { keyboardInset, rowSwipe, rowSwipeCommits, swipeIsBack, swipeMove, waitingList } from "./phone";
import { home, href, parse, tabOf } from "./routes";

const row = (id: string, status: SessionRow["status"], updated_at: number, archived = false): SessionRow => ({
  id,
  project: "p",
  stream: `p/p/s/${id}`,
  title: id,
  agent: "claude-code",
  workspace: { mode: "direct", path: "/x", branch: null },
  model: null,
  status,
  alive: true,
  pending: 0,
  turns: 1,
  cost_usd: 0,
  created_at: 0,
  updated_at,
  archived,
  imported: false,
  parent: null,
});

describe("waitingList", () => {
  it("keeps waiting then failed, newest first, skipping archived and others", () => {
    const { waiting, failed } = waitingList([
      row("w-old", "waiting", 1),
      row("run", "running", 9),
      row("f-old", "failed", 2),
      row("w-new", "waiting", 5),
      row("idle", "idle", 8),
      row("f-new", "failed", 7),
      row("w-arch", "waiting", 10, true),
    ]);
    expect(waiting.map((s) => s.id)).toEqual(["w-new", "w-old"]);
    expect(failed.map((s) => s.id)).toEqual(["f-new", "f-old"]);
  });
});

describe("swipe", () => {
  it("waits for a direction, follows rightward, abandons vertical or leftward", () => {
    expect(swipeMove(4, 3)).toBe("pending");
    expect(swipeMove(30, 8)).toBe("track");
    expect(swipeMove(12, 30)).toBe("cancel");
    expect(swipeMove(-15, 2)).toBe("cancel");
  });
  it("goes back only when far enough and mostly horizontal", () => {
    expect(swipeIsBack(120, 10, 390)).toBe(true);
    expect(swipeIsBack(60, 0, 390)).toBe(false);
    expect(swipeIsBack(120, 90, 390)).toBe(false);
  });
});

describe("keyboardInset", () => {
  it("sees a keyboard, not a pinch-zoom or a toolbar", () => {
    expect(keyboardInset(844, 844, 0, 1)).toBe(0);
    expect(keyboardInset(844, 508, 0, 1)).toBe(336);
    expect(keyboardInset(844, 400, 100, 2)).toBe(0);
    expect(keyboardInset(844, 790, 0, 1)).toBe(0);
  });
});

describe("routes", () => {
  it("round-trips the cross-project lists", () => {
    for (const r of [{ name: "waiting" }, { name: "all" }, { name: "projects" }] as const) expect(parse(href(r))).toEqual(r);
  });
  it("opens on Waiting only on a phone with no explicit hash", () => {
    expect(home("", true)).toEqual({ name: "waiting" });
    expect(home("#/", true)).toEqual({ name: "projects" });
    expect(home("", false)).toEqual({ name: "projects" });
    expect(home("#/p/x/s/y", true)).toEqual({ name: "sessions", project: "x", session: "y" });
  });
  it("keeps the tab a project screen was reached from", () => {
    expect(tabOf({ name: "sessions", project: "x", session: "y" }, "waiting")).toBe("waiting");
    expect(tabOf({ name: "all" }, "waiting")).toBe("all");
  });
});

describe("row swipe", () => {
  it("follows leftward horizontal drags only", () => {
    expect(rowSwipe(-3, 2)).toBe("pending");
    expect(rowSwipe(-40, 5)).toBe(-40);
    expect(rowSwipe(40, 5)).toBeNull();
    expect(rowSwipe(-10, 30)).toBeNull();
  });
  it("commits past a threshold", () => {
    expect(rowSwipeCommits(-100, 390)).toBe(true);
    expect(rowSwipeCommits(-50, 390)).toBe(false);
  });
});
