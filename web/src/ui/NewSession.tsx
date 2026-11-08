import { createSignal, Show } from "solid-js";
import type { ProjectRow } from "../gen/ProjectRow";
import type { WorkspaceMode } from "../gen/WorkspaceMode";
import { go } from "../lib/router";
import { useApp } from "./context";

export function NewSession(props: { project: ProjectRow }) {
  const { conn } = useApp();
  const [prompt, setPrompt] = createSignal("");
  const [workspace, setWorkspace] = createSignal<WorkspaceMode>(props.project.is_git ? "worktree" : "direct");
  const [model, setModel] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function start(e?: Event) {
    e?.preventDefault();
    if (!prompt().trim() || busy()) return;
    setBusy(true);
    setError(null);
    try {
      const res = (await conn.call("session.create", {
        project: props.project.id,
        prompt: prompt(),
        workspace: workspace(),
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
          <select class="field" value={workspace()} onChange={(e) => setWorkspace(e.currentTarget.value as WorkspaceMode)}>
            <option value="worktree" disabled={!props.project.is_git}>
              a new worktree
            </option>
            <option value="direct">the project directory</option>
          </select>
        </label>
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
        <button class="btn dark" disabled={busy() || !prompt().trim()}>
          Start
        </button>
      </div>
      <Show when={!props.project.is_git}>
        <p class="faint">Not a git repository, so the agent works in the project directory itself.</p>
      </Show>
      <Show when={error()}>
        <p class="err">{error()}</p>
      </Show>
    </form>
  );
}
