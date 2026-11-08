import { Match, Show, Switch } from "solid-js";
import type { ProjectRow } from "../gen/ProjectRow";
import type { SessionRow } from "../gen/SessionRow";
import type { Conn } from "../lib/conn";
import { route } from "../lib/router";
import { byRecent, createView } from "../lib/view";
import { Ctx } from "./context";
import { ProjectsView } from "./ProjectsView";
import { SessionsView } from "./SessionsView";
import { Sidebar } from "./Sidebar";

export function App(props: { conn: Conn }) {
  const projects = createView<ProjectRow>(props.conn, "projects", null, byRecent);
  const sessions = createView<SessionRow>(props.conn, "sessions", null, byRecent);

  const projectRoute = () => {
    const r = route();
    return r.name === "sessions" ? r : null;
  };

  return (
    <Ctx.Provider value={{ conn: props.conn, projects, sessions }}>
      <div class="app">
        <Sidebar />
        <main class="main">
          <Switch>
            <Match when={projectRoute()}>
              {(r) => (
                // Keyed by project: switching projects builds a fresh view.
                <Show when={r().project} keyed>
                  {(project) => <SessionsView project={project} session={r().session} creating={!!r().creating} />}
                </Show>
              )}
            </Match>
            <Match when={true}>
              <ProjectsView />
            </Match>
          </Switch>
        </main>
      </div>
    </Ctx.Provider>
  );
}
