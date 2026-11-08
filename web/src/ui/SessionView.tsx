import { createSignal, Show, type Accessor } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { createTranscript } from "../agents/transcript";
import { go } from "../lib/router";
import { tokens, usd } from "../lib/time";
import { Composer } from "./Composer";
import { useApp } from "./context";
import { TranscriptView } from "./Transcript";

export function SessionView(props: { row: Accessor<SessionRow> }) {
  const { conn } = useApp();
  const s = props.row;
  // Captured once: the view is keyed by session id, so the stream is fixed.
  const live = createTranscript(conn, s().stream);
  const [error, setError] = createSignal<string | null>(null);

  const act = (p: Promise<unknown>) => p.catch((e: Error) => setError(e.message));
  const busy = () => s().status === "running" || s().status === "waiting";

  return (
    <>
      <header class="sh">
        <i class={`st ${s().status}`} title={s().status} />
        <h3 title={s().title}>{s().title}</h3>
        <span class={`tag ${s().agent}`}>claude</span>
        <Show when={busy()}>
          <button class="btn quiet" onClick={() => act(conn.call("session.interrupt", { session: s().id }))}>
            Interrupt
          </button>
        </Show>
        <Show when={s().alive && !busy()}>
          <button class="btn quiet" title="Stop the agent process; it resumes on your next message" onClick={() => act(conn.call("session.stop", { session: s().id }))}>
            Stop
          </button>
        </Show>
        <Show when={!s().archived}>
          <button
            class="btn quiet"
            onClick={() =>
              act(conn.call("session.archive", { session: s().id })).then(() => go({ name: "sessions", project: s().project }))
            }
          >
            Archive
          </button>
        </Show>
        <div class="meta">
          <Show when={s().workspace.branch} fallback={<span>project directory</span>}>
            <span>
              worktree <code>{s().workspace.branch}</code>
            </span>
          </Show>
          <span>{s().turns === 1 ? "1 turn" : `${s().turns} turns`}</span>
          <Show when={live.t.context}>{(n) => <span>{tokens(n())}</span>}</Show>
          <Show when={s().cost_usd > 0}>
            <span>{usd(s().cost_usd)}</span>
          </Show>
          <Show when={live.t.model ?? s().model}>{(m) => <span>{m()}</span>}</Show>
        </div>
      </header>

      <TranscriptView live={live} session={s} />

      <Show when={error()}>
        <p class="err composer-note">{error()}</p>
      </Show>
      <Show when={!s().archived}>
        <Composer
          waiting={s().status === "waiting"}
          onSend={(text) => conn.call("session.send", { session: s().id, text })}
        />
      </Show>
    </>
  );
}
