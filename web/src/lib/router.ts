// A tiny hash router: the current route as a signal. Routes themselves live in routes.ts.

import { createSignal } from "solid-js";
import { home, href, NARROW, parse, tabOf, type Route, type Tab } from "./routes";

export { href, NARROW, type Route, type Tab };

const initial = home(location.hash, matchMedia(NARROW).matches);
if (initial.name === "waiting" && !location.hash) history.replaceState(null, "", href(initial));

// Each history entry records its depth within this app, so "back" never leaves it.
const depth = () => (history.state as { d?: number } | null)?.d ?? 0;

const [route, setRoute] = createSignal<Route>(initial);
const [tab, setTab] = createSignal<Tab>(tabOf(initial, "projects"));
let at = depth();
window.addEventListener("hashchange", () => {
  // A fresh entry has no state yet: it is one deeper than where we were.
  if (history.state?.d == null) history.replaceState({ d: at + 1 }, "");
  at = depth();
  const r = parse(location.hash);
  setRoute(r);
  setTab((t) => tabOf(r, t));
});

export { route, tab };

export function go(r: Route) {
  location.hash = href(r);
}

/** Back from a detail screen: the previous page if we came from one, else the list it belongs to. */
export function back() {
  if (depth() > 0) return history.back();
  const r = route();
  go(r.name === "sessions" && (r.session || r.creating) ? { name: "sessions", project: r.project } : { name: tab() });
}
