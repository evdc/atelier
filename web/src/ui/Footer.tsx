import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import type { LimitWindow } from "../gen/LimitWindow";
import { resetsAt } from "../lib/time";
import { useApp } from "./context";
import { connLabel, Settings } from "./Settings";

/** Utilization from which a limit turns red: it needs attention, not a glance. */
const NEAR_LIMIT = 0.8;

/** The one bar across the bottom of the app: subscription limits, connection.
 *  The connection opens the server settings, which also open by themselves
 *  when the server refuses us: nothing else works until that is fixed. */
export function Footer() {
  const { conn, usage } = useApp();
  const limits = () => usage.get("global");
  const [settings, setSettings] = createSignal(false);
  createEffect(on(conn.state, (s) => s === "rejected" && setSettings(true)));
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSettings(false);
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));

  return (
    <footer class="foot" title={connLabel(conn)}>
      <div class="limits">
        <Limit label="5h" w={limits()?.five_hour ?? null} />
        <Limit label="7d" w={limits()?.seven_day ?? null} />
      </div>
      <div class="foot-r">
        <span class="conn">
          <i class={`conn-dot ${conn.state()}`} />
          <span>{connLabel(conn)}</span>
        </span>
        <button
          type="button"
          class="gear"
          title="Settings"
          aria-label="Settings"
          onClick={() => setSettings((v) => !v)}
          aria-expanded={settings()}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.1">
            <circle cx="8" cy="8" r="2.2" />
            <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4" />
          </svg>
        </button>
      </div>
      <Show when={settings()}>
        <div class="sheet-back" onClick={() => setSettings(false)} />
        <div class="sheet" role="dialog" aria-label="Settings">
          <Settings conn={conn} onClose={() => setSettings(false)} />
        </div>
      </Show>
    </footer>
  );
}

/** One hairline meter with its percentage. The track is always drawn so the
 *  place is findable; the fill, the number and the reset time appear only once
 *  the vendor has reported a utilization. Nothing is guessed. */
function Limit(props: { label: string; w: LimitWindow | null }) {
  const known = () => props.w?.utilization != null;
  const used = () => Math.min(1, props.w?.utilization ?? 0);
  const title = () => {
    if (!known()) return `${props.label}: not reported yet`;
    return props.w?.resets_at ? `Resets ${resetsAt(props.w.resets_at)}` : undefined;
  };
  return (
    <div class="limit" title={title()}>
      <span>{props.label}</span>
      <span class="bar" classList={{ near: used() >= NEAR_LIMIT }}>
        <Show when={known()}>
          <i style={{ width: `${Math.round(used() * 100)}%` }} />
        </Show>
      </span>
      <span class="pct">
        <Show when={known()}>{`${Math.round((props.w?.utilization ?? 0) * 100)}%`}</Show>
      </span>
    </div>
  );
}
