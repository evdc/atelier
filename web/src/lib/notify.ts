// The side effects of "needs you": a notification, and the desktop dock badge.
//
// On desktop, tauri-plugin-notification's init script replaces `window.Notification`
// with one that calls the plugin, so the web API is the single path here. In the
// browser dev UI it is the browser's own; on the phone there is neither, and
// nothing happens (the phone gets push later).

import type { SessionRow } from "../gen/SessionRow";
import { tauri } from "./openin";
import { go } from "./router";

export async function notifyNeedsYou(s: SessionRow): Promise<void> {
  try {
    if (typeof Notification === "undefined") return;
    if (Notification.permission === "default") await Notification.requestPermission();
    if (Notification.permission !== "granted") return;
    const n = new Notification(s.title, {
      body: s.status === "failed" ? "Failed" : "Waiting for you",
    });
    n.onclick = () => {
      window.focus();
      go({ name: "sessions", project: s.project, session: s.id });
    };
  } catch {
    // Notifications are a nicety; a refused or broken one must not break the view.
  }
}

let lastBadge: number | null = null;

/** Number of sessions waiting on you, shown on the dock icon. Desktop only. */
export function setDockBadge(count: number): void {
  const t = tauri();
  if (!t) return;
  const next = count > 0 ? count : null;
  // Only send changes: this runs on every session update.
  if (next === lastBadge) return;
  lastBadge = next;
  // Best effort: the badge is decoration, and an older host may not have the command.
  t.invoke("set_badge_count", { count: next }).catch(() => {});
}
