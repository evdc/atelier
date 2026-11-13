import { For, Show } from "solid-js";
import { href, route } from "../lib/router";
import { sidebarOpen, toggleSidebar } from "../lib/layout";
import { useApp } from "./context";

export function Sidebar() {
  const { projects } = useApp();
  const current = () => {
    const r = route();
    return r.name === "sessions" ? r.project : null;
  };
  const project = () => (current() ? projects.get(current()!) : undefined);

  return (
    <aside class="side">
      <div class="brand">
        <a class="brand-link" href={href({ name: "projects" })}>
          <span class="emblem" aria-hidden="true" />
          <b>Atelier</b>
        </a>
        <button
          class="btn quiet side-toggle"
          aria-label={sidebarOpen() ? "Collapse sidebar" : "Expand sidebar"}
          title={sidebarOpen() ? "Collapse sidebar" : "Expand sidebar"}
          onClick={toggleSidebar}
        >
          {sidebarOpen() ? "‹" : "›"}
        </button>
      </div>
      <nav>
        <a class="it" classList={{ on: route().name === "projects" }} href={href({ name: "projects" })}>
          <span>All projects</span>
        </a>

        <Show when={projects.list().length}>
          <div class="grp cap">Projects</div>
        </Show>
        <For each={projects.list()}>
          {(p) => (
            <a class="it" classList={{ here: current() === p.id }} href={href({ name: "sessions", project: p.id })}>
              <span>{p.name}</span>
              <Show when={p.waiting} fallback={<Show when={p.running}><i class="st running" /></Show>}>
                <span class="needs">{p.waiting}</span>
              </Show>
            </a>
          )}
        </For>

        <Show when={project()}>
          {(p) => (
            <>
              <div class="grp cap">Views</div>
              <a class="it on" href={href({ name: "sessions", project: p().id })}>
                <span>Sessions</span>
                <span class="n">{p().sessions}</span>
              </a>
            </>
          )}
        </Show>
      </nav>
    </aside>
  );
}
