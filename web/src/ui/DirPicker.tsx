import { createSignal, For, onMount, Show } from "solid-js";
import type { DirListing } from "../gen/DirListing";
import type { Suggestion } from "../gen/Suggestion";
import { useApp } from "./context";

/** Matches the server's `fs.list` cap; the picker says when it's reached. */
const LIST_CAP = 500;

/**
 * Browse the server's directories to pick a project folder. The server may be
 * remote, so this lists its filesystem rather than opening a local dialog.
 * Every folder shown is reported through `onPick`, so the path field follows
 * the browsing; "Add this folder" hands control back to the add form.
 */
export function DirPicker(props: { start?: string; onPick: (path: string) => void; onAdd: () => void; busy: boolean }) {
  const { conn } = useApp();
  const [dir, setDir] = createSignal<DirListing | null>(null);
  const [suggestions, setSuggestions] = createSignal<Suggestion[]>([]);
  const [hidden, setHidden] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  // Only the latest navigation may land: a slow listing must not overwrite a newer one.
  let seq = 0;

  function open(path: string | undefined, showHidden = hidden()) {
    const mine = ++seq;
    setLoading(true);
    conn
      .call("fs.list", { path, hidden: showHidden })
      .then(
        (l) => {
          if (mine !== seq) return;
          const d = l as DirListing;
          setDir(d);
          setError(null);
          props.onPick(d.path);
        },
        (e: Error) => {
          // The last good listing stays on screen.
          if (mine === seq) setError(e.message);
        },
      )
      .finally(() => {
        if (mine === seq) setLoading(false);
      });
  }

  onMount(() => {
    open(props.start);
    conn.call("fs.suggest", {}).then(
      (s) => setSuggestions((s as { entries: Suggestion[] }).entries),
      () => {},
    );
  });

  const crumbs = () => {
    const parts = (dir()?.path ?? "").split("/").filter(Boolean);
    return [{ label: "/", path: "/" }, ...parts.map((label, i) => ({ label, path: "/" + parts.slice(0, i + 1).join("/") }))];
  };

  const child = (name: string) => {
    const p = dir()?.path ?? "";
    return `${p === "/" ? "" : p}/${name}`;
  };

  return (
    <div class="picker">
      <nav class="picker-h" aria-label="Current folder">
        <For each={crumbs()}>
          {(c, i) => (
            <>
              <Show when={i() > 1}>
                <span class="picker-sep">/</span>
              </Show>
              <button
                class="crumb"
                aria-current={i() === crumbs().length - 1 ? "location" : undefined}
                onClick={() => open(c.path)}
              >
                {c.label}
              </button>
            </>
          )}
        </For>
      </nav>

      <Show when={suggestions().length}>
        <div class="picker-sub">Recent repositories</div>
        <ul class="picker-list">
          <For each={suggestions()}>
            {(s) => (
              <li>
                <button onClick={() => open(s.path)}>
                  <span class="name">{s.name}</span>
                  <span class="faint">{s.path}</span>
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>

      <Show when={dir()}>
        {(d) => (
          <>
            <Show when={d().entries.length} fallback={<p class="faint picker-msg">No folders here.</p>}>
              <ul class="picker-list">
                <For each={d().entries}>
                  {(e) => (
                    <li>
                      <button onClick={() => open(child(e.name))}>
                        <span class="name">{e.name}</span>
                        <Show when={e.is_git}>
                          <span class="tag">git</span>
                        </Show>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <Show when={d().entries.length >= LIST_CAP}>
              <p class="faint picker-msg">Showing the first {LIST_CAP} folders.</p>
            </Show>
          </>
        )}
      </Show>

      <Show when={loading()}>
        <p class="faint picker-msg">Loading…</p>
      </Show>
      <Show when={error()}>
        <p class="err picker-msg">{error()}</p>
      </Show>

      <div class="picker-f">
        <label class="faint">
          <input
            type="checkbox"
            checked={hidden()}
            onChange={(e) => {
              setHidden(e.currentTarget.checked);
              open(dir()?.path ?? props.start);
            }}
          />{" "}
          Show hidden
        </label>
        <span class="grow" />
        <button class="btn dark" disabled={props.busy || !dir()} onClick={() => props.onAdd()}>
          Add this folder
        </button>
      </div>
    </div>
  );
}
