import { createResource, Show } from "solid-js";
import { localServerStatus, restartLocalServer } from "../lib/openin";

/** The server this app started on this machine: whether it is running under
 *  launchd (so it survives quitting and logging out), its version and log, and
 *  a restart. Only rendered in the desktop app, for a local server. */
export function LocalServer() {
  const [status, { refetch }] = createResource(localServerStatus);
  let busy = false;
  async function restart() {
    if (busy) return;
    busy = true;
    try {
      await restartLocalServer();
    } catch (e) {
      alert(String(e));
    }
    // Give it a moment to come back before asking again.
    setTimeout(() => {
      busy = false;
      refetch();
    }, 1500);
  }
  return (
    <Show when={status()}>
      {(s) => (
        <div class="settings-local">
          <p>
            {s().running ? (s().server ?? "running") : "Not running"}
            {s().manager === "launchd" ? " · starts at login" : " · not a login item"}
            <Show when={s().server && s().server !== `atelierd/${s().app_version}`}>
              {` · app is ${s().app_version}`}
            </Show>
          </p>
          <p class="faint">
            Log <code>{s().log}</code>
          </p>
          <Show when={s().note}>
            <p class="err">{s().note}</p>
          </Show>
          <Show when={s().manager === "launchd"}>
            <button type="button" class="btn quiet" onClick={restart}>
              Restart server
            </button>
          </Show>
        </div>
      )}
    </Show>
  );
}
