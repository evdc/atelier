// Keep the app inside the visual viewport, so the on-screen keyboard pushes the
// composer up instead of covering it. The narrow layout sizes `.app` from
// --vv-h / --vv-top; `html.kb` hides the bottom bars while the keyboard is up.

import { keyboardInset } from "./phone";

export function trackViewport(): () => void {
  const vv = window.visualViewport;
  if (!vv) return () => {};
  const root = document.documentElement;
  let frame = 0;
  let last = "";

  const apply = () => {
    frame = 0;
    const zoomed = vv.scale > 1.01;
    const h = Math.round(vv.height);
    const top = Math.round(vv.offsetTop);
    const kb = keyboardInset(window.innerHeight, vv.height, vv.offsetTop, vv.scale) > 0;
    const key = `${zoomed}:${h}:${top}:${kb}`;
    if (key === last) return; // scroll events repeat; only write on change
    last = key;
    if (zoomed) {
      root.style.removeProperty("--vv-h");
      root.style.removeProperty("--vv-top");
    } else {
      root.style.setProperty("--vv-h", `${h}px`);
      root.style.setProperty("--vv-top", `${top}px`);
    }
    root.classList.toggle("kb", kb);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(apply);
  };

  vv.addEventListener("resize", schedule);
  vv.addEventListener("scroll", schedule);
  schedule();
  return () => {
    vv.removeEventListener("resize", schedule);
    vv.removeEventListener("scroll", schedule);
    cancelAnimationFrame(frame);
  };
}
