import { createSignal, For, Show } from "solid-js";
import { canOpenIn } from "../lib/openin";
import { LocalServer } from "./LocalServer";
import { hostServer, serverConfig, type Conn } from "../lib/conn";
import { normalizeServerUrl, probe, savedServer, saveServer } from "../lib/server";
import "../styles/settings.css";

export function connLabel(conn: Conn): string {
  return { open: "Connected", connecting: "Connecting…", offline: "Reconnecting…", rejected: conn.error() ?? "Rejected" }[
    conn.state()
  ];
}

/** Environment variables that configure the server (`atelierd`), set where it
 *  runs. Listed by hand: whoever adds one adds a line here. */
const SERVER_ENV: { name: string; default: string; doc: string }[] = [
  { name: "ATELIER_HOME", default: "~/.atelier", doc: "Data directory: database, token, worktrees." },
  { name: "ATELIER_LISTEN", default: "127.0.0.1:7747", doc: "Address the server listens on." },
  { name: "ATELIER_LISTEN_TAILSCALE", default: "off", doc: "Also listen on the tailnet address (1 to enable)." },
  { name: "ATELIER_TAILSCALE_BIN", default: "usual locations, then PATH", doc: "The tailscale executable." },
  { name: "ATELIER_CLAUDE_BIN", default: "claude, from PATH", doc: "The Claude Code executable." },
  { name: "ATELIER_IDLE_SECS", default: "600", doc: "Idle seconds before an agent process is stopped." },
  { name: "ATELIER_AUTO_TITLE", default: "on", doc: "Title sessions after their first turn (0 to disable)." },
];

/** Which server to talk to. With `conn`, also shows how that connection is
 *  doing. Saving probes the address first, so a typo or a refused token is
 *  reported here rather than as a dead app after the reload. */
export function Settings(props: { conn?: Conn; onClose?: () => void }) {
  const host = hostServer();
  const saved = savedServer();
  const current = serverConfig();
  const overriding = !!(host && saved?.override);

  const [address, setAddress] = createSignal(current?.url ?? "");
  // A host-injected token stays out of the form; only one typed here is shown.
  const [token, setToken] = createSignal(saved?.token ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const url = () => normalizeServerUrl(address());

  async function connect(e: Event) {
    e.preventDefault();
    const u = url();
    if (!u || busy()) return;
    setBusy(true);
    setError(null);
    const cfg = { url: u, token: token().trim() || undefined };
    const problem = await probe(cfg);
    if (problem) {
      setError(problem);
      setBusy(false);
      return;
    }
    saveServer(host ? { ...cfg, override: true } : cfg);
    location.reload();
  }

  function useHost() {
    saveServer(null);
    location.reload();
  }

  const rejection = () => (props.conn?.state() === "rejected" ? props.conn.error() : null);

  return (
    <form class="settings" onSubmit={connect}>
      <h3>Server</h3>
      <Show when={props.conn && current}>
        {(cur) => (
          <p class="settings-now">
            <i class={`conn-dot ${props.conn!.state()}`} />
            <span>{props.conn!.state() === "rejected" ? "Refused" : connLabel(props.conn!)}</span>
            <code>{cur().url}</code>
          </p>
        )}
      </Show>
      <Show when={props.conn && current && canOpenIn(current.url)}>
        <LocalServer />
      </Show>
      <Show when={host && !overriding}>
        <p class="faint">Provided by the desktop app. Connecting elsewhere below overrides it on this device.</p>
      </Show>

      <label>
        <span class="cap">Address</span>
        <input
          class="field"
          placeholder="serenity, 100.64.0.1, or ws://host:7747/ws"
          autocapitalize="off"
          autocorrect="off"
          spellcheck={false}
          value={address()}
          onInput={(e) => setAddress(e.currentTarget.value)}
        />
      </label>
      <Show
        when={url()}
        fallback={
          <Show when={address().trim()}>
            <p class="err">Not an address. Try a host name, an IP, host:port, or a ws:// URL.</p>
          </Show>
        }
      >
        {(u) => (
          <Show when={u() !== address().trim()}>
            <p class="faint">
              Connects to <code>{u()}</code>
            </p>
          </Show>
        )}
      </Show>

      <label>
        <span class="cap">Token</span>
        <input
          class="field"
          type="password"
          placeholder="Optional on your tailnet"
          autocomplete="off"
          value={token()}
          onInput={(e) => setToken(e.currentTarget.value)}
        />
      </label>
      <p class="faint">
        From your own devices over Tailscale, none is needed. Otherwise it is in <code>~/.atelier/token</code> on the
        server.
      </p>

      <Show when={props.conn}>
        <details class="settings-env">
          <summary>Server environment</summary>
          <p class="faint">Set these where the server runs, then restart it.</p>
          <dl>
            <For each={SERVER_ENV}>
              {(v) => (
                <>
                  <dt>
                    <code>{v.name}</code>
                  </dt>
                  <dd>
                    {v.doc} <span class="faint">Default: {v.default}.</span>
                  </dd>
                </>
              )}
            </For>
          </dl>
        </details>
      </Show>

      <Show when={error() ?? rejection()}>
        <p class="err">{error() ?? rejection()}</p>
      </Show>

      <div class="settings-actions">
        <Show when={overriding}>
          <button type="button" class="btn quiet" onClick={useHost}>
            Use the desktop app’s server
          </button>
        </Show>
        <span class="grow" />
        <Show when={props.onClose}>
          <button type="button" class="btn quiet" onClick={() => props.onClose?.()}>
            Close
          </button>
        </Show>
        <button class="btn dark" disabled={busy() || !url()}>
          {busy() ? "Connecting…" : "Connect"}
        </button>
      </div>
    </form>
  );
}

/** Shown when there is no server to talk to yet. */
export function Unconfigured() {
  return (
    <div class="empty" style={{ height: "100%" }}>
      <div class="illum" />
      <h2>Connect to a server</h2>
      <p>
        Where is <code>atelierd</code> running? On another machine, start it with{" "}
        <code>ATELIER_LISTEN_TAILSCALE=1</code> and give its tailnet name or address.
      </p>
      <Settings />
    </div>
  );
}
