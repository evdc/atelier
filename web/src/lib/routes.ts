// Routes are data; the app matches on them. Pure, so it can be tested.
//
//   #/                         all projects
//   #/waiting                  sessions waiting on you, across projects (home on a phone)
//   #/sessions                 every session, across projects
//   #/p/<project>              a project's sessions
//   #/p/<project>/new          start a session
//   #/p/<project>/s/<session>  one session

export type Route =
  | { name: "projects" }
  | { name: "waiting" }
  | { name: "all" }
  | { name: "sessions"; project: string; session?: string; creating?: boolean };

/** The phone's bottom bar. */
export type Tab = "waiting" | "projects" | "all";

export function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  if (parts[0] === "p" && parts[1]) {
    const project = parts[1];
    if (parts[2] === "s" && parts[3]) return { name: "sessions", project, session: parts[3] };
    if (parts[2] === "new") return { name: "sessions", project, creating: true };
    return { name: "sessions", project };
  }
  if (parts[0] === "waiting") return { name: "waiting" };
  if (parts[0] === "sessions") return { name: "all" };
  return { name: "projects" };
}

export function href(r: Route): string {
  if (r.name === "projects") return "#/";
  if (r.name === "waiting") return "#/waiting";
  if (r.name === "all") return "#/sessions";
  const p = `#/p/${encodeURIComponent(r.project)}`;
  if (r.session) return `${p}/s/${encodeURIComponent(r.session)}`;
  if (r.creating) return `${p}/new`;
  return p;
}

/** Where a fresh launch lands: a phone opens on what needs you. An explicit
 *  hash (including `#/`, the Projects tab) is always kept. */
export function home(hash: string, narrow: boolean): Route {
  return narrow && hash.replace(/^#/, "") === "" ? { name: "waiting" } : parse(hash);
}

/** The bottom-bar tab a route belongs to; project screens keep the tab they were reached from. */
export function tabOf(r: Route, prev: Tab): Tab {
  if (r.name === "waiting") return "waiting";
  if (r.name === "all") return "all";
  if (r.name === "projects") return "projects";
  return prev;
}

export const NARROW = "(max-width: 760px)";
