import { createSignal, For, Match, Show, Switch } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { back, href } from "../lib/router";
import { byRecent, createView } from "../lib/view";
import { useApp } from "./context";
import { EdgeSwipe } from "./EdgeSwipe";
import { NewSession } from "./NewSession";
import { SessionRowLink } from "./SessionRowLink";
import { SessionView } from "./SessionView";

export function SessionsView(props: { project: string; session?: string; creating: boolean }) {
  const { conn, projects } = useApp();
  const project = () => projects.get(props.project);
  const sessions = createView<SessionRow>(conn, "sessions", props.project, byRecent);

  // Archived sessions are hidden unless asked for; the choice lasts only while this view is open.
  const [showArchived, setShowArchived] = createSignal(false);
  const live = () => sessions.list().filter((s) => !s.archived);
  const archived = () => sessions.list().filter((s) => s.archived);
  const active = () => live().filter(isActive);
  const earlier = () => live().filter((s) => !isActive(s));
  const shownArchived = () => (showArchived() ? archived() : []);
  const selected = () => (props.session ? sessions.get(props.session) : undefined);
  const hasDetail = () => !!props.session || props.creating;
  let detail: HTMLElement | undefined;

  return (
    <div class="split" classList={{ "has-detail": hasDetail() }}>
      <section class="list">
        <header class="list-h">
          <h2>{project()?.name ?? ""}</h2>
          <a class="btn" href={href({ name: "sessions", project: props.project, creating: true })} title="New session">
            New
          </a>
        </header>
        <Show when={archived().length}>
          <label class="show-arch faint">
            <input type="checkbox" checked={showArchived()} onChange={(e) => setShowArchived(e.currentTarget.checked)} />{" "}
            Show archived
          </label>
        </Show>
        <div class="pane">
          <Show when={active().length}>
            <div class="grp cap">Active</div>
            <For each={active()}>{(s) => <SessionRowLink s={s} on={s.id === props.session} />}</For>
          </Show>
          <Show when={earlier().length}>
            <div class="grp cap">Earlier</div>
            <For each={earlier()}>{(s) => <SessionRowLink s={s} on={s.id === props.session} />}</For>
          </Show>
          <Show when={shownArchived().length}>
            <div class="grp cap">Archived</div>
            <For each={shownArchived()}>{(s) => <SessionRowLink s={s} on={s.id === props.session} />}</For>
          </Show>
        </div>
      </section>

      <section class="detail" ref={detail}>
        <Show when={hasDetail()}>
          <EdgeSwipe target={() => detail} onBack={back} />
        </Show>
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
          <Match when={sessions.ready() && !live().length && !shownArchived().length && archived().length}>
            <div class="empty">
              <p class="faint">Every session here is archived.</p>
            </div>
          </Match>
          <Match when={sessions.ready() && !sessions.list().length}>
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
