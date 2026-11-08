// A tiny hash router. Routes are data; the app matches on them.
//
//   #/                         all projects
//   #/p/<project>              a project's sessions
//   #/p/<project>/new          start a session
//   #/p/<project>/s/<session>  one session

import { createSignal } from "solid-js";

export type Route =
  | { name: "projects" }
  | { name: "sessions"; project: string; session?: string; creating?: boolean };

export function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  if (parts[0] === "p" && parts[1]) {
    const project = parts[1];
    if (parts[2] === "s" && parts[3]) return { name: "sessions", project, session: parts[3] };
    if (parts[2] === "new") return { name: "sessions", project, creating: true };
    return { name: "sessions", project };
  }
  return { name: "projects" };
}

export function href(r: Route): string {
  if (r.name === "projects") return "#/";
  const p = `#/p/${encodeURIComponent(r.project)}`;
  if (r.session) return `${p}/s/${encodeURIComponent(r.session)}`;
  if (r.creating) return `${p}/new`;
  return p;
}

const [route, setRoute] = createSignal<Route>(parse(location.hash));
window.addEventListener("hashchange", () => setRoute(parse(location.hash)));

export { route };

export function go(r: Route) {
  location.hash = href(r);
}
