import { createSignal, Show } from "solid-js";

export function Composer(props: { waiting: boolean; onSend: (text: string) => Promise<unknown> }) {
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  let el!: HTMLTextAreaElement;

  // Grow with content, up to the CSS max-height.
  const fit = () => {
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  async function send() {
    const t = text().trim();
    if (!t) return;
    setError(null);
    setText("");
    queueMicrotask(fit);
    try {
      await props.onSend(t);
    } catch (e) {
      setText(t); // give it back rather than lose it
      setError((e as Error).message);
    }
  }

  return (
    <>
      <Show when={error()}>
        <p class="err composer-note">{error()}</p>
      </Show>
      <div class="composer">
        <textarea
          ref={el}
          rows={1}
          placeholder={props.waiting ? "Waiting on you above — or reply here…" : "Reply…"}
          value={text()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            fit();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button class="btn dark" disabled={!text().trim()} onClick={() => void send()}>
          Send
        </button>
      </div>
    </>
  );
}
