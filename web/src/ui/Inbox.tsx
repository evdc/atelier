// Cross-project session lists: what is waiting on you (the phone's home), and everything.

import { createSignal, For, Show, type JSX } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { waitingList } from "../lib/phone";
import { href } from "../lib/router";
import { NewSession } from "./NewSession";
import { useApp } from "./context";
import { SessionRowLink } from "./SessionRowLink";

export function WaitingView() {
  const { sessions } = useApp();
  const lists = () => waitingList(sessions.list());
  return (
    <Page title="Waiting on you">
      <Show when={lists().waiting.length}>
        <Group label="Waiting" rows={lists().waiting} />
      </Show>
      <Show when={lists().failed.length}>
        <Group label="Failed" rows={lists().failed} />
      </Show>
      <Show when={sessions.ready() && !lists().waiting.length && !lists().failed.length}>
        <div class="empty">
          <div class="illum" />
          <p>Nothing needs you. Questions and permission requests from your agents arrive here.</p>
        </div>
      </Show>
    </Page>
  );
}

export function AllSessionsView() {
  const { sessions } = useApp();
  const visible = () => sessions.list().filter((s) => !s.archived);
  const isActive = (s: SessionRow) => s.status === "running" || s.status === "waiting";
  return (
    <Page
      title="Sessions"
      action={
        <a class="btn" href={href({ name: "new" })} title="New session">
          New
        </a>
      }
    >
      <Show when={visible().some(isActive)}>
        <Group label="Active" rows={visible().filter(isActive)} />
      </Show>
      <Show when={visible().some((s) => !isActive(s))}>
        <Group label="Earlier" rows={visible().filter((s) => !isActive(s))} />
      </Show>
      <Show when={sessions.ready() && !visible().length}>
        <div class="empty">
          <p class="faint">No sessions yet. Start one from a project.</p>
        </div>
      </Show>
    </Page>
  );
}

/** Start a session without first going to a project: pick the project here. */
export function NewAnywhereView() {
  const { projects, sessions } = useApp();
  const [chosen, setChosen] = createSignal<string>();
  // Default to the project touched most recently.
  const fallback = () => {
    const recent = sessions.list().reduce<SessionRow | undefined>((a, s) => (!a || s.updated_at > a.updated_at ? s : a), undefined)?.project;
    return projects.get(recent ?? "")?.id ?? projects.list()[0]?.id;
  };
  const project = () => projects.get(chosen() ?? fallback() ?? "");
  return (
    <Page title="New session">
      <div class="new">
        <label>
          Project
          <select class="field" value={project()?.id ?? ""} onChange={(e) => setChosen(e.currentTarget.value)}>
            <For each={projects.list()}>{(p) => <option value={p.id}>{p.name}</option>}</For>
          </select>
        </label>
      </div>
      <Show when={project()?.id} keyed>
        {(id) => <NewSession project={projects.get(id)!} />}
      </Show>
    </Page>
  );
}

function Page(props: { title: string; action?: JSX.Element; children: JSX.Element }) {
  return (
    <section class="list inbox">
      <header class="list-h">
        <h2>{props.title}</h2>
        {props.action}
      </header>
      <div class="pane">{props.children}</div>
    </section>
  );
}

function Group(props: { label: string; rows: SessionRow[] }) {
  const { projects } = useApp();
  return (
    <>
      <div class="grp cap">{props.label}</div>
      <For each={props.rows}>{(s) => <SessionRowLink s={s} projectName={projects.get(s.project)?.name} />}</For>
    </>
  );
}
