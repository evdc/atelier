import { createMemo, createSignal, For, Index, Show } from "solid-js";
import type { BoardConfig } from "../../gen/BoardConfig";
import type { TicketRow } from "../../gen/TicketRow";
import * as d from "../../lib/boardDraft";
import { useApp } from "../context";
import "../../styles/boardeditor.css";

/** Edits a draft of the project's board config; nothing is written until Save. Mounted only while open. */
export function BoardEditor(props: {
  project: string;
  config: BoardConfig;
  tickets: readonly TicketRow[];
  onClose: () => void;
  /** After a successful write; the `board` watch delivers the change itself. */
  onSaved: () => void;
}) {
  const { conn } = useApp();
  const [draft, setDraft] = createSignal(d.clone(props.config));
  const [busy, setBusy] = createSignal(false);
  const [refused, setRefused] = createSignal<string | null>(null);
  const edit = (f: (c: BoardConfig) => BoardConfig) => {
    setDraft(f(draft()));
    setRefused(null);
  };

  const issues = createMemo(() => d.problems(draft()));
  const gone = createMemo(() => d.orphaned(draft(), props.tickets));

  async function write(config: BoardConfig) {
    setBusy(true);
    setRefused(null);
    try {
      await conn.call("board.update", { project: props.project, config });
      props.onSaved();
      props.onClose();
    } catch (e) {
      setRefused(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }
  const reset = () => {
    if (confirm("Replace this board's columns and statuses with the defaults (open, in-progress, done)?")) void write(d.DEFAULTS);
  };

  const [newStatus, setNewStatus] = createSignal("");
  const addStatus = () => {
    const s = newStatus().trim();
    if (s) edit((c) => d.addStatus(c, s));
    setNewStatus("");
  };

  return (
    <>
      <div class="be-back" onClick={props.onClose} />
      <aside class="be" role="dialog" aria-label="Edit columns" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
        <header>
          <h3>Edit columns</h3>
          <button class="btn quiet" onClick={props.onClose}>
            Cancel
          </button>
          <button class="btn dark" disabled={busy() || issues().length > 0} onClick={() => void write(draft())}>
            Save
          </button>
        </header>
        <div class="be-body">
          <Show when={issues().length || refused()}>
            <ul class="be-problems" role="alert">
              <For each={issues()}>{(p) => <li>{p}</li>}</For>
              <Show when={refused()}>{(r) => <li>{r()}</li>}</Show>
            </ul>
          </Show>

          <section>
            <h4>Columns</h4>
            <Index each={draft().columns}>
              {(col, i) => (
                <div class="be-col">
                  <div class="be-row">
                    <input
                      class="field"
                      value={col().title}
                      aria-label="Column title"
                      onInput={(e) => edit((c) => d.renameColumn(c, i, e.currentTarget.value))}
                    />
                    <label class="be-check">
                      <input type="checkbox" checked={col().collapsed} onChange={() => edit((c) => d.toggleCollapsed(c, i))} /> collapsed
                    </label>
                    <button class="btn quiet" disabled={i === 0} aria-label="Move up" onClick={() => edit((c) => d.moveColumn(c, i, -1))}>
                      ↑
                    </button>
                    <button
                      class="btn quiet"
                      disabled={i === draft().columns.length - 1}
                      aria-label="Move down"
                      onClick={() => edit((c) => d.moveColumn(c, i, 1))}
                    >
                      ↓
                    </button>
                    <button class="btn quiet danger" aria-label={`Delete ${col().title}`} onClick={() => edit((c) => d.removeColumn(c, i))}>
                      Delete
                    </button>
                  </div>
                  <ul class="be-chips">
                    <Index each={col().statuses}>
                      {(s, k) => (
                        <li class="be-chip">
                          <span>{s()}</span>
                          <Show when={k === 0}>
                            <em>set when dropped here</em>
                          </Show>
                          <Show when={k > 0}>
                            <button aria-label={`Make ${s()} first`} title="Make first" onClick={() => edit((c) => d.moveChip(c, i, k, -1))}>
                              ‹
                            </button>
                          </Show>
                          <Show when={k < col().statuses.length - 1}>
                            <button aria-label={`Move ${s()} later`} title="Move later" onClick={() => edit((c) => d.moveChip(c, i, k, 1))}>
                              ›
                            </button>
                          </Show>
                          <button aria-label={`Remove ${s()}`} title="Remove" onClick={() => edit((c) => d.removeChip(c, i, k))}>
                            ×
                          </button>
                        </li>
                      )}
                    </Index>
                    <li>
                      <input
                        class="field be-add"
                        placeholder="Add status…"
                        list="be-known"
                        aria-label={`Add a status to ${col().title}`}
                        onKeyDown={(e) => {
                          const v = e.currentTarget.value.trim();
                          if (e.key !== "Enter" || !v) return;
                          e.preventDefault();
                          edit((c) => d.addChip(c, i, v));
                          e.currentTarget.value = "";
                        }}
                        onChange={(e) => {
                          // Picking from the list fires change without Enter.
                          const v = e.currentTarget.value.trim();
                          if (v && draft().statuses.includes(v)) {
                            edit((c) => d.addChip(c, i, v));
                            e.currentTarget.value = "";
                          }
                        }}
                      />
                    </li>
                  </ul>
                </div>
              )}
            </Index>
            <datalist id="be-known">
              <For each={draft().statuses}>{(s) => <option value={s} />}</For>
            </datalist>
            <button class="btn" onClick={() => edit((c) => d.addColumn(c, uniqueTitle(c), uniqueStatus(c)))}>
              Add column
            </button>
          </section>

          <section>
            <h4>Statuses</h4>
            <p class="faint">
              Renaming a status here doesn't change tickets. They keep their old status and show under Other until moved.
            </p>
            <Index each={draft().statuses}>
              {(s) => (
                <div class="be-row">
                  <input
                    class="field"
                    value={s()}
                    aria-label={`Rename ${s()}`}
                    // On commit, not per keystroke: renaming rewrites every column holding it.
                    onChange={(e) => {
                      const to = e.currentTarget.value.trim();
                      if (to && to !== s() && !draft().statuses.includes(to)) edit((c) => d.renameStatus(c, s(), to));
                      else e.currentTarget.value = s();
                    }}
                  />
                  <Show when={d.usedBy(props.tickets, s())}>{(n) => <span class="faint">{n()} used</span>}</Show>
                  <button class="btn quiet danger" aria-label={`Remove status ${s()}`} onClick={() => edit((c) => d.removeStatus(c, s()))}>
                    Remove
                  </button>
                </div>
              )}
            </Index>
            <form
              class="be-row"
              onSubmit={(e) => {
                e.preventDefault();
                addStatus();
              }}
            >
              <input class="field" placeholder="New status" value={newStatus()} onInput={(e) => setNewStatus(e.currentTarget.value)} />
              <button class="btn" disabled={!newStatus().trim()}>
                Add status
              </button>
            </form>
            <Show when={gone().length}>
              <ul class="be-warn">
                <For each={gone()}>
                  {(g) => (
                    <li>
                      {g.count} ticket{g.count === 1 ? " has" : "s have"} status <i>{g.status}</i>; they'll show under Other.
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </section>

          <footer>
            <button class="btn quiet" disabled={busy()} onClick={reset}>
              Reset to defaults
            </button>
          </footer>
        </div>
      </aside>
    </>
  );
}

const uniqueTitle = (c: BoardConfig) => fresh("New column", c.columns.map((x) => x.title));
const uniqueStatus = (c: BoardConfig) => fresh("new", c.statuses);
function fresh(base: string, taken: string[]): string {
  let n = base;
  for (let i = 2; taken.includes(n); i++) n = `${base} ${i}`;
  return n;
}
