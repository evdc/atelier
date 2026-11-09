import { createEffect, createSignal, For, on, onCleanup, Show, type Accessor } from "solid-js";
import { patchLines } from "../agents/patch";
import type { Changes } from "../gen/Changes";
import type { FileChange } from "../gen/FileChange";
import type { SessionRow } from "../gen/SessionRow";
import { serverConfig } from "../lib/conn";
import { canOpenIn, openIn, type OpenKind } from "../lib/openin";
import { useApp } from "./context";
import { Code } from "./tools/Code";

/**
 * A session's changes against its base. Fetched (not watched): refreshed when
 * the session settles after a turn, since git is the source of truth and
 * nothing is logged.
 */
export function createChanges(s: Accessor<SessionRow>): Accessor<Changes | null> {
  const { conn } = useApp();
  const [changes, setChanges] = createSignal<Changes | null>(null);
  const refresh = () => {
    if (s().archived) return;
    conn.call("session.changes", { session: s().id }).then(
      (c) => setChanges(c as Changes),
      () => setChanges(null),
    );
  };
  // Not while running: files are mid-edit, and git per tool call is wasted work.
  createEffect(on(() => [s().status, s().turns] as const, () => s().status !== "running" && refresh()));
  return changes;
}

export const totals = (c: Changes | null) => ({
  files: c?.files.length ?? 0,
  added: c?.files.reduce((n, f) => n + f.added, 0) ?? 0,
  removed: c?.files.reduce((n, f) => n + f.removed, 0) ?? 0,
});

/** The file list, each file expanding to its diff. */
export function ChangesList(props: { session: SessionRow["id"]; changes: Changes | null }) {
  return (
    <Show when={props.changes?.files.length} fallback={<p class="faint side-empty">No changes yet.</p>}>
      <ul class="chg">
        <For each={props.changes!.files}>{(f) => <FileDiff session={props.session} file={f} />}</For>
      </ul>
    </Show>
  );
}

const KINDS: [OpenKind, string][] = [
  ["terminal", "Terminal"],
  ["editor", "Editor"],
  ["finder", "Finder"],
];

/** "Open in…" menu. Desktop app + local server only: the path is a path on this machine. */
export function OpenIn(props: { row: Accessor<SessionRow> }) {
  const s = props.row;
  const [open, setOpen] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  createEffect(() => {
    if (!open()) return;
    const close = () => setOpen(false);
    // Deferred a tick so the click that opened the menu doesn't close it.
    const t = setTimeout(() => document.addEventListener("click", close), 0);
    onCleanup(() => {
      clearTimeout(t);
      document.removeEventListener("click", close);
    });
  });
  return (
    <Show when={canOpenIn(serverConfig()?.url ?? "") && !s().archived}>
      <span class="menu">
        <button class="btn quiet" aria-haspopup="menu" aria-expanded={open()} onClick={() => setOpen(!open())}>
          Open in… ▾
        </button>
        <Show when={open()}>
          <div class="menu-list" role="menu">
            <For each={KINDS}>
              {([kind, label]) => (
                <button
                  role="menuitem"
                  onClick={() => openIn(kind, s().workspace.path).then(() => setErr(null), (e) => setErr(String(e)))}
                >
                  {label}
                </button>
              )}
            </For>
          </div>
        </Show>
        <Show when={err()}>
          <span class="err">{err()}</span>
        </Show>
      </span>
    </Show>
  );
}

function FileDiff(props: { session: SessionRow["id"]; file: FileChange }) {
  const { conn } = useApp();
  const [lines, setLines] = createSignal<ReturnType<typeof patchLines> | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const load = (open: boolean) => {
    if (!open) return;
    conn.call("session.diff", { session: props.session, path: props.file.path }).then(
      (r) => setLines(patchLines((r as { patch: string }).patch)),
      (e: Error) => setError(e.message),
    );
  };
  return (
    <li>
      <details onToggle={(e) => load(e.currentTarget.open)}>
        <summary>
          <span class="path" title={props.file.path}>
            {props.file.path}
          </span>
          <Show when={props.file.status === "untracked"}>
            <span class="st-u">new</span>
          </Show>
          <span class="add-n">+{props.file.added}</span>
          <span class="del-n">−{props.file.removed}</span>
        </summary>
        <Show when={error()}>
          <p class="err">{error()}</p>
        </Show>
        <Show when={lines()} keyed>
          {(l) => (l.length ? <Code lines={l} /> : <p class="faint side-empty">No text changes (binary or mode only).</p>)}
        </Show>
      </details>
    </li>
  );
}
