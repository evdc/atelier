// Swipe in from the left edge to go back (narrow layout only; the strip is
// hidden otherwise). A thin strip catches the touch so scrolling and
// horizontally scrolled code blocks elsewhere keep their gestures.

import { onCleanup } from "solid-js";
import { swipeIsBack, swipeMove } from "../lib/phone";

export function EdgeSwipe(props: { target: () => HTMLElement | undefined; onBack: () => void }) {
  let start: { id: number; x: number; y: number } | null = null;
  let dx = 0;
  let frame = 0;

  const paint = () => {
    frame = 0;
    const el = props.target();
    if (el) el.style.transform = dx > 0 ? `translateX(${dx}px)` : "";
  };
  const reset = () => {
    start = null;
    dx = 0;
    cancelAnimationFrame(frame);
    paint();
  };
  onCleanup(reset);

  return (
    <div
      class="edge"
      aria-hidden="true"
      onPointerDown={(e) => {
        start = { id: e.pointerId, x: e.clientX, y: e.clientY };
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        if (!start || e.pointerId !== start.id) return;
        const mx = e.clientX - start.x;
        const v = swipeMove(mx, e.clientY - start.y);
        if (v === "cancel") return reset();
        if (v === "track") {
          dx = mx;
          if (!frame) frame = requestAnimationFrame(paint);
        }
      }}
      onPointerUp={(e) => {
        if (!start || e.pointerId !== start.id) return;
        const back = swipeIsBack(e.clientX - start.x, e.clientY - start.y, window.innerWidth);
        reset();
        if (back) props.onBack();
      }}
      onPointerCancel={reset}
    />
  );
}
