// The one connection to atelierd. See crates/atelier-proto for the protocol.
//
// Everything is resumable: subs remember the next seq they need and watches
// re-snapshot, so a reconnect (network blip, laptop sleep, phone backgrounded)
// is invisible to the views built on top.

import { batch, createSignal, type Accessor } from "solid-js";
import type { Call } from "../gen/Call";
import type { ClientMsg } from "../gen/ClientMsg";
import type { Event } from "../gen/Event";
import type { ServerMsg } from "../gen/ServerMsg";
import type { StreamFilter } from "../gen/StreamFilter";
import type { ViewName } from "../gen/ViewName";

export const PROTOCOL = 1;

export type ConnState = "connecting" | "open" | "offline" | "rejected";

export interface ServerConfig {
  url: string;
  token: string;
}

export interface SubHandler {
  /** Persisted events, in seq order. Called for history first, then live. */
  events(events: Event[]): void;
  /** History has been delivered. */
  live?(): void;
  /** Ephemeral events (token deltas). Live only; may be dropped. */
  eph?(kind: string, body: unknown, stream: string): void;
}

export interface WatchHandler<Row> {
  snapshot(rows: Row[]): void;
  patch(upsert: Row[], remove: string[]): void;
}

type Method = Call["method"];
export type Params<M extends Method> = Extract<Call, { method: M }>["params"];

interface Sub {
  filter: StreamFilter;
  next: number;
  h: SubHandler;
}
interface Watch {
  view: ViewName;
  scope: string | undefined;
  h: WatchHandler<unknown>;
}
interface Pending {
  resolve(v: unknown): void;
  reject(e: Error): void;
}

export class Conn {
  readonly state: Accessor<ConnState>;
  readonly error: Accessor<string | null>;
  private setState: (s: ConnState) => void;
  private setError: (e: string | null) => void;

  private ws: WebSocket | null = null;
  private ids = 1;
  private retry = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private subs = new Map<number, Sub>();
  private watches = new Map<number, Watch>();
  private calls = new Map<number, Pending>();

  constructor(private cfg: ServerConfig) {
    const [state, setState] = createSignal<ConnState>("connecting");
    const [error, setError] = createSignal<string | null>(null);
    this.state = state;
    this.error = error;
    this.setState = setState;
    this.setError = setError;
    this.open();
    // Mobile webviews suspend sockets in the background; reconnect promptly on return.
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible" && this.state() === "offline") this.reconnectNow();
    });
  }

  /** Raw events of a stream (or prefix) from `from`, then live. */
  subscribe(filter: StreamFilter, h: SubHandler, from = 1): () => void {
    const id = this.ids++;
    this.subs.set(id, { filter, next: from, h });
    this.sendIfOpen({ t: "sub", id, filter, from });
    return () => {
      this.subs.delete(id);
      this.sendIfOpen({ t: "unsub", id });
    };
  }

  /** A server view: snapshot, then patches. */
  watch<Row>(view: ViewName, scope: string | null, h: WatchHandler<Row>): () => void {
    const id = this.ids++;
    const w = { view, scope: scope ?? undefined, h: h as WatchHandler<unknown> };
    this.watches.set(id, w);
    this.sendIfOpen({ t: "watch", id, view, scope: w.scope });
    return () => {
      this.watches.delete(id);
      this.sendIfOpen({ t: "unwatch", id });
    };
  }

  /** Run a command. Rejects if offline: commands are never queued, so they
   *  can't fire unexpectedly later. */
  call<M extends Method>(method: M, params: Params<M>): Promise<unknown> {
    if (this.state() !== "open") return Promise.reject(new Error("Not connected to the server"));
    const id = this.ids++;
    return new Promise((resolve, reject) => {
      this.calls.set(id, { resolve, reject });
      this.send({ t: "rpc", id, call: { method, params } as Call });
    });
  }

  private open() {
    clearTimeout(this.timer);
    this.setState("connecting");
    const ws = new WebSocket(this.cfg.url);
    this.ws = ws;
    ws.onopen = () => {
      ws.send(JSON.stringify({ t: "hello", protocol: PROTOCOL, client: navigator.userAgent, token: this.cfg.token } satisfies ClientMsg));
    };
    ws.onmessage = (e) => batch(() => this.receive(JSON.parse(e.data as string) as ServerMsg));
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      for (const p of this.calls.values()) p.reject(new Error("Connection lost"));
      this.calls.clear();
      if (this.state() === "rejected") return;
      this.setState("offline");
      // 250ms, 500ms, … capped at 5s, with jitter so devices don't stampede.
      const delay = Math.min(5000, 250 * 2 ** this.retry++) * (0.75 + Math.random() * 0.5);
      this.timer = setTimeout(() => this.open(), delay);
    };
  }

  private reconnectNow() {
    this.retry = 0;
    this.open();
  }

  private receive(m: ServerMsg) {
    switch (m.t) {
      case "welcome":
        this.retry = 0;
        this.setError(null);
        this.setState("open");
        for (const [id, s] of this.subs) this.send({ t: "sub", id, filter: s.filter, from: s.next });
        for (const [id, w] of this.watches) this.send({ t: "watch", id, view: w.view, scope: w.scope });
        break;
      case "events": {
        const s = this.subs.get(m.sub);
        const last = m.events[m.events.length - 1];
        if (!s || !last) break;
        s.next = last.seq + 1;
        s.h.events(m.events);
        break;
      }
      case "live":
        this.subs.get(m.sub)?.h.live?.();
        break;
      case "eph":
        this.subs.get(m.sub)?.h.eph?.(m.kind, m.body, m.stream);
        break;
      case "snapshot":
        this.watches.get(m.watch)?.h.snapshot(m.rows);
        break;
      case "patch":
        this.watches.get(m.watch)?.h.patch(m.upsert, m.remove);
        break;
      case "result":
      case "error": {
        const p = this.calls.get(m.id);
        this.calls.delete(m.id);
        if (m.t === "result") p?.resolve(m.ok);
        else p?.reject(new Error(m.error));
        break;
      }
      case "fatal":
        this.setError(m.error);
        this.setState("rejected");
        this.ws?.close();
        break;
    }
  }

  private send(m: ClientMsg) {
    this.ws?.send(JSON.stringify(m));
  }

  private sendIfOpen(m: ClientMsg) {
    if (this.state() === "open") this.send(m);
  }
}

declare const __ATELIER_DEV__: ServerConfig | null;
declare global {
  interface Window {
    /** Injected by the desktop/mobile host before the page loads. */
    __ATELIER__?: ServerConfig;
  }
}

/** Where to connect: host-injected, then dev-server-injected, then saved. */
export function serverConfig(): ServerConfig | null {
  if (window.__ATELIER__) return window.__ATELIER__;
  if (__ATELIER_DEV__?.token) return __ATELIER_DEV__;
  try {
    const saved = localStorage.getItem("atelier.server");
    if (saved) return JSON.parse(saved) as ServerConfig;
  } catch {
    // storage unavailable
  }
  return null;
}
