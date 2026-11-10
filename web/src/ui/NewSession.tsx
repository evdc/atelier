import { createSignal, For, Show } from "solid-js";
import type { ProjectRow } from "../gen/ProjectRow";
import type { WorkspaceMode } from "../gen/WorkspaceMode";
import { go } from "../lib/router";
import { takePrefill, type Source } from "../lib/review";
import { useApp } from "./context";

/** The workspace select: a mode, or a session whose worktree is shared or branched from. */
type Choice = WorkspaceMode | Source;

export function NewSession(props: { project: ProjectRow }) {
  const { conn, sessions } = useApp();
  const pre = takePrefill();
  const [prompt, setPrompt] = createSignal(pre?.prompt ?? "");
  const [choice, setChoice] = createSignal<Choice>(pre?.source ?? (props.project.is_git ? "worktree" : "direct"));
  const [from, setFrom] = createSignal(pre?.from ?? "");
  const [model, setModel] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // This project's live sessions that have a worktree to share or branch from.
  const sources = () =>
    sessions
      .list()
      .filter(
        (s) =>
          s.project === props.project.id &&
          !s.archived &&
          (s.workspace.mode === "worktree" || s.workspace.mode === "shared"),
      );
  const needsFrom = () => choice() === "share" || choice() === "branch";
  const canStart = () => !!prompt().trim() && !busy() && (!needsFrom() || !!from());

  function pickChoice(next: Choice) {
    setChoice(next);
    if ((next === "share" || next === "branch") && !from()) setFrom(sources()[0]?.id ?? "");
  }

  async function start(e?: Event) {
    e?.preventDefault();
    if (!canStart()) return;
    setBusy(true);
    setError(null);
    const c = choice();
    try {
      const res = (await conn.call("session.create", {
        project: props.project.id,
        prompt: prompt(),
        workspace: c === "share" ? "shared" : c === "branch" ? "worktree" : c,
        from: needsFrom() ? from() : undefined,
        title: pre?.title || undefined,
        model: model() || undefined,
      })) as { id: string };
      go({ name: "sessions", project: props.project.id, session: res.id });
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <form class="new" onSubmit={start}>
      <h3>A new session</h3>
      <textarea
        class="field"
        placeholder="What should it work on?"
        value={prompt()}
        onInput={(e) => setPrompt(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) start(e);
        }}
        ref={(el) => queueMicrotask(() => el.focus())}
      />
      <div class="opts">
        <label>
          Work in
          <select
            class="field"
            value={choice()}
            onChange={(e) => pickChoice(e.currentTarget.value as Choice)}
          >
            <option value="worktree" disabled={!props.project.is_git}>
              a new worktree
            </option>
            <option value="direct">the project directory</option>
            <option value="share" disabled={!props.project.is_git}>
              Share a session's worktree (read-only)…
            </option>
            <option value="branch" disabled={!props.project.is_git}>
              Branch from a session…
            </option>
          </select>
        </label>
        <Show when={needsFrom()}>
          <label>
            Session
            <select class="field" value={from()} onChange={(e) => setFrom(e.currentTarget.value)}>
              <option value="" disabled>
                {sources().length ? "Pick a session" : "No sessions with a worktree"}
              </option>
              <For each={sources()}>
                {(s) => (
                  <option value={s.id}>
                    {s.title}
                    {s.workspace.branch ? ` · ${s.workspace.branch}` : ""}
                  </option>
                )}
              </For>
            </select>
          </label>
        </Show>
        <label>
          Model
          <select class="field" value={model()} onChange={(e) => setModel(e.currentTarget.value)}>
            <option value="">default</option>
            <option value="opus">Opus</option>
            <option value="sonnet">Sonnet</option>
            <option value="haiku">Haiku</option>
          </select>
        </label>
        <span class="grow" />
        <span class="faint">⌘↵</span>
        <button class="btn dark" disabled={!canStart()}>
          Start
        </button>
      </div>
      <Show when={!props.project.is_git}>
        <p class="faint">Not a git repository, so the agent works in the project directory itself.</p>
      </Show>
      <Show when={choice() === "share"}>
        <p class="faint">
          Read-only by convention: the agent starts with its edit tools off and is told not to change files. Bash can
          still write, and any change shows in that session's Changes.
        </p>
      </Show>
      <Show when={choice() === "branch"}>
        <p class="faint">
          A new worktree at a snapshot of that session's working tree, uncommitted files included. That session is not
          touched.
        </p>
      </Show>
      <Show when={error()}>
        <p class="err">{error()}</p>
      </Show>
    </form>
  );
}
