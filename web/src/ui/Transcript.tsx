import { createEffect, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type Accessor } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import type { Item } from "../agents/model";
import type { LiveTranscript } from "../agents/transcript";
import { usd } from "../lib/time";
import { summarize, toolStat } from "../agents/tools";
import { ToolBody } from "./tools/ToolBody";
import { Ask } from "./Ask";
import { Markdown } from "./Markdown";

export function TranscriptView(props: { live: LiveTranscript; session: Accessor<SessionRow> }) {
  const { t, streaming, ready } = props.live;
  let pane!: HTMLDivElement;
  // Follow new output only while the reader is at the bottom.
  let stick = true;
  const onScroll = () => {
    stick = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 48;
  };
  createEffect(
    on([() => t.items.length, streaming, ready], () => {
      if (stick) pane.scrollTop = pane.scrollHeight;
    }),
  );
  // The pane shrinks when the on-screen keyboard comes up; keep the end in view.
  onMount(() => {
    const ro = new ResizeObserver(() => {
      if (stick) pane.scrollTop = pane.scrollHeight;
    });
    ro.observe(pane);
    onCleanup(() => ro.disconnect());
  });

  return (
    <div class="pane" ref={pane} onScroll={onScroll}>
      <div class="tx">
        <For each={t.items}>{(item, i) => <ItemView item={item} prev={t.items[i() - 1]} session={props.session} />}</For>
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

function ItemView(props: { item: Item; prev: Item | undefined; session: Accessor<SessionRow> }) {
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
      <Match when={it.k === "permission" && it}>{(p) => <Ask item={p()} session={props.session} />}</Match>
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

function ToolView(props: { tool: ToolItem; cwd: string }) {
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
