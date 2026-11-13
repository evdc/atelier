import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import type { Imported } from "../gen/Imported";
import type { SessionRow } from "../gen/SessionRow";
import { back, href } from "../lib/router";
import { listWidth, saveListWidth, setListWidth } from "../lib/layout";
import { nest, splitTrees, type Node } from "../lib/tree";
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
  // Parents the user folded (live) or unfolded (archived), against each default.
  const [toggled, setToggled] = createSignal<ReadonlySet<string>>(new Set());
  const flip = (id: string) =>
    setToggled((t) => {
      const next = new Set(t);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const groups = createMemo(() => splitTrees(nest(live(), toggled()), isActive));
  const active = () => groups()[0];
  const earlier = () => groups()[1];
  const shownArchived = () => (showArchived() ? archived() : []);
  const selected = () => (props.session ? sessions.get(props.session) : undefined);
  const hasDetail = () => !!props.session || props.creating;
  let detail: HTMLElement | undefined;

  // Importing reads Claude Code's history from disk, so it only happens when asked.
  const [importing, setImporting] = createSignal(false);
  const [imported, setImported] = createSignal("");
  const importHistory = async () => {
    setImporting(true);
    setImported("");
    try {
      const r = (await conn.call("project.import", { project: props.project })) as Imported;
      setImported(r.messages ? `Imported ${r.sessions} new, ${r.messages} messages` : "Nothing new to import");
    } catch (e) {
      setImported(e instanceof Error ? e.message : String(e));
    } finally {
      setImporting(false);
    }
  };

  const row = (n: Node) => (
    <SessionRowLink
      s={n.row}
      on={n.row.id === props.session}
      depth={n.depth}
      hidden={n.hidden}
      fold={{ kids: n.kids, open: !n.hidden, toggle: () => flip(n.row.id) }}
    />
  );

  return (
    <div class="split" classList={{ "has-detail": hasDetail() }} style={{ "--list-w": `${listWidth()}px` }}>
      <section class="list">
        <header class="list-h">
          <h2>{project()?.name ?? ""}</h2>
          <button
            class="btn quiet"
            disabled={importing()}
            onClick={importHistory}
            title="Bring in this project's existing Claude Code sessions"
          >
            {importing() ? "Importing…" : "Import"}
          </button>
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
          <Show when={imported()}>
            <div class="grp faint">{imported()}</div>
          </Show>
          <Show when={active().length}>
            <div class="grp cap">Active</div>
            <For each={active()}>
              {(n) => row(n)}
            </For>
          </Show>
          <Show when={earlier().length}>
            <div class="grp cap">Earlier</div>
            <For each={earlier()}>
              {(n) => row(n)}
            </For>
          </Show>
          <Show when={shownArchived().length}>
            <div class="grp cap">Archived</div>
            <For each={nest(shownArchived(), toggled())}>
              {(n) => row(n)}
            </For>
          </Show>
        </div>
        <div
          class="resize"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize session list"
          tabindex="0"
          onPointerDown={(e) => {
            const el = e.currentTarget;
            const x0 = e.clientX;
            const w0 = listWidth();
            el.setPointerCapture(e.pointerId);
            el.classList.add("drag");
            const move = (m: PointerEvent) => setListWidth(w0 + m.clientX - x0);
            const up = () => {
              el.classList.remove("drag");
              el.removeEventListener("pointermove", move);
              el.removeEventListener("pointerup", up);
              el.removeEventListener("pointercancel", up);
              saveListWidth();
            };
            el.addEventListener("pointermove", move);
            el.addEventListener("pointerup", up);
            el.addEventListener("pointercancel", up);
          }}
          onKeyDown={(e) => {
            if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
            e.preventDefault();
            setListWidth(listWidth() + (e.key === "ArrowRight" ? 16 : -16));
            saveListWidth();
          }}
        />
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
