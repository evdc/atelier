// A permission request or a question from the agent, answered inline.

import { createSignal, For, Match, Show, Switch, type Accessor } from "solid-js";
import type { PermissionDecision } from "../gen/PermissionDecision";
import type { SessionRow } from "../gen/SessionRow";
import type { Item } from "../agents/model";
import { useApp } from "./context";
import { summarize } from "../agents/tools";

type Perm = Extract<Item, { k: "permission" }>;

interface Question {
  question: string;
  header?: string;
  options: { label: string; description?: string }[];
  multiSelect?: boolean;
}

export function Ask(props: { item: Perm; session: Accessor<SessionRow> }) {
  const { conn } = useApp();
  const p = () => props.item;
  const [error, setError] = createSignal<string | null>(null);
  const [sent, setSent] = createSignal(false);

  async function answer(decision: PermissionDecision) {
    setSent(true);
    setError(null);
    try {
      await conn.call("session.answer", { session: props.session().id, request_id: p().id, decision });
    } catch (e) {
      setError((e as Error).message);
      setSent(false);
    }
  }

  const questions = () => ((p().input as { questions?: Question[] })?.questions ?? []) as Question[];

  return (
    <Switch>
      <Match when={p().state !== "pending"}>
        <p class="asked">
          <Show when={p().tool === "AskUserQuestion"} fallback={<><b>{p().tool}</b> {summarize(p().tool, p().input, props.session().workspace.path)} — </>}>
            <b>Asked</b> {questions()[0]?.question} —{" "}
          </Show>
          {{ allowed: "answered", denied: "declined", void: "expired", pending: "" }[p().state]}
        </p>
      </Match>
      <Match when={p().tool === "AskUserQuestion"}>
        <Questions questions={questions()} input={p().input} disabled={sent()} onAnswer={answer} error={error()} />
      </Match>
      <Match when={true}>
        <div class="ask">
          <span class="q">
            Claude wants to use <b>{p().tool}</b>
          </span>
          <pre>{describe(p().tool, p().input)}</pre>
          <div class="acts">
            <button class="btn dark" disabled={sent()} onClick={() => answer({ behavior: "allow" })}>
              Allow
            </button>
            <button
              class="btn"
              disabled={sent()}
              onClick={() => answer({ behavior: "deny", message: "The user declined this in Atelier." })}
            >
              Deny
            </button>
          </div>
          <Show when={error()}>
            <p class="err">{error()}</p>
          </Show>
        </div>
      </Match>
    </Switch>
  );
}

function Questions(props: {
  questions: Question[];
  input: unknown;
  disabled: boolean;
  error: string | null;
  onAnswer: (d: PermissionDecision) => void;
}) {
  const [picked, setPicked] = createSignal<Record<string, string[]>>({});
  const complete = () => props.questions.every((q) => (picked()[q.question] ?? []).length > 0);

  const submit = (sel: Record<string, string[]>) => {
    const answers = Object.fromEntries(Object.entries(sel).map(([q, ls]) => [q, ls.join(", ")]));
    props.onAnswer({ behavior: "allow", updated_input: { ...(props.input as object), answers } });
  };

  const pick = (q: Question, label: string) => {
    const cur = picked()[q.question] ?? [];
    const next = q.multiSelect ? (cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label]) : [label];
    const sel = { ...picked(), [q.question]: next };
    setPicked(sel);
    // A single single-choice question answers on click.
    if (props.questions.length === 1 && !q.multiSelect) submit(sel);
  };

  return (
    <div class="ask">
      <For each={props.questions}>
        {(q) => (
          <>
            <span class="q">{q.question}</span>
            <div class="acts">
              <For each={q.options}>
                {(o) => (
                  <button
                    class="btn opt"
                    classList={{ dark: (picked()[q.question] ?? []).includes(o.label) }}
                    disabled={props.disabled}
                    onClick={() => pick(q, o.label)}
                  >
                    {o.label}
                    <Show when={o.description}>
                      <small>{o.description}</small>
                    </Show>
                  </button>
                )}
              </For>
            </div>
          </>
        )}
      </For>
      <div class="acts">
        <Show when={props.questions.length > 1 || props.questions.some((q) => q.multiSelect)}>
          <button class="btn dark" disabled={props.disabled || !complete()} onClick={() => submit(picked())}>
            Answer
          </button>
        </Show>
        <button
          class="btn quiet"
          disabled={props.disabled}
          onClick={() => props.onAnswer({ behavior: "deny", message: "The user chose not to answer." })}
        >
          Skip
        </button>
      </div>
      <Show when={props.error}>
        <p class="err">{props.error}</p>
      </Show>
    </div>
  );
}

function describe(tool: string, input: unknown): string {
  const i = (input ?? {}) as Record<string, unknown>;
  if (tool === "Bash" && typeof i.command === "string") return i.command;
  return JSON.stringify(input, null, 2);
}
