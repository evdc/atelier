// "Open in…": hand the worktree to the person's own terminal/editor/Finder.
// Only possible in the desktop app (a Tauri command) and only meaningful when
// the server is on this machine, since the path is a server path.

export type OpenKind = "terminal" | "editor" | "finder";

interface TauriInternals {
  invoke(cmd: string, args: Record<string, unknown>): Promise<unknown>;
}

const tauri = (): TauriInternals | undefined =>
  (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__;

export function isLoopback(url: string): boolean {
  try {
    const h = new URL(url).hostname;
    return h === "localhost" || h === "127.0.0.1" || h === "::1" || h === "[::1]";
  } catch {
    return false;
  }
}

export function canOpenIn(serverUrl: string): boolean {
  return !!tauri() && isLoopback(serverUrl);
}

export function openIn(kind: OpenKind, path: string): Promise<unknown> {
  const t = tauri();
  return t ? t.invoke("open_path", { kind, path }) : Promise.reject(new Error("not available here"));
}
