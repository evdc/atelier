import { Show } from "solid-js";
import { href, tab, type Tab } from "../lib/router";
import { useApp } from "./context";

/** The phone's bottom bar (hidden on wide layouts, where the sidebar does this job). */
export function Tabs() {
  const { sessions } = useApp();
  const waiting = () => sessions.list().filter((s) => s.status === "waiting" && !s.archived).length;
  const item = (t: Tab, label: string, badge?: () => number) => (
    <a class="tab" classList={{ on: tab() === t }} href={href({ name: t })} aria-current={tab() === t ? "page" : undefined}>
      <span>{label}</span>
      <Show when={badge?.()}>{(n) => <span class="badge">{n()}</span>}</Show>
    </a>
  );
  return (
    <nav class="tabs">
      {item("waiting", "Waiting", waiting)}
      {item("projects", "Projects")}
      {item("all", "Sessions")}
    </nav>
  );
}
