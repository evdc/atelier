import { createSignal } from "solid-js";

// One shared clock for every relative timestamp on screen: a single timer,
// ticking once a minute, instead of one per row.
const [now, setNow] = createSignal(Date.now());
setInterval(() => setNow(Date.now()), 60_000);

/** "now", "4m", "2h", "Tue", "Oct 30". Reactive: re-evaluates each minute. */
export function ago(ts: number): string {
  const s = Math.max(0, (now() - ts) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  const d = new Date(ts);
  if (s < 6 * 86400) return d.toLocaleDateString(undefined, { weekday: "short" });
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function usd(n: number): string {
  return n < 0.01 && n > 0 ? "<$0.01" : `$${n.toFixed(2)}`;
}

/** "96k tokens", "1.2M tokens", "740 tokens". */
export function tokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M tokens`;
  if (n >= 1000) return `${Math.round(n / 1000)}k tokens`;
  return `${n} tokens`;
}

/** When a limit window resets: "3:40 PM" today, "Tue 3:40 PM" otherwise. */
export function resetsAt(ms: number): string {
  const d = new Date(ms);
  const today = d.toDateString() === new Date().toDateString();
  return d.toLocaleString(undefined, {
    weekday: today ? undefined : "short",
    hour: "numeric",
    minute: "2-digit",
  });
}
