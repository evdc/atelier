import { For, Match, Show, Switch } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { href } from "../lib/router";
import { ago } from "../lib/time";
import { byRecent, createView } from "../lib/view";
import { useApp } from "./context";
import { NewSession } from "./NewSession";
import { SessionView } from "./SessionView";

export function SessionsView(props: { project: string; session?: string; creating: boolean }) {
  const { conn, projects } = useApp();
  const project = () => projects.get(props.project);
  const sessions = createView<SessionRow>(conn, "sessions", props.project, byRecent);

  const visible = () => sessions.list().filter((s) => !s.archived);
  const active = () => visible().filter(isActive);
  const earlier = () => visible().filter((s) => !isActive(s));
  const selected = () => (props.session ? sessions.get(props.session) : undefined);

  return (
    <div class="split" classList={{ "has-detail": !!props.session || props.creating }}>
      <section class="list">
        <header class="list-h">
          <h2>{project()?.name ?? ""}</h2>
          <a class="btn" href={href({ name: "sessions", project: props.project, creating: true })} title="New session">
            New
          </a>
        </header>
        <div class="pane">
          <Show when={active().length}>
            <div class="grp cap">Active</div>
            <For each={active()}>{(s) => <Row s={s} on={s.id === props.session} />}</For>
          </Show>
          <Show when={earlier().length}>
            <div class="grp cap">Earlier</div>
            <For each={earlier()}>{(s) => <Row s={s} on={s.id === props.session} />}</For>
          </Show>
        </div>
      </section>

      <section class="detail">
        <Switch>
          <Match when={props.creating && project()}>{(p) => <NewSession project={p()} />}</Match>
          <Match when={selected()}>
            {(s) => (
              // Keyed by id: a different session gets a fresh transcript subscription.
              <Show when={s().id} keyed>
                <SessionView row={s} />
              </Show>
            )}
          </Match>
          <Match when={sessions.ready() && !visible().length}>
            <div class="empty">
              <div class="illum" />
              <p>No sessions yet. Start one, and it gets its own worktree to work in.</p>
              <a class="btn dark" href={href({ name: "sessions", project: props.project, creating: true })}>
                New session
              </a>
            </div>
          </Match>
          <Match when={sessions.ready()}>
            <div class="empty">
              <p class="faint">Choose a session.</p>
            </div>
          </Match>
        </Switch>
      </section>
    </div>
  );
}

const isActive = (s: SessionRow) => s.status === "running" || s.status === "waiting";

function Row(props: { s: SessionRow; on: boolean }) {
  const s = () => props.s;
  return (
    <a class="row" classList={{ on: props.on }} href={href({ name: "sessions", project: s().project, session: s().id })}>
      <i class={`st ${s().status}`} />
      <span class="t">{s().title}</span>
      <span class="when">{ago(s().updated_at)}</span>
      <span class="m">
        <Show when={s().status === "waiting"}>
          <span class="needs">waiting on you</span>
        </Show>
        <span>{s().workspace.branch ?? "project dir"}</span>
      </span>
    </a>
  );
}
