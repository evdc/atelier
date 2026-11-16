import { describe, expect, it } from "vitest";
import { edgeSpeed, hit, IDLE, step, type Gesture } from "./drag";

const down = (touch: boolean): Gesture => step(IDLE, { t: "down", x: 100, y: 100, touch }, false);

describe("gesture", () => {
  it("a mouse drags after 4px, not before", () => {
    expect(step(down(false), { t: "move", x: 103, y: 100 }, false).phase).toBe("armed");
    expect(step(down(false), { t: "move", x: 100, y: 104 }, false).phase).toBe("drag");
  });
  it("a click without movement ends idle", () => {
    expect(step(down(false), { t: "up" }, false).phase).toBe("idle");
  });
  it("touch drags only after the hold", () => {
    expect(step(down(true), { t: "move", x: 102, y: 101 }, false).phase).toBe("armed");
    expect(step(down(true), { t: "hold" }, false).phase).toBe("drag");
  });
  it("touch that moves before the hold is a scroll", () => {
    expect(step(down(true), { t: "move", x: 100, y: 130 }, false).phase).toBe("idle");
  });
  it("a phone hold opens the menu instead", () => {
    expect(step(down(true), { t: "hold" }, true).phase).toBe("menu");
  });
  it("a mouse ignores the hold; moving while dragging stays dragging", () => {
    expect(step(down(false), { t: "hold" }, false).phase).toBe("armed");
    const d = step(down(false), { t: "move", x: 120, y: 100 }, false);
    expect(step(d, { t: "move", x: 300, y: 300 }, false).phase).toBe("drag");
  });
  it("cancel and up end it", () => {
    const d = step(down(false), { t: "move", x: 120, y: 100 }, false);
    expect(step(d, { t: "cancel" }, false).phase).toBe("idle");
    expect(step(d, { t: "up" }, false).phase).toBe("idle");
  });
});

describe("hit and edge scroll", () => {
  const rects = [
    { left: 0, right: 100, top: 0, bottom: 50 },
    { left: 110, right: 200, top: 0, bottom: 50 },
  ];
  it("finds the rect under the point, or -1 in a gap or outside", () => {
    expect(hit(rects, 150, 10)).toBe(1);
    expect(hit(rects, 105, 10)).toBe(-1);
    expect(hit(rects, 50, 60)).toBe(-1);
  });
  it("scrolls near the edges, faster closer in", () => {
    expect(edgeSpeed(500, 0, 1000)).toBe(0);
    expect(edgeSpeed(10, 0, 1000)).toBeLessThan(0);
    expect(edgeSpeed(995, 0, 1000)).toBeGreaterThan(edgeSpeed(960, 0, 1000));
  });
});
