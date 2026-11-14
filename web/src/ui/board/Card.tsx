import { For, Show } from "solid-js";
import type { TicketRow } from "../../gen/TicketRow";

/** One ticket. `status` is shown only when the column holds several (the caller says so), and `onOpen`
 *  adds an "Open in editor" action. Drag-and-drop wraps this; it doesn't know about it. */
export function Card(props: { t: TicketRow; showStatus: boolean; onOpen?: () => void }) {
  const t = () => props.t;
  const meta = () => [t().size, t().area].filter(Boolean).join(" · ");
  return (
    <article class="bd-card" data-key={t().key}>
      <header>
        <span class="id">{t().id}</span>
        <Show when={t().problem}>
          {(p) => (
            <span class="mark" title={p()} aria-label={`Problem: ${p()}`}>
              ◆
            </span>
          )}
        </Show>
        <Show when={props.showStatus && t().status}>
          <span class="tag">{t().status}</span>
        </Show>
      </header>
      <h3>{t().title}</h3>
      <Show when={meta() || t().checks_total}>
        <p class="meta">
          <span>{meta()}</span>
          <Show when={t().checks_total}>
            <span class="mono">
              {t().checks_done}/{t().checks_total}
            </span>
          </Show>
        </p>
      </Show>
      <Show when={t().depends.length}>
        <p class="deps">
          after{" "}
          <For each={t().depends}>{(d, i) => <span class="mono">{i() ? `, ${d}` : d}</span>}</For>
        </p>
      </Show>
      <Show when={props.onOpen}>
        <button class="btn quiet open" onClick={() => props.onOpen!()}>
          Open in editor
        </button>
      </Show>
    </article>
  );
}
