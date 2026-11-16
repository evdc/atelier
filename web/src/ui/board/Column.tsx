import { For, Show, type Accessor } from "solid-js";
import type { Column as Col } from "../../lib/board";
import type { TicketRow } from "../../gen/TicketRow";
import { Card } from "./Card";

/** A board column. Cards are keyed by row object, which `createView` keeps stable, so a status change
 *  re-renders one card in two columns. Under ~300 cards per column this needs no virtualization. */
export function Column(props: {
  col: Accessor<Col>;
  collapsed: boolean;
  onToggle: () => void;
  onOpen?: (t: TicketRow) => void;
}) {
  const multi = () => props.col().statuses.length > 1;
  return (
    <section class="bd-col" classList={{ shut: props.collapsed }} data-status={props.col().status ?? ""}>
      <Show
        when={!props.collapsed}
        fallback={
          <button class="strip" onClick={props.onToggle} aria-expanded="false" title={`Expand ${props.col().title}`}>
            <span class="t">{props.col().title}</span>
            <span class="n">{props.col().tickets.length}</span>
          </button>
        }
      >
        <header>
          <h2>{props.col().title}</h2>
          <span class="n">{props.col().tickets.length}</span>
          <button class="btn quiet fold" onClick={props.onToggle} aria-expanded="true" title={`Collapse ${props.col().title}`}>
            ‹
          </button>
        </header>
        <div class="cards">
          <For each={props.col().tickets}>
            {(t) => (
              <Card t={t} showStatus={multi() && t.status !== props.col().status} onOpen={props.onOpen && (() => props.onOpen!(t))} />
            )}
          </For>
        </div>
      </Show>
    </section>
  );
}
