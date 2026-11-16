import { createEffect, createMemo, createSignal, For, Index, onCleanup, Show } from "solid-js";
import type { BoardRow } from "../gen/BoardRow";
import type { TicketRow } from "../gen/TicketRow";
import { columns, overlay, settle, withMove, type Pending } from "../lib/board";
import { createDrag } from "../lib/drag";
import { serverConfig } from "../lib/conn";
import { canOpenIn, openIn } from "../lib/openin";
import { href, NARROW } from "../lib/router";
import { createView } from "../lib/view";
import { setTicketCount } from "../lib/tickets";
import { useApp } from "./context";
import { Column } from "./board/Column";
import { BoardEditor } from "./board/BoardEditor";
import { MoveMenu } from "./board/MoveMenu";

/** The project's ticket board (read-only). Watches `tickets` and `board` only while open. */
export function TicketsView(props: { project: string }) {
  const { conn, projects } = useApp();
  const project = () => projects.get(props.project);
  const tickets = createView<TicketRow>(conn, "tickets", props.project, () => 0, "key");
  const board = createView<BoardRow>(conn, "board", props.project, () => 0);
  const row = () => board.list()[0];
  const [pending, setPending] = createSignal<Pending>(new Map());
  const cols = createMemo(() => (row() ? columns(row()!.config, overlay(tickets.list(), pending())) : []));
  // Moves the rows have caught up with are done. Tracks `pending` and (inside settle) each row's status:
  // a status change patches the row in place, so the list alone never changes and the move would stay
  // pending forever, blocking any further move of that card.
  createEffect(() => {
    const p = pending();
    const next = settle(p, tickets.list());
    if (next !== p) setPending(next);
  });

  // Optimistic move: show it now, ask the server, and snap back with its message if it refuses.
  const [fail, setFail] = createSignal("");
  let failTimer = 0;
  onCleanup(() => clearTimeout(failTimer));
  const showFail = (e: unknown) => {
    setFail(e instanceof Error ? e.message : String(e));
    clearTimeout(failTimer);
    failTimer = window.setTimeout(() => setFail(""), 6000);
  };
  const move = (key: string, to: string) => {
    const t = tickets.get(key);
    if (!t || t.status === to || pending().has(key)) return;
    setPending((p) => withMove(p, key, { from: t.status, to }));
    conn
      .call("ticket.update", { project: props.project, ticket: t.id, status: to, expect: t.status })
      .catch((e: unknown) => {
        setPending((p) => withMove(p, key, null));
        showFail(e);
      });
  };

  const [menu, setMenu] = createSignal<{ key: string; card: HTMLElement; at: DOMRect } | null>(null);
  const openMenu = (key: string, card: HTMLElement) => setMenu({ key, card, at: card.getBoundingClientRect() });
  const closeMenu = () => {
    menu()?.card.focus({ preventScroll: true });
    setMenu(null);
  };
  const menuFrom = () => cols().find((c) => c.tickets.some((t) => t.key === menu()?.key))?.title ?? "";
  const onKeyDown = (e: KeyboardEvent) => {
    const el = e.target as HTMLElement;
    if (!el.classList.contains("bd-card") || (e.key !== "Enter" && e.key !== " ")) return;
    e.preventDefault();
    openMenu(el.dataset.key!, el);
  };

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

  const [editing, setEditing] = createSignal(false);

  let scroller: HTMLDivElement | undefined;
  const [at, setAt] = createSignal(0);
  const onScroll = () => scroller && setAt(Math.round(scroller.scrollLeft / Math.max(1, scroller.clientWidth)));
  const jump = (i: number) => scroller?.children[i]?.scrollIntoView({ behavior: "smooth", inline: "start", block: "nearest" });

  const onDown = createDrag({
    scroller: () => scroller,
    menuOnHold: narrow,
    onDrop: (key, status) => move(key, status),
    onMenu: openMenu,
  });

  const open = () => {
    const p = project();
    return p && canOpenIn(serverConfig()?.url ?? "")
      ? (t: TicketRow) => void openIn("editor", `${p.path}/${t.path}`).catch(showFail)
      : undefined;
  };

  return (
    <div class="bd">
      <header class="list-h">
        <a class="btn quiet back" href={href({ name: "sessions", project: props.project })} aria-label="Sessions">
          ‹
        </a>
        <h2>{project()?.name ?? ""} · Tickets</h2>
        <button class="btn quiet" disabled={!row()} onClick={() => setEditing(true)}>
          Edit columns
        </button>
      </header>
      <Show when={row()?.problems.length}>
        <ul class="bd-note">
          <For each={row()!.problems}>{(p) => <li>{p}</li>}</For>
        </ul>
      </Show>
      <Show when={fail()}>
        <p class="bd-fail err" role="alert">{fail()}</p>
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
        <div class="bd-cols" ref={scroller} onScroll={onScroll} onPointerDown={onDown} onKeyDown={onKeyDown}>
          {/* Index, not For: `columns()` builds fresh objects on every change, and Index keeps each
              column's DOM, so only the cards that moved re-render. */}
          <Index each={cols()}>
            {(c) => <Column col={c} collapsed={shut(c().title, c().collapsed)} onToggle={() => flip(c().title)} onOpen={open()} />}
          </Index>
        </div>
      </Show>
      <Show when={menu()}>
        {(m) => (
          <MoveMenu
            cols={cols()}
            current={menuFrom()}
            at={m().at}
            onPick={(status) => {
              const k = m().key;
              closeMenu();
              move(k, status);
            }}
            onClose={closeMenu}
          />
        )}
      </Show>
      <Show when={editing() && row()}>
        <BoardEditor
          project={props.project}
          config={row()!.config}
          tickets={tickets.list()}
          onClose={() => setEditing(false)}
          onSaved={() => setFlipped(new Set())}
        />
      </Show>
    </div>
  );
}
