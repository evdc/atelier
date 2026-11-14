// Pointer-based card dragging: a pure gesture reducer, and a controller that wires it to the DOM.
// No library and no HTML5 drag-and-drop (its ghost image and touch support are poor in the iOS WebView).

export type Phase = "idle" | "armed" | "drag" | "menu";
export interface Gesture {
  phase: Phase;
  x: number;
  y: number;
  touch: boolean;
}
export type GestureEvent =
  | { t: "down"; x: number; y: number; touch: boolean }
  | { t: "move"; x: number; y: number }
  | { t: "hold" }
  | { t: "up" }
  | { t: "cancel" };

/** A mouse starts dragging after this much movement. */
export const MOUSE_SLOP = 4;
/** A finger that moves this far before the long-press fires is scrolling, not dragging. */
export const TOUCH_SLOP = 10;
export const HOLD_MS = 250;

export const IDLE: Gesture = { phase: "idle", x: 0, y: 0, touch: false };

/** Mouse: down, then 4 px → drag. Touch: down, then hold → drag (or the menu on a phone); moving first → scroll. */
export function step(s: Gesture, e: GestureEvent, menuOnHold: boolean): Gesture {
  switch (e.t) {
    case "down":
      return { phase: "armed", x: e.x, y: e.y, touch: e.touch };
    case "move": {
      if (s.phase !== "armed") return s;
      const far = Math.hypot(e.x - s.x, e.y - s.y) >= (s.touch ? TOUCH_SLOP : MOUSE_SLOP);
      if (!far) return s;
      return s.touch ? IDLE : { ...s, phase: "drag" };
    }
    case "hold":
      return s.phase === "armed" && s.touch ? { ...s, phase: menuOnHold ? "menu" : "drag" } : s;
    case "up":
    case "cancel":
      return IDLE;
  }
}

/** Which rect holds the point; the list is measured once at drag start. */
export function hit<T extends { left: number; right: number; top: number; bottom: number }>(
  rects: readonly T[],
  x: number,
  y: number,
): number {
  return rects.findIndex((r) => x >= r.left && x < r.right && y >= r.top && y < r.bottom);
}

/** Auto-scroll speed (px/frame) for a pointer at `x` in a scroller spanning left..right; 0 away from the edges. */
export function edgeSpeed(x: number, left: number, right: number, zone = 56, max = 16): number {
  if (x < left + zone) return -max * Math.min(1, (left + zone - x) / zone);
  if (x > right - zone) return max * Math.min(1, (x - (right - zone)) / zone);
  return 0;
}

export interface DragOptions {
  scroller: () => HTMLElement | undefined;
  /** True on a phone, where a long-press opens the move menu instead of dragging. */
  menuOnHold: () => boolean;
  /** Dropped on another column that accepts drops. `status` is that column's `data-status`. */
  onDrop: (key: string, status: string, fromCol: HTMLElement) => void;
  onMenu: (key: string, card: HTMLElement) => void;
}

/** Returns the `pointerdown` handler for the board; everything else attaches to the window for the gesture's life. */
export function createDrag(o: DragOptions) {
  return (down: PointerEvent) => {
    if (down.button !== 0 || !down.isPrimary) return;
    const target = down.target as HTMLElement;
    const card = target.closest<HTMLElement>(".bd-card");
    // Buttons inside a card (open in editor) stay buttons.
    if (!card || target.closest("button, a")) return;
    const key = card.dataset.key!;
    const scroller = o.scroller();
    if (!scroller) return;

    const touch = down.pointerType !== "mouse";
    let g = step(IDLE, { t: "down", x: down.clientX, y: down.clientY, touch }, o.menuOnHold());
    let px = down.clientX;
    let py = down.clientY;
    const timer = touch ? window.setTimeout(() => feed({ t: "hold" }), HOLD_MS) : 0;

    // Set up at drag start.
    let lift: HTMLElement | undefined;
    let cols: HTMLElement[] = [];
    let rects: DOMRect[] = []; // in scroller content coordinates, so they stay valid while it scrolls
    let ox = 0;
    let oy = 0;
    let lit = -1;
    let raf = 0;
    const home = card.closest<HTMLElement>(".bd-col")!;

    const block = (e: Event) => e.cancelable && e.preventDefault();
    const stop = () => {
      clearTimeout(timer);
      cancelAnimationFrame(raf);
      removeEventListener("pointermove", onMove);
      removeEventListener("pointerup", onUp);
      removeEventListener("pointercancel", onCancel);
      removeEventListener("keydown", onKey, true);
      removeEventListener("touchmove", block);
      lift?.remove();
      delete card.dataset.drag;
      cols[lit]?.classList.remove("over");
      scroller.classList.remove("dragging");
    };
    const finish = (drop: boolean) => {
      const was = g.phase;
      const to = lit >= 0 ? cols[lit]! : undefined;
      stop();
      g = IDLE;
      if (was === "drag") {
        // The click that follows a drag's pointerup would hit whatever is under the pointer.
        const eat = (e: Event) => e.stopPropagation();
        addEventListener("click", eat, { capture: true, once: true });
        setTimeout(() => removeEventListener("click", eat, true), 0);
        if (drop && to && to !== home && to.dataset.status) o.onDrop(key, to.dataset.status, home);
      }
    };

    const beginDrag = () => {
      const r = card.getBoundingClientRect();
      const s = scroller.getBoundingClientRect();
      ox = down.clientX - r.left;
      oy = down.clientY - r.top;
      cols = [...scroller.querySelectorAll<HTMLElement>(".bd-col")];
      rects = cols.map((c) => {
        const b = c.getBoundingClientRect();
        return new DOMRect(b.left - s.left + scroller.scrollLeft, b.top, b.width, b.height);
      });
      lift = card.cloneNode(true) as HTMLElement;
      lift.removeAttribute("data-key");
      lift.removeAttribute("tabindex");
      lift.className = "bd-card lifted";
      lift.style.cssText = `width:${r.width}px;left:0;top:0;transform:translate(${r.left}px,${r.top}px)`;
      document.body.append(lift);
      card.dataset.drag = "";
      scroller.classList.add("dragging");
      try {
        card.setPointerCapture(down.pointerId);
      } catch {}
      // A touch that has started panning can't be stopped by pointer events; its touchmove can.
      addEventListener("touchmove", block, { passive: false });
      frame();
    };

    // Once per animation frame: place the lifted copy, hit-test against the measured rects, auto-scroll.
    const frame = () => {
      raf = 0;
      if (g.phase !== "drag" || !lift) return;
      lift.style.transform = `translate(${px - ox}px,${py - oy}px)`;
      const s = scroller.getBoundingClientRect();
      scroller.scrollLeft += edgeSpeed(px, s.left, s.right);
      const i = hit(rects, px - s.left + scroller.scrollLeft, py);
      if (i !== lit) {
        cols[lit]?.classList.remove("over");
        lit = i;
        // Own column and "Other" (no status) take no drops, so they don't light up.
        if (i >= 0 && cols[i] !== home && cols[i]!.dataset.status) cols[i]!.classList.add("over");
      }
      raf = requestAnimationFrame(frame);
    };

    function feed(e: GestureEvent) {
      const before = g.phase;
      g = step(g, e, o.menuOnHold());
      if (g.phase === before) return;
      if (g.phase === "drag") beginDrag();
      else if (g.phase === "menu") {
        stop();
        g = IDLE;
        o.onMenu(key, card!);
      } else if (g.phase === "idle") stop();
    }
    function onMove(e: PointerEvent) {
      if (!e.isPrimary) return;
      px = e.clientX;
      py = e.clientY;
      feed({ t: "move", x: px, y: py });
    }
    function onUp() {
      finish(true);
    }
    function onCancel() {
      finish(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      finish(false);
    }

    addEventListener("pointermove", onMove);
    addEventListener("pointerup", onUp);
    addEventListener("pointercancel", onCancel);
    addEventListener("keydown", onKey, true);
    // A long-press on touch would otherwise raise the system context menu / text selection.
    if (touch) card.addEventListener("contextmenu", block, { once: true });
  };
}
