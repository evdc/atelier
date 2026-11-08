import { createSignal, For, Show } from "solid-js";
import { go, href } from "../lib/router";
import { ago } from "../lib/time";
import { useApp } from "./context";

export function ProjectsView() {
  const { conn, projects, sessions } = useApp();
  const waiting = () => sessions.list().filter((s) => s.status === "waiting" && !s.archived);
  const name = (id: string) => projects.get(id)?.name ?? "";

  const [path, setPath] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!path().trim()) return;
    setBusy(true);
    setError(null);
    try {
      const res = (await conn.call("project.add", { path: path().trim() })) as { id: string };
      setPath("");
      go({ name: "sessions", project: res.id });
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="pane">
      <div class="page">
        <header class="page-h">
          <div class="grow">
            <h1>Projects</h1>
          </div>
        </header>

        <Show when={waiting().length}>
          {/* the phone has its own Waiting tab */}
          <section class="sec waiting-sec">
            <h2>
              Waiting on you <span>{waiting().length}</span>
            </h2>
            <div>
              <For each={waiting()}>
                {(s) => (
                  <a class="row" href={href({ name: "sessions", project: s.project, session: s.id })}>
                    <i class="st waiting" />
                    <span class="t">{s.title}</span>
                    <span class="when">{ago(s.updated_at)}</span>
                    <span class="m">{name(s.project)}</span>
                  </a>
                )}
              </For>
            </div>
          </section>
        </Show>

        <section class="sec">
          <h2>
            All projects <span>{projects.list().length || ""}</span>
          </h2>
          <Show
            when={projects.list().length || !projects.ready()}
            fallback={
              <div class="empty">
                <div class="illum" />
                <p>Add a repository on the server to begin. Sessions, and later tickets and pages, live inside a project.</p>
              </div>
            }
          >
            <div class="cards">
              <For each={projects.list()}>
                {(p) => (
                  <a class="card" href={href({ name: "sessions", project: p.id })}>
                    <h3>{p.name}</h3>
                    <span class="path" title={p.path}>
                      {p.path}
                    </span>
                    <div class="stats">
                      <span>{p.sessions === 1 ? "1 session" : `${p.sessions} sessions`}</span>
                      <Show when={p.running}>
                        <span>
                          <i class="st running" /> {p.running}
                        </span>
                      </Show>
                      <Show when={p.waiting}>
                        <span>
                          <i class="st waiting" /> {p.waiting}
                        </span>
                      </Show>
                      <span class="grow" />
                      <span class="faint">{ago(p.updated_at)}</span>
                    </div>
                  </a>
                )}
              </For>
            </div>
          </Show>
        </section>

        <section class="sec">
          <h2>Add a project</h2>
          <form class="add" onSubmit={add}>
            <input
              class="field"
              placeholder="~/Projects/my-repo — a directory on the server"
              value={path()}
              onInput={(e) => setPath(e.currentTarget.value)}
              spellcheck={false}
              autocapitalize="off"
            />
            <button class="btn dark" disabled={busy() || !path().trim()}>
              Add
            </button>
          </form>
          <Show when={error()}>
            <p class="err">{error()}</p>
          </Show>
        </section>
      </div>
    </div>
  );
}
