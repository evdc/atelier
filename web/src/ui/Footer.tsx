import { Show } from "solid-js";
import type { LimitWindow } from "../gen/LimitWindow";
import { resetsAt } from "../lib/time";
import { useApp } from "./context";

/** Utilization from which a limit turns red: it needs attention, not a glance. */
const NEAR_LIMIT = 0.8;

/** The one bar across the bottom of the app: subscription limits, connection. */
export function Footer() {
  const { conn, usage } = useApp();
  const limits = () => usage.get("global");

  const connLabel = () =>
    ({ open: "Connected", connecting: "Connecting…", offline: "Reconnecting…", rejected: conn.error() ?? "Rejected" })[
      conn.state()
    ];

  return (
    <footer class="foot" title={connLabel()}>
      <div class="limits">
        <Limit label="5h" w={limits()?.five_hour ?? null} />
        <Limit label="7d" w={limits()?.seven_day ?? null} />
      </div>
      <div class="conn">
        <i class={`conn-dot ${conn.state()}`} />
        <span>{connLabel()}</span>
      </div>
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
