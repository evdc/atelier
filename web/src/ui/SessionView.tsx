import { createEffect, createSignal, onCleanup, Show, type Accessor } from "solid-js";
import type { Archived } from "../gen/Archived";
import type { SessionRow } from "../gen/SessionRow";
import { createTranscript } from "../agents/transcript";
import { back, go, href } from "../lib/router";
import { prefillNewSession, readersOf, reviewPrompt, reviewTitle, shortCommit } from "../lib/review";
import { tokens, usd } from "../lib/time";
import { panelMode, setPanelMode, togglePanel } from "../lib/panel";
import { ChangesList, createChanges, MergeButton, OpenInItems, totals } from "./Changes";
import { Composer } from "./Composer";
import { useApp } from "./context";
import { TranscriptView } from "./Transcript";

export function SessionView(props: { row: Accessor<SessionRow> }) {
  const { conn, sessions } = useApp();
  const s = props.row;
  // Captured once: the view is keyed by session id, so the stream is fixed.
  const live = createTranscript(conn, s().stream);
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  const [renaming, setRenaming] = createSignal(false);
  const [draft, setDraft] = createSignal("");

  const [changes, refreshChanges] = createChanges(s);
  const sum = () => totals(changes());
  // The owner of a shared or branched workspace, when it's still in the list.
  const source = () => (s().workspace.from ? sessions.get(s().workspace.from as string) : undefined);
  const sourceName = () => source()?.title ?? "another session";
  const readers = () => readersOf(sessions.list(), s().id);
  const spawner = () => (s().parent ? sessions.get(s().parent as string) : undefined);
  const liveChildren = () => sessions.list().filter((c) => c.parent === s().id && !c.archived);
  const reviewChanges = () => {
    const ws = s().workspace;
    prefillNewSession({
      source: "share",
      from: s().id,
      title: reviewTitle(s().title),
      prompt: reviewPrompt({ title: s().title, base: ws.base, branch: ws.branch }),
    });
    go({ name: "sessions", project: s().project, creating: true });
  };

  const act = (p: Promise<unknown>) => p.catch((e: Error) => setError(e.message));
  const busy = () => s().status === "running" || s().status === "waiting";

  const archive = () =>
    act(conn.call("session.archive", { session: s().id })).then((r) => {
      const kept = (r as Archived | undefined)?.kept;
      const kids = liveChildren().length;
      // Stay put when there is something to read first: why the worktree was kept, or the live children.
      if (kept || kids) {
        setNote(
          `Archived.${kept ? ` Worktree and branch kept: ${kept}.` : ""}${kids ? ` The ${kids} session${kids === 1 ? "" : "s"} it spawned keep${kids === 1 ? "s" : ""} going; archive ${kids === 1 ? "it" : "them"} separately.` : ""}`,
        );
      } else go({ name: "sessions", project: s().project });
    });

  const [menuOpen, setMenuOpen] = createSignal(false);
  createEffect(() => {
    if (!menuOpen()) return;
    const close = () => setMenuOpen(false);
    // Deferred a tick so the click that opened the menu doesn't close it.
    const t = setTimeout(() => document.addEventListener("click", close), 0);
    onCleanup(() => {
      clearTimeout(t);
      document.removeEventListener("click", close);
    });
  });

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
        <Show when={busy()}>
          <button class="btn quiet" onClick={() => act(conn.call("session.interrupt", { session: s().id }))}>
            Interrupt
          </button>
        </Show>
        {/* The side panel is hidden on narrow screens, so the header carries it there. */}
        <MergeButton class="in-header" row={s} refresh={refreshChanges} />
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
        <span class="menu">
          <button
            class="btn quiet more"
            aria-label="Session actions"
            aria-haspopup="menu"
            aria-expanded={menuOpen()}
            onClick={() => setMenuOpen(!menuOpen())}
          >
            ⋯
          </button>
          <Show when={menuOpen()}>
            <div class="menu-list" role="menu">
              <Show when={s().alive && !busy()}>
                <button role="menuitem" title="Stop the agent process; it resumes on your next message" onClick={() => act(conn.call("session.stop", { session: s().id }))}>
                  Stop agent
                </button>
              </Show>
              <Show when={s().workspace.mode === "worktree" && sum().files > 0}>
                <button role="menuitem" title="Start a fresh session that reviews this worktree's changes" onClick={reviewChanges}>
                  Review changes
                </button>
              </Show>
              <OpenInItems row={s} onError={setError} />
              <Show
                when={!s().archived}
                fallback={
                  <button role="menuitem" onClick={() => act(conn.call("session.unarchive", { session: s().id }))}>
                    Unarchive
                  </button>
                }
              >
                <button
                  role="menuitem"
                  title={liveChildren().length ? `Archiving doesn't archive the ${liveChildren().length} session(s) it spawned` : undefined}
                  onClick={archive}
                >
                  Archive
                </button>
              </Show>
            </div>
          </Show>
        </span>
        </header>
      <div class="sh-meta">
          <Show when={s().workspace.branch} fallback={<span>project directory</span>}>
            <span>
              worktree <code>{s().workspace.branch}</code>
            </span>
          </Show>
          <Show when={s().parent}>
            <span class="prov">
              spawned by{" "}
              <a href={href({ name: "sessions", project: s().project, session: s().parent ?? undefined })}>
                "{spawner()?.title ?? "another session"}"
              </a>
            </span>
          </Show>
          <Show when={s().workspace.mode === "shared"}>
            <span class="prov">
              reading{" "}
              <a href={href({ name: "sessions", project: s().project, session: s().workspace.from ?? undefined })}>
                "{sourceName()}"
              </a>
              's worktree
            </span>
          </Show>
          <Show when={s().workspace.mode === "worktree" && s().workspace.snapshot}>
            <span class="prov">
              branched from{" "}
              <a href={href({ name: "sessions", project: s().project, session: s().workspace.from ?? undefined })}>
                "{sourceName()}"
              </a>{" "}
              at <code>{shortCommit(s().workspace.snapshot ?? "")}</code>
            </span>
          </Show>
          <Show when={readers().length}>
            <span class="prov">
              read by{" "}
              {readers().map((r, i) => (
                <>
                  {i > 0 ? ", " : ""}
                  <a href={href({ name: "sessions", project: s().project, session: r.id })}>"{r.title}"</a>
                </>
              ))}
            </span>
          </Show>
          <span>{s().turns === 1 ? "1 turn" : `${s().turns} turns`}</span>
          <Show when={live.t.context}>{(n) => <span>{tokens(n())}</span>}</Show>
          <Show when={s().cost_usd > 0}>
            <span>{usd(s().cost_usd)}</span>
          </Show>
          <Show when={live.t.model ?? s().model}>{(m) => <span>{m()}</span>}</Show>
      </div>

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
            <h4 class="cap">{s().workspace.mode === "shared" ? `${sourceName()}'s changes` : "Changes"}</h4>
            <MergeButton row={s} refresh={refreshChanges} />
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
