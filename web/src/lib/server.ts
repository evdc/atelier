// Choosing a server: address normalization, a one-off connection probe, and
// the saved choice in localStorage. `serverConfig()` in conn.ts reads it.

import type { ClientMsg } from "../gen/ClientMsg";
import type { ServerMsg } from "../gen/ServerMsg";
import { PROTOCOL, type ServerConfig } from "./conn";

export const DEFAULT_PORT = 7747;
const KEY = "atelier.server";

/** `host`, `host:port`, `[v6]:port` or a full `ws(s)://…` / `http(s)://…` URL
 *  → a WebSocket URL, defaulting to port 7747 and path `/ws`. */
export function normalizeServerUrl(input: string): string | null {
  let s = input.trim();
  let scheme = "ws";
  const m = /^(wss?|https?):\/\//i.exec(s);
  if (m) {
    scheme = m[1]!.toLowerCase().replace("http", "ws");
    s = s.slice(m[0].length);
  }
  const slash = s.indexOf("/");
  const authority = slash < 0 ? s : s.slice(0, slash);
  const path = slash < 0 || s.slice(slash) === "/" ? "/ws" : s.slice(slash);
  const host = "(\\[[0-9a-fA-F:.]+\\]|[^\\s:/@\\[\\]]+)";
  let hostPort: string;
  if (new RegExp(`^${host}:\\d{1,5}$`).test(authority)) hostPort = authority;
  else if (new RegExp(`^${host}$`).test(authority)) hostPort = `${authority}:${DEFAULT_PORT}`;
  else return null;
  try {
    return new URL(`${scheme}://${hostPort}${path}`).href;
  } catch {
    return null;
  }
}

/** The choice saved on this device. `override` means it wins over a server the
 *  desktop host injected. */
export interface SavedServer extends ServerConfig {
  override?: boolean;
}

export function savedServer(): SavedServer | null {
  try {
    const saved = localStorage.getItem(KEY);
    return saved ? (JSON.parse(saved) as SavedServer) : null;
  } catch {
    return null;
  }
}

export function saveServer(cfg: SavedServer | null) {
  try {
    if (cfg) localStorage.setItem(KEY, JSON.stringify(cfg));
    else localStorage.removeItem(KEY);
  } catch {
    // storage unavailable: nothing to remember the choice in
  }
}

/** Open a throwaway connection and say hello. Resolves to null if the server
 *  welcomes us, else to a reason a person can act on. */
export function probe(cfg: ServerConfig, timeoutMs = 6000): Promise<string | null> {
  return new Promise((resolve) => {
    let ws: WebSocket;
    try {
      ws = new WebSocket(cfg.url);
    } catch {
      resolve(`${cfg.url} isn't a valid address`);
      return;
    }
    const done = (r: string | null) => {
      clearTimeout(timer);
      ws.onclose = null;
      ws.close();
      resolve(r);
    };
    const timer = setTimeout(() => done(`No answer from ${cfg.url}`), timeoutMs);
    ws.onopen = () => {
      const hello: ClientMsg = { t: "hello", protocol: PROTOCOL, client: "atelier settings", token: cfg.token || undefined };
      ws.send(JSON.stringify(hello));
    };
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data as string) as ServerMsg;
      if (m.t === "welcome") done(null);
      else if (m.t === "fatal") done(m.error);
    };
    ws.onclose = () => done(`Can't reach ${cfg.url}. Is atelierd running and listening on that address?`);
  });
}
