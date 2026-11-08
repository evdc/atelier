// A session's transcript as a reactive store, fed by a stream subscription.

import { createSignal, onCleanup, type Accessor } from "solid-js";
import { createStore, produce } from "solid-js/store";
import type { Conn } from "../lib/conn";
import { fold, streamDelta } from "./fold";
import { emptyTranscript, type Transcript } from "./model";

export interface LiveTranscript {
  t: Transcript;
  /** Text streaming in for the block being written right now. */
  streaming: Accessor<string>;
  /** History has loaded. */
  ready: Accessor<boolean>;
}

/** Must be called inside a reactive owner. */
export function createTranscript(conn: Conn, stream: string): LiveTranscript {
  const [t, setT] = createStore<Transcript>(emptyTranscript());
  const [ready, setReady] = createSignal(false);
  const [streaming, setStreaming] = createSignal("");

  // Deltas arrive tens of times a second; coalesce them into one update per
  // frame instead of one per token.
  let pending = "";
  let frame = 0;
  const flush = () => {
    frame = 0;
    setStreaming((s) => s + pending);
    pending = "";
  };

  const stop = conn.subscribe(
    { stream },
    {
      events(events) {
        setT(produce((d) => events.forEach((e) => fold(d, e))));
        // A persisted message supersedes whatever was streaming for it.
        if (events.some((e) => e.actor.startsWith("agent:") || e.kind === "turn.ended")) {
          pending = "";
          setStreaming("");
        }
      },
      live: () => setReady(true),
      eph(kind, body) {
        const d = streamDelta(kind, body);
        if (d.reset) {
          pending = "";
          setStreaming("");
        }
        if (d.text) {
          pending += d.text;
          frame ||= requestAnimationFrame(flush);
        }
      },
    },
  );
  onCleanup(() => {
    stop();
    cancelAnimationFrame(frame);
  });

  return { t, streaming, ready };
}
