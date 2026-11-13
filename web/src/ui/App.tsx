import { createEffect, Match, onCleanup, Show, Switch, untrack } from "solid-js";
import type { ProjectRow } from "../gen/ProjectRow";
import type { SessionRow } from "../gen/SessionRow";
import type { UsageRow } from "../gen/UsageRow";
import { attention, type AttentionMemo } from "../lib/attention";
import type { Conn } from "../lib/conn";
import { notifyNeedsYou, setDockBadge } from "../lib/notify";
import { route } from "../lib/router";
import { byRecent, createView } from "../lib/view";
import { trackViewport } from "../lib/viewport";
import { sidebarOpen } from "../lib/layout";
import { Ctx } from "./context";
import { AllSessionsView, NewAnywhereView, WaitingView } from "./Inbox";
import { ProjectsView } from "./ProjectsView";
import { SessionsView } from "./SessionsView";
import { Footer } from "./Footer";
import { Sidebar } from "./Sidebar";
import { Tabs } from "./Tabs";

export function App(props: { conn: Conn }) {
  const projects = createView<ProjectRow>(props.conn, "projects", null, byRecent);
  const sessions = createView<SessionRow>(props.conn, "sessions", null, byRecent);
  const usage = createView<UsageRow>(props.conn, "usage", null, () => 0);
  onCleanup(trackViewport());

  // Notify once when a session starts needing you, and while it is not on screen.
  // The first snapshot only sets the baseline (see lib/attention.ts).
  let memo: AttentionMemo | null = null;
  createEffect(() => {
    if (!sessions.ready()) return;
    const rows = sessions.list();
    const r = untrack(() => route());
    const result = attention(memo, {
      rows,
      now: Date.now(),
      viewing: {
        session: r.name === "sessions" ? (r.session ?? null) : null,
        visible: document.visibilityState === "visible",
      },
    });
    memo = result.memo;
    for (const s of result.notify) void notifyNeedsYou(s);
    setDockBadge(result.waiting);
  });

  const projectRoute = () => {
    const r = route();
    return r.name === "sessions" ? r : null;
  };

  return (
    <Ctx.Provider value={{ conn: props.conn, projects, sessions, usage }}>
      <div class="app" classList={{ "side-off": !sidebarOpen() }}>
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
            <Match when={route().name === "waiting"}>
              <WaitingView />
            </Match>
            <Match when={route().name === "new"}>
              <NewAnywhereView />
            </Match>
            <Match when={route().name === "all"}>
              <AllSessionsView />
            </Match>
            <Match when={true}>
              <ProjectsView />
            </Match>
          </Switch>
        </main>
        <Footer />
        <Tabs />
      </div>
    </Ctx.Provider>
  );
}
