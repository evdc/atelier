import { createSignal, For, Show } from "solid-js";
import type { Image } from "../gen/Image";
import { dataUrl, isImage, MAX_IMAGES, toImage } from "../lib/images";

export function Composer(props: { waiting: boolean; onSend: (text: string, images: Image[]) => Promise<unknown> }) {
  const [text, setText] = createSignal("");
  const [images, setImages] = createSignal<Image[]>([]);
  const [error, setError] = createSignal<string | null>(null);
  let el!: HTMLTextAreaElement;

  // Grow with content, up to the CSS max-height.
  const fit = () => {
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  async function attach(files: File[]) {
    const pics = files.filter(isImage);
    if (!pics.length) return;
    setError(null);
    try {
      const added = await Promise.all(pics.slice(0, MAX_IMAGES - images().length).map(toImage));
      setImages((cur) => [...cur, ...added]);
      if (pics.length > added.length) setError(`At most ${MAX_IMAGES} images per message.`);
    } catch (e) {
      setError((e as Error).message);
    }
  }

  const canSend = () => !!text().trim() || images().length > 0;

  async function send() {
    const t = text().trim();
    const imgs = images();
    if (!canSend()) return;
    setError(null);
    setText("");
    setImages([]);
    queueMicrotask(fit);
    try {
      await props.onSend(t, imgs);
    } catch (e) {
      setText(t); // give it back rather than lose it
      setImages(imgs);
      setError((e as Error).message);
    }
  }

  return (
    <>
      <Show when={error()}>
        <p class="err composer-note">{error()}</p>
      </Show>
      <Show when={images().length}>
        <div class="composer-imgs">
          <For each={images()}>
            {(img, n) => (
              <span class="thumb">
                <img src={dataUrl(img)} alt="attached" />
                <button title="Remove" onClick={() => setImages((cur) => cur.filter((_, j) => j !== n()))}>
                  ×
                </button>
              </span>
            )}
          </For>
        </div>
      </Show>
      <div
        class="composer"
        onDragOver={(e) => e.dataTransfer?.types.includes("Files") && e.preventDefault()}
        onDrop={(e) => {
          if (!e.dataTransfer?.files.length) return;
          e.preventDefault();
          void attach([...e.dataTransfer.files]);
        }}
      >
        <textarea
          ref={el}
          rows={1}
          placeholder={props.waiting ? "Waiting on you above — or reply here…" : "Reply…"}
          value={text()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            fit();
          }}
          onPaste={(e) => {
            const files = [...(e.clipboardData?.files ?? [])].filter(isImage);
            if (!files.length) return;
            e.preventDefault();
            void attach(files);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button class="btn dark" disabled={!canSend()} onClick={() => void send()}>
          Send
        </button>
      </div>
    </>
  );
}
