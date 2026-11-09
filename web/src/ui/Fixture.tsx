// Dev-only host for `?fixture=<n>`: a transcript with no server behind it.
import { createSignal } from "solid-js";
import { fixtureLive, fixtureSession } from "../agents/fixtures";
import { TranscriptView } from "./Transcript";

export function Fixture(props: { n: number }) {
  const t0 = performance.now();
  const live = fixtureLive(props.n);
  const [stat, setStat] = createSignal("");
  const session = () => fixtureSession();
  // Two frames after mount ≈ first paint; logged for the ticket's numbers.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => setStat(`${props.n} items, mount→paint ${Math.round(performance.now() - t0)} ms`)),
  );
  return (
    <div class="app" style={{ display: "flex", "flex-direction": "column", height: "100vh" }}>
      <div style={{ font: "12px var(--f-mono)", padding: "4px 12px" }}>{stat()}</div>
      <TranscriptView live={live} session={session} />
    </div>
  );
}
