import { For, onCleanup, onMount } from "solid-js";
import type { Column } from "../../lib/board";

/** "Move to…" for one card: the columns that take drops, except the card's own. Arrows move, Enter picks, Escape closes. */
export function MoveMenu(props: {
  cols: Column[];
  /** Title of the card's current column. */
  current: string;
  at: DOMRect;
  onPick: (status: string) => void;
  onClose: () => void;
}) {
  let ul: HTMLUListElement | undefined;
  const items = () => [...ul!.querySelectorAll<HTMLButtonElement>("button")];
  const away = (e: PointerEvent) => !ul?.contains(e.target as Node) && props.onClose();
  onMount(() => {
    items()[0]?.focus();
    // After this tick, so the press that opened the menu doesn't close it.
    setTimeout(() => addEventListener("pointerdown", away, true));
  });
  onCleanup(() => removeEventListener("pointerdown", away, true));

  const onKey = (e: KeyboardEvent) => {
    const b = items();
    const i = b.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") props.onClose();
    else if (e.key === "ArrowDown") b[(i + 1) % b.length]?.focus();
    else if (e.key === "ArrowUp") b[(i - 1 + b.length) % b.length]?.focus();
    else if (e.key === "Tab") props.onClose();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };
  const top = () => Math.min(props.at.bottom + 4, innerHeight - 8 - 40 * (props.cols.length + 1));
  const left = () => Math.max(8, Math.min(props.at.left, innerWidth - 200));

  return (
    <ul class="bd-menu" role="menu" aria-label="Move to…" ref={ul} onKeyDown={onKey} style={{ top: `${top()}px`, left: `${left()}px` }}>
      <li class="h" role="presentation">Move to…</li>
      <For each={props.cols.filter((c) => c.status && c.title !== props.current)}>
        {(c) => (
          <li role="none">
            <button role="menuitem" onClick={() => props.onPick(c.status!)}>
              {c.title}
            </button>
          </li>
        )}
      </For>
    </ul>
  );
}
