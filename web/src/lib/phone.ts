// Pure decisions behind the phone layout: what is waiting, what a gesture
// means, whether the keyboard is up. The DOM wiring lives in the components.

import type { SessionRow } from "../gen/SessionRow";

/** Sessions that need you, newest first; failed ones (also newest first) below them. */
export function waitingList(all: readonly SessionRow[]): { waiting: SessionRow[]; failed: SessionRow[] } {
  const newest = (a: SessionRow, b: SessionRow) => b.updated_at - a.updated_at;
  const live = all.filter((s) => !s.archived);
  return {
    waiting: live.filter((s) => s.status === "waiting").sort(newest),
    failed: live.filter((s) => s.status === "failed").sort(newest),
  };
}

/** Edge swipe: how far in from the left a back gesture may start. */
export const EDGE_PX = 20;
/** Movement below this is a tap, not yet a direction. */
const SLOP_PX = 10;

/** Mid-gesture: undecided, following the finger, or abandoned (mostly vertical, or leftward). */
export function swipeMove(dx: number, dy: number): "pending" | "track" | "cancel" {
  if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return "pending";
  if (Math.abs(dy) > Math.abs(dx) || dx < 0) return "cancel";
  return "track";
}

/** On release: far enough to the right, and still mostly horizontal. */
export function swipeIsBack(dx: number, dy: number, width: number): boolean {
  return dx >= Math.min(80, width * 0.25) && Math.abs(dy) < dx * 0.6;
}

/** How much of the layout viewport the on-screen keyboard covers, or 0 when it is down.
 *  A pinch-zoom also shrinks the visual viewport; that is not a keyboard. */
export function keyboardInset(layoutH: number, vvH: number, vvTop: number, scale: number): number {
  if (scale > 1.01) return 0;
  const covered = layoutH - vvH - vvTop;
  return covered > 100 ? covered : 0;
}

/** Row swipe: a leftward drag that is mostly horizontal. Returns the distance to follow, or null to let the page scroll. */
export function rowSwipe(dx: number, dy: number): number | "pending" | null {
  if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return "pending";
  if (dx >= 0 || Math.abs(dy) > Math.abs(dx)) return null;
  return dx;
}

/** On release: dragged far enough left to mean "archive". */
export function rowSwipeCommits(dx: number, width: number): boolean {
  return -dx >= Math.min(96, width * 0.3);
}
