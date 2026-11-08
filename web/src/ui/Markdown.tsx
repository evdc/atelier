import { renderMarkdown } from "./mdrender";

/** Renders a *finished* message once. Items are immutable, so the HTML is computed a single time per mount. */
export function Markdown(props: { text: string }) {
  // Links must never navigate the app window (Tauri); open them in a new context instead.
  const onClick = (e: MouseEvent) => {
    const a = (e.target as Element).closest?.("a");
    if (!a) return;
    e.preventDefault();
    window.open(a.href, "_blank", "noreferrer");
  };
  // eslint-disable-next-line solid/no-innerhtml -- sanitized by renderMarkdown (raw HTML escaped, link schemes allowlisted)
  return <div class="body md" innerHTML={renderMarkdown(props.text)} onClick={onClick} />;
}
