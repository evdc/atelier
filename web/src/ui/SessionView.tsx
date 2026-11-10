import { createSignal, Show, type Accessor } from "solid-js";
import type { Archived } from "../gen/Archived";
import type { SessionRow } from "../gen/SessionRow";
import { createTranscript } from "../agents/transcript";
import { back, go } from "../lib/router";
import { tokens, usd } from "../lib/time";
import { panelMode, setPanelMode, togglePanel } from "../lib/panel";
import { ChangesList, createChanges, OpenIn, totals } from "./Changes";
import { Composer } from "./Composer";
import { useApp } from "./context";
import { TranscriptView } from "./Transcript";

export function SessionView(props: { row: Accessor<SessionRow> }) {
  const { conn } = useApp();
  const s = props.row;
  // Captured once: the view is keyed by session id, so the stream is fixed.
  const live = createTranscript(conn, s().stream);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  const [renaming, setRenaming] = createSignal(false);
  const [draft, setDraft] = createSignal("");

  const changes = createChanges(s);
  const sum = () => totals(changes());

  const act = (p: Promise<unknown>) => p.catch((e: Error) => setError(e.message));
  const busy = () => s().status === "running" || s().status === "waiting";

  const startRename = () => {
    setDraft(s().title);
    setRenaming(true);
  };
  // Enter saves, Escape and leaving the field cancel. The new title comes back as a row patch.
  const finishRename = (save: boolean) => {
    if (!renaming()) return;
    setRenaming(false);
    const title = draft().trim();
    if (save && title && title !== s().title) act(conn.call("session.rename", { session: s().id, title }));
  };

  return (
    <div class="sv" classList={{ full: panelMode() === "full" }}>
      <div class="sv-main">
      <header class="sh">
        <button class="btn quiet back" aria-label="Back" onClick={back}>
          ‹
        </button>
        <i class={`st ${s().status}`} title={s().status} />
        <Show
          when={renaming()}
          fallback={
            <h3 title={s().title}>
              <button type="button" class="title-btn" title="Rename" onClick={startRename}>
                {s().title}
              </button>
            </h3>
          }
        >
          <input
            class="title-edit"
            aria-label="Session title"
            value={draft()}
            ref={(el) =>
              queueMicrotask(() => {
                el.focus();
                el.select();
              })
            }
            onInput={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.isComposing) return;
              if (e.key === "Enter") {
                e.preventDefault();
                finishRename(true);
              } else if (e.key === "Escape") {
                e.preventDefault();
                finishRename(false);
              }
            }}
            onBlur={() => finishRename(false)}
          />
        </Show>
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
        <Show
          when={!s().archived}
          fallback={
            <button class="btn quiet" onClick={() => act(conn.call("session.unarchive", { session: s().id }))}>
              Unarchive
            </button>
          }
        >
          <button
            class="btn quiet"
            onClick={() =>
              act(conn.call("session.archive", { session: s().id })).then((r) => {
                const kept = (r as Archived | undefined)?.kept;
                // Stay put when the worktree was kept, so the reason is read before leaving.
                if (kept) setNote(`Archived. Worktree and branch kept: ${kept}.`);
                else go({ name: "sessions", project: s().project });
              })
            }
          >
            Archive
          </button>
        </Show>
        <OpenIn row={s} />
        <button
          class="btn quiet panel-toggle"
          aria-pressed={panelMode() !== "closed"}
          title={panelMode() !== "closed" ? "Hide side panel" : "Show side panel"}
          onClick={togglePanel}
        >
          <Show when={sum().files} fallback="Panel">
            {sum().files} {sum().files === 1 ? "file" : "files"} <span class="add-n">+{sum().added}</span>{" "}
            <span class="del-n">−{sum().removed}</span>
          </Show>
        </button>
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

      <Show when={note()}>
        <p class="composer-note">{note()}</p>
      </Show>
      <Show when={error()}>
        <p class="err composer-note">{error()}</p>
      </Show>
      <Show when={!s().archived}>
        <Composer
          waiting={s().status === "waiting"}
          onSend={(text) => conn.call("session.send", { session: s().id, text })}
        />
      </Show>
      </div>
      <Show when={panelMode() !== "closed"}>
        <aside class="sv-side">
          <div class="side-h">
            <span class="cap">{s().title}</span>
            <button
              class="btn quiet"
              title={panelMode() === "full" ? "Back to sidebar" : "Expand to full width"}
              onClick={() => setPanelMode(panelMode() === "full" ? "side" : "full")}
            >
              {panelMode() === "full" ? "Shrink" : "Expand"}
            </button>
            <button class="btn quiet" title="Close panel" onClick={() => setPanelMode("closed")}>
              Close
            </button>
          </div>
          <section>
            <h4 class="cap">Changes</h4>
            <ChangesList session={s().id} changes={changes()} />
          </section>
          <Show when={s().workspace.branch}>
            {(b) => (
              <section>
                <h4 class="cap">Workspace</h4>
                <dl class="kv">
                  <dt>branch</dt>
                  <dd>{b()}</dd>
                  <dt>path</dt>
                  <dd>{s().workspace.path}</dd>
                  <Show when={changes()?.base}>
                    {(base) => (
                      <>
                        <dt>base</dt>
                        <dd>{base().slice(0, 10)}</dd>
                      </>
                    )}
                  </Show>
                </dl>
              </section>
            )}
          </Show>
        </aside>
      </Show>
    </div>
  );
}
