import { createSignal, For, Show } from "solid-js";
import type { Item } from "../../agents/model";
import { diffLines, splitLines, tail, todos } from "../../agents/tools";
import { Code } from "./Code";

type ToolItem = Extract<Item, { k: "tool" }>;

const WRITE_COLLAPSED = 40;
const BASH_TAIL = 200;
const LIMIT = 6000;
const clip = (s: string) => (s.length > LIMIT ? `${s.slice(0, LIMIT)}\n… ${s.length - LIMIT} more characters` : s);

/** Only mounted while the tool's <details> is open (see ToolView), so nothing here costs while collapsed. */
export function ToolBody(props: { tool: ToolItem }) {
  const t = props.tool;
  const i = (t.input ?? {}) as Record<string, unknown>;
  switch (t.name) {
    case "Edit":
      return (
        <>
          <Code lines={diffLines(String(i.old_string ?? ""), String(i.new_string ?? ""))} />
          <Result tool={t} errorsOnly />
        </>
      );
    case "Write":
      return (
        <>
          <WriteBody content={String(i.content ?? "")} />
          <Result tool={t} errorsOnly />
        </>
      );
    case "Bash":
      return <BashBody tool={t} />;
    case "Read":
      return <Result tool={t} errorsOnly />;
    case "TodoWrite":
      return (
        <ul class="todos">
          <For each={todos(t.input)}>
            {(x) => (
              <li classList={{ done: x.status === "completed", doing: x.status === "in_progress" }}>
                <span>{x.status === "completed" ? "☑" : "☐"}</span> {x.content}
              </li>
            )}
          </For>
        </ul>
      );
    default:
      return (
        <>
          <pre>{clip(JSON.stringify(t.input, null, 2))}</pre>
          <Result tool={t} />
        </>
      );
  }
}

function Result(props: { tool: ToolItem; errorsOnly?: boolean }) {
  return (
    <Show when={props.tool.result && (!props.errorsOnly || props.tool.result.error) && props.tool.result}>
      {(r) => <pre>{clip(r().text) || "(no output)"}</pre>}
    </Show>
  );
}

function WriteBody(props: { content: string }) {
  const lines = splitLines(props.content);
  const [all, setAll] = createSignal(lines.length <= WRITE_COLLAPSED);
  return (
    <>
      <Show when={all()} fallback={<Code lines={lines.slice(0, WRITE_COLLAPSED).map((text) => ({ text }))} />}>
        <Code lines={lines.map((text) => ({ text }))} />
      </Show>
      <Show when={!all()}>
        <button class="more" onClick={() => setAll(true)}>
          show all {lines.length} lines
        </button>
      </Show>
    </>
  );
}

function BashBody(props: { tool: ToolItem }) {
  const [all, setAll] = createSignal(false);
  const out = () => {
    const r = props.tool.result;
    if (!r) return null;
    return all() ? { text: r.text, dropped: 0 } : tail(r.text, BASH_TAIL);
  };
  return (
    <Show when={out()} fallback={<pre>{String((props.tool.input as { command?: string })?.command ?? "")}</pre>}>
      {(o) => (
        <>
          <Show when={o().dropped > 0}>
            <button class="more" onClick={() => setAll(true)}>
              show all ({o().dropped} earlier lines hidden)
            </button>
          </Show>
          <pre class="out">{o().text || "(no output)"}</pre>
        </>
      )}
    </Show>
  );
}
