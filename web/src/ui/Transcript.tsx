import { createEffect, createMemo, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type Accessor } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import type { Item } from "../agents/model";
import type { LiveTranscript } from "../agents/transcript";
import { usd } from "../lib/time";
import { sendSummary, spawnedId, summarize, toolStat } from "../agents/tools";
import { href } from "../lib/router";
import { useApp } from "./context";
import { ToolBody } from "./tools/ToolBody";
import { Ask } from "./Ask";
import { Markdown } from "./Markdown";

// Windowed: only items near the viewport exist in the DOM. Heights are
// measured once rendered and cached by item id; unmeasured ones use a
// per-kind estimate. Spacers stand in for everything outside the window.
const GAP = 12; // matches .tx gap; folded into every cached height
const OVERSCAN = 600;
const EST: Record<Item["k"], number> = { user: 56, message: 90, text: 110, thinking: 24, tool: 26, permission: 70, turn: 20, note: 24 };

interface Win {
  s: number;
  e: number;
  top: number;
  bot: number;
}
const sameWin = (a: Win, b: Win) => a.s === b.s && a.e === b.e && a.top === b.top && a.bot === b.bot;

export function TranscriptView(props: { live: LiveTranscript; session: Accessor<SessionRow> }) {
  const { t, streaming, ready } = props.live;
  let pane!: HTMLDivElement;
  const heights = new Map<string, number>();
  // Indices are always < length; `undefined` only satisfies noUncheckedIndexedAccess.
  const h = (it: Item | undefined) => (it ? (heights.get(it.id) ?? EST[it.k] + GAP) : 0);
  // Only the newest pending permission takes the ⌘↵ / ⌘⌫ shortcuts. Read through the
  // permission index, so this doesn't touch every item of a long transcript.
  const newestAsk = createMemo(() => {
    let best = -1;
    for (const i of Object.values(t.perms)) {
      const it = t.items[i];
      if (i > best && it?.k === "permission" && it.state === "pending") best = i;
    }
    return best < 0 ? null : (t.items[best]?.id ?? null);
  });
  // Follow new output only while the reader is at the bottom.
  let stick = true;
  const [win, setWin] = createSignal<Win>({ s: 0, e: 0, top: 0, bot: 0 }, { equals: sameWin });
  const slice = createMemo(() => t.items.slice(win().s, win().e));

  const sync = () => {
    frame = 0;
    const items = t.items;
    const n = items.length;
    const vh = pane.clientHeight;
    let total = 0;
    for (let i = 0; i < n; i++) total += h(items[i]);
    // When following, the window is the end, whatever the (estimated) scrollTop says.
    const top = stick ? Math.max(0, total - vh) : pane.scrollTop;
    let acc = 0;
    let s = 0;
    while (s < n && acc + h(items[s]) < top - OVERSCAN) acc += h(items[s++]);
    const topPx = acc;
    let e = s;
    while (e < n && acc <= top + vh + OVERSCAN) acc += h(items[e++]);
    setWin({ s, e, top: topPx, bot: total - acc });
    if (stick) pane.scrollTop = pane.scrollHeight;
  };
  let frame = 0;
  const schedule = () => (frame ||= requestAnimationFrame(sync));

  // Any upward movement that leaves the bottom releases the follow at once: a
  // distance threshold alone re-pins slow drags (<48px) back to the end every frame.
  // Layout shrinking clamps scrollTop too, but that stays at distance ~0.
  let lastTop = 0;
  const onScroll = () => {
    const dist = pane.scrollHeight - pane.scrollTop - pane.clientHeight;
    if (pane.scrollTop < lastTop && dist > 2) stick = false;
    else if (dist <= 2) stick = true;
    lastTop = pane.scrollTop;
    schedule();
  };
  createEffect(on([() => t.items.length, ready], () => sync()));
  createEffect(
    on(streaming, () => {
      if (stick) pane.scrollTop = pane.scrollHeight;
    }),
  );

  // One observer for the pane (keyboard resize) and every rendered item.
  const ro = new ResizeObserver((entries) => {
    for (const en of entries) {
      const el = en.target as HTMLElement;
      const id = el.dataset.id;
      if (id === undefined) continue;
      const next = (en.borderBoxSize[0]?.blockSize ?? el.offsetHeight) + GAP;
      const prev = heights.get(id) ?? Number(el.dataset.est);
      heights.set(id, next);
      // An item above the viewport changed height: hold the reader's place.
      if (!stick && next !== prev && el.getBoundingClientRect().top < pane.getBoundingClientRect().top) {
        pane.scrollTop += next - prev;
      }
    }
    schedule();
  });
  onMount(() => ro.observe(pane));
  onCleanup(() => {
    ro.disconnect();
    cancelAnimationFrame(frame);
  });

  return (
    <div class="pane" ref={pane} onScroll={onScroll}>
      <div class="tx">
        <Show when={win().top > 0}>
          <div style={{ height: `${win().top - GAP}px` }} />
        </Show>
        <For each={slice()}>
          {(item, i) => (
            <div
              class="it"
              data-id={item.id}
              data-est={EST[item.k] + GAP}
              ref={(el) => {
                ro.observe(el);
                onCleanup(() => ro.unobserve(el));
              }}
            >
              <ItemView item={item} prev={t.items[win().s + i() - 1]} session={props.session} newestAsk={newestAsk} />
            </div>
          )}
        </For>
        <Show when={win().bot > 0}>
          <div style={{ height: `${win().bot - GAP}px` }} />
        </Show>
        <Show when={streaming()}>
          <div class="msg">
            <span class="who agent" />
            <div class="body caret">{streaming()}</div>
          </div>
        </Show>
      </div>
    </div>
  );
}

function ItemView(props: {
  item: Item;
  prev: Item | undefined;
  session: Accessor<SessionRow>;
  newestAsk: Accessor<string | null>;
}) {
  const it = props.item;
  return (
    <Switch>
      <Match when={it.k === "user" && it}>
        {(u) => (
          <div class="msg user">
            <span class="who">you</span>
            <div class="body">{u().text}</div>
          </div>
        )}
      </Match>
      <Match when={it.k === "message" && it}>{(m) => <AgentMessageCard item={m()} />}</Match>
      <Match when={it.k === "text" && it}>
        {(m) => (
          <div class="msg" classList={{ sub: !!m().sub, cont: props.prev?.k === "text" }}>
            <span class="who agent" />
            <Markdown text={m().text} />
          </div>
        )}
      </Match>
      <Match when={it.k === "thinking" && it}>
        {(m) => (
          <details class="thinking">
            <summary>thinking…</summary>
            <p>{m().text}</p>
          </details>
        )}
      </Match>
      <Match when={it.k === "tool" && it}>{(tool) => <ToolView tool={tool()} cwd={props.session().workspace.path} />}</Match>
      <Match when={it.k === "permission" && it}>
        {(p) => <Ask item={p()} session={props.session} newest={() => props.newestAsk() === p().id} />}
      </Match>
      <Match when={it.k === "turn" && it}>
        {(turn) => (
          <div class="turn" classList={{ err: turn().error }}>
            {[turn().error ? "stopped" : "", turn().cost != null ? usd(turn().cost!) : "", seconds(turn().ms)]
              .filter(Boolean)
              .join(" · ")}
          </div>
        )}
      </Match>
      <Match when={it.k === "note" && it}>{(n) => <p class={`note ${n().tone}`}>{n().text}</p>}</Match>
    </Switch>
  );
}

type ToolItem = Extract<Item, { k: "tool" }>;

/** A message from another session: gold rule, the sender (linked, title live), their words as markdown. */
function AgentMessageCard(props: { item: Extract<Item, { k: "message" }> }) {
  const { sessions } = useApp();
  const from = () => sessions.get(props.item.from);
  const to = (at?: number) =>
    from() ? href({ name: "sessions", project: from()!.project, session: props.item.from, at }) : undefined;
  return (
    <div class="msg agent-msg">
      <div class="from">
        from{" "}
        <a href={to()} classList={{ dead: !from() }}>
          "{from()?.title ?? props.item.from.slice(0, 8)}"
        </a>
      </div>
      <Markdown text={props.item.text} />
      <Show when={props.item.refs.length > 0}>
        <div class="refs">
          <For each={props.item.refs}>{(n) => <a href={to(n)}>#{n}</a>}</For>
        </div>
      </Show>
    </div>
  );
}

/** A `send` call in the sender's transcript: where it went and how it began. */
function SendLine(props: { to: string; line: string }) {
  const { sessions } = useApp();
  const target = () => (props.to.length >= 4 ? sessions.list().find((s) => s.id.startsWith(props.to)) : undefined);
  return (
    <div class="send-line">
      <span class="arrow">→</span>{" "}
      <Show when={target()} fallback={<span>"{props.to.slice(0, 8)}"</span>}>
        {(s) => <a href={href({ name: "sessions", project: s().project, session: s().id })}>"{s().title}"</a>}
      </Show>
      <span class="faint"> {props.line}</span>
    </div>
  );
}

/** A `spawn` call, as a link to the session it started, with that session's live status. */
function SpawnCard(props: { id: string; tool: ToolItem }) {
  const { sessions } = useApp();
  const child = () => sessions.get(props.id);
  return (
    <a class="spawn-card" href={href({ name: "sessions", project: child()?.project ?? "", session: props.id })}>
      <i class={`st ${child()?.status ?? "idle"}`} />
      <span class="t">{child()?.title ?? String((props.tool.input as { title?: string })?.title ?? "spawned session")}</span>
      <span class="faint">{child()?.status ?? ""}</span>
    </a>
  );
}

function ToolView(props: { tool: ToolItem; cwd: string }) {
  const tool = () => props.tool;
  const spawned = () => spawnedId(tool().name, tool().result);
  const sent = () => sendSummary(tool().name, tool().input);
  return (
    <Show when={!sent()} fallback={<SendLine to={sent()!.to} line={sent()!.line} />}>
      <Show when={spawned()} fallback={<PlainTool tool={tool()} cwd={props.cwd} />}>
        {(id) => <SpawnCard id={id()} tool={tool()} />}
      </Show>
    </Show>
  );
}

function PlainTool(props: { tool: ToolItem; cwd: string }) {
  const tool = () => props.tool;
  const [open, setOpen] = createSignal(false);
  return (
    <details
      class="tool"
      classList={{ err: !!tool().result?.error, "open-ish": !tool().result, sub: !!tool().sub }}
      onToggle={(e) => setOpen(e.currentTarget.open)}
    >
      <summary>
        <b>{tool().name}</b>
        <span class="arg">{summarize(tool().name, tool().input, props.cwd)}</span>
        <Show when={toolStat(tool().name, tool().input, tool().result)}>{(s) => <span class="stat">{s()}</span>}</Show>
      </summary>
      <Show when={open()}>
        <ToolBody tool={tool()} />
      </Show>
    </details>
  );
}

const seconds = (ms: number | null) => (ms == null ? "" : ms < 1000 ? `${ms}ms` : `${Math.round(ms / 1000)}s`);
