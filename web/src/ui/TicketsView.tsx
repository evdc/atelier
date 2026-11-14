import { createEffect, createMemo, createSignal, For, Index, onCleanup, Show } from "solid-js";
import type { BoardRow } from "../gen/BoardRow";
import type { TicketRow } from "../gen/TicketRow";
import { columns } from "../lib/board";
import { serverConfig } from "../lib/conn";
import { canOpenIn, openIn } from "../lib/openin";
import { href, NARROW } from "../lib/router";
import { createView } from "../lib/view";
import { setTicketCount } from "../lib/tickets";
import { useApp } from "./context";
import { Column } from "./board/Column";

/** The project's ticket board (read-only). Watches `tickets` and `board` only while open. */
export function TicketsView(props: { project: string }) {
  const { conn, projects } = useApp();
  const project = () => projects.get(props.project);
  const tickets = createView<TicketRow>(conn, "tickets", props.project, () => 0, "key");
  const board = createView<BoardRow>(conn, "board", props.project, () => 0);
  const row = () => board.list()[0];
  const cols = createMemo(() => (row() ? columns(row()!.config, tickets.list()) : []));
  const ready = () => tickets.ready() && board.ready();

  // The sidebar shows the count; it has no watch of its own.
  createEffect(() => setTicketCount(tickets.ready() ? tickets.list().length : null));
  onCleanup(() => setTicketCount(null));

  // Columns the user flipped from their configured default; lasts while the view is open.
  const [flipped, setFlipped] = createSignal<ReadonlySet<string>>(new Set());
  const flip = (title: string) =>
    setFlipped((s) => {
      const n = new Set(s);
      if (!n.delete(title)) n.add(title);
      return n;
    });

  // On a phone one column fills the screen, so "collapsed" means nothing there.
  const mq = matchMedia(NARROW);
  const [narrow, setNarrow] = createSignal(mq.matches);
  const onMq = () => setNarrow(mq.matches);
  mq.addEventListener("change", onMq);
  onCleanup(() => mq.removeEventListener("change", onMq));
  const shut = (title: string, dflt: boolean) => !narrow() && dflt !== flipped().has(title);

  let scroller: HTMLDivElement | undefined;
  const [at, setAt] = createSignal(0);
  const onScroll = () => scroller && setAt(Math.round(scroller.scrollLeft / Math.max(1, scroller.clientWidth)));
  const jump = (i: number) => scroller?.children[i]?.scrollIntoView({ behavior: "smooth", inline: "start", block: "nearest" });

  const open = () => {
    const p = project();
    return p && canOpenIn(serverConfig()?.url ?? "")
      ? (t: TicketRow) => void openIn("editor", `${p.path}/${t.path}`).catch(() => {})
      : undefined;
  };

  return (
    <div class="bd">
      <header class="list-h">
        <a class="btn quiet back" href={href({ name: "sessions", project: props.project })} aria-label="Sessions">
          ‹
        </a>
        <h2>{project()?.name ?? ""} · Tickets</h2>
      </header>
      <Show when={row()?.problems.length}>
        <ul class="bd-note">
          <For each={row()!.problems}>{(p) => <li>{p}</li>}</For>
        </ul>
      </Show>
      <Show
        when={!ready() || tickets.list().length}
        fallback={
          <div class="empty">
            <p class="faint">No tickets yet. They live in this project's <code>.atelier/tickets/*.md</code>.</p>
          </div>
        }
      >
        <Show when={narrow() && cols().length}>
          <nav class="bd-seg">
            <For each={cols()}>
              {(c, i) => (
                <button classList={{ on: at() === i() }} onClick={() => jump(i())}>
                  {c.title} <span>{c.tickets.length}</span>
                </button>
              )}
            </For>
          </nav>
        </Show>
        <div class="bd-cols" ref={scroller} onScroll={onScroll}>
          {/* Index, not For: `columns()` builds fresh objects on every change, and Index keeps each
              column's DOM, so only the cards that moved re-render. */}
          <Index each={cols()}>
            {(c) => <Column col={c} collapsed={shut(c().title, c().collapsed)} onToggle={() => flip(c().title)} onOpen={open()} />}
          </Index>
        </div>
      </Show>
    </div>
  );
}
