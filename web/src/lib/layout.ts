import { createSignal } from "solid-js";

// Desktop shell layout the user can adjust. Remembered across restarts.
const num = (key: string, fallback: number, lo: number, hi: number) => {
  try {
    const n = Number(localStorage.getItem(key));
    return n >= lo && n <= hi ? n : fallback;
  } catch {
    return fallback;
  }
};
const save = (key: string, v: string) => {
  try {
    localStorage.setItem(key, v);
  } catch {
    // storage unavailable: still works for this run
  }
};

const SIDE = "atelier.sidebar";
const LIST = "atelier.listw";

export const LIST_MIN = 220;
export const LIST_MAX = 560;

export const [sidebarOpen, setSidebar] = createSignal(
  (() => {
    try {
      return localStorage.getItem(SIDE) !== "closed";
    } catch {
      return true;
    }
  })(),
);
export const toggleSidebar = () => {
  setSidebar(!sidebarOpen());
  save(SIDE, sidebarOpen() ? "open" : "closed");
};

export const [listWidth, setList] = createSignal(num(LIST, 300, LIST_MIN, LIST_MAX));
export const setListWidth = (w: number) => setList(Math.round(Math.min(LIST_MAX, Math.max(LIST_MIN, w))));
export const saveListWidth = () => save(LIST, String(listWidth()));
