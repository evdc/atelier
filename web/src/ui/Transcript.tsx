import { createEffect, For, Match, on, Show, Switch, type Accessor } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import type { Item } from "../agents/model";
import type { LiveTranscript } from "../agents/transcript";
import { usd } from "../lib/time";
import { summarize } from "../agents/tools";
import { Ask } from "./Ask";

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
            <div class="body">{m().text}</div>
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
  return (
    <details class="tool" classList={{ err: !!tool().result?.error, "open-ish": !tool().result, sub: !!tool().sub }}>
      <summary>
        <b>{tool().name}</b>
        <span class="arg">{summarize(tool().name, tool().input, props.cwd)}</span>
      </summary>
      <pre>{clip(JSON.stringify(tool().input, null, 2))}</pre>
      <Show when={tool().result}>{(r) => <pre>{clip(r().text) || "(no output)"}</pre>}</Show>
    </details>
  );
}

const LIMIT = 6000;
const clip = (s: string) => (s.length > LIMIT ? `${s.slice(0, LIMIT)}\n… ${s.length - LIMIT} more characters` : s);
const seconds = (ms: number | null) => (ms == null ? "" : ms < 1000 ? `${ms}ms` : `${Math.round(ms / 1000)}s`);
