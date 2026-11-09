import { createSignal } from "solid-js";

/** closed: chat only · side: chat + narrow panel · full: panel only, for reviewing. */
export type PanelMode = "closed" | "side" | "full";

// Remembered across sessions and restarts.
const KEY = "atelier.panel";
const read = (): PanelMode => {
  try {
    const v = localStorage.getItem(KEY);
    return v === "closed" || v === "full" ? v : "side";
  } catch {
    return "side";
  }
};
const [panelMode, set] = createSignal<PanelMode>(read());
export { panelMode };

export function setPanelMode(m: PanelMode) {
  set(m);
  try {
    localStorage.setItem(KEY, m);
  } catch {
    // storage unavailable: still works for this run
  }
}

/** The header button: show the panel as a sidebar, or hide it whatever it was. */
export const togglePanel = () => setPanelMode(panelMode() === "closed" ? "side" : "closed");
