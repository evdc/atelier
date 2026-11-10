import { createSignal, Show } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { rowSwipe, rowSwipeCommits } from "../lib/phone";
import { href } from "../lib/router";
import { ago } from "../lib/time";
import { useApp } from "./context";

/** One session in a list. `projectName` is shown in cross-project lists.
 *  On touch, swiping left archives it. */
export function SessionRowLink(props: { s: SessionRow; on?: boolean; projectName?: string }) {
  const { conn } = useApp();
  const s = () => props.s;
  const [armed, setArmed] = createSignal(false);
  let el: HTMLAnchorElement | undefined;
  let start: { id: number; x: number; y: number; live: boolean } | null = null;
  let dx = 0;
  let frame = 0;
  let swiped = false;

  const paint = () => {
    frame = 0;
    if (el) el.style.transform = dx < 0 ? `translateX(${dx}px)` : "";
    setArmed(rowSwipeCommits(dx, el?.offsetWidth ?? 360));
  };
  const reset = () => {
    start = null;
    dx = 0;
    cancelAnimationFrame(frame);
    paint();
    setArmed(false);
  };

  return (
    <div class="swipe">
      <span class="swipe-act" classList={{ armed: armed() }} aria-hidden="true">
        Archive
      </span>
      <a
        ref={el}
        class="row"
        classList={{ on: !!props.on }}
        href={href({ name: "sessions", project: s().project, session: s().id })}
        onPointerDown={(e) => {
          if (e.pointerType !== "touch") return;
          start = { id: e.pointerId, x: e.clientX, y: e.clientY, live: false };
          swiped = false;
        }}
        onPointerMove={(e) => {
          if (!start || e.pointerId !== start.id) return;
          const v = rowSwipe(e.clientX - start.x, e.clientY - start.y);
          if (v === null) return reset();
          if (v === "pending") return;
          if (!start.live) {
            start.live = true;
            el?.setPointerCapture(e.pointerId);
          }
          dx = v;
          if (!frame) frame = requestAnimationFrame(paint);
        }}
        onPointerUp={(e) => {
          if (!start || e.pointerId !== start.id) return;
          const commit = start.live && rowSwipeCommits(e.clientX - start.x, el?.offsetWidth ?? 360);
          swiped = start.live;
          reset();
          if (commit && !s().archived) conn.call("session.archive", { session: s().id }).catch(() => {});
        }}
        onPointerCancel={reset}
        // A drag that ends over the link would otherwise also navigate.
        onClick={(e) => {
          if (swiped) {
            e.preventDefault();
            swiped = false;
          }
        }}
      >
        <i class={`st ${s().status}`} />
        <span class="t">{s().title}</span>
        <span class="when">{ago(s().updated_at)}</span>
        <span class="m">
          <Show when={s().status === "waiting"}>
            <span class="needs">waiting on you</span>
          </Show>
          <Show when={s().archived}>
            <span class="faint">archived</span>
          </Show>
          <Show when={props.projectName}>
            <span>{props.projectName}</span>
          </Show>
          <Show when={s().imported}>
            <span>imported</span>
          </Show>
          <span>{s().workspace.branch ?? "project dir"}</span>
        </span>
      </a>
    </div>
  );
}
