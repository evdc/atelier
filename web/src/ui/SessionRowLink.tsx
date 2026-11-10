import { Show } from "solid-js";
import type { SessionRow } from "../gen/SessionRow";
import { href } from "../lib/router";
import { ago } from "../lib/time";

/** One session in a list. `projectName` is shown in cross-project lists. */
export function SessionRowLink(props: { s: SessionRow; on?: boolean; projectName?: string }) {
  const s = () => props.s;
  return (
    <a class="row" classList={{ on: !!props.on }} href={href({ name: "sessions", project: s().project, session: s().id })}>
      <i class={`st ${s().status}`} />
      <span class="t">{s().title}</span>
      <span class="when">{ago(s().updated_at)}</span>
      <span class="m">
        <Show when={s().status === "waiting"}>
          <span class="needs">waiting on you</span>
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
  );
}
