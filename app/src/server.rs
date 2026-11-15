use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use atelier_proto::local;
use serde::Serialize;

/// What the web UI needs to connect (`window.__ATELIER__`).
#[derive(Serialize, Debug, PartialEq)]
pub struct Target {
    url: String,
    /// Not needed on a server's tailnet address from one of its owner's devices.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

/// Connect to a remote server if one is configured: `ATELIER_URL` (+ `ATELIER_TOKEN`), else `<home>/remote.json`
/// (see [`parse_remote`]). Otherwise connect to the local server, starting it if it isn't running.
///
/// A remote target means this machine runs no server at all: no login item, no spawned process. A second, empty
/// local server beside the real one would only confuse where the sessions are.
///
/// Doesn't wait for a freshly started server: the UI shows "connecting" and
/// retries, so the window appears immediately.
pub fn ensure_running() -> Result<Target, Box<dyn std::error::Error>> {
    let home = local::home().ok_or("no home directory")?;
    if let Ok(url) = std::env::var("ATELIER_URL") {
        // `make app` points us at a loopback server without a token: that one is ours, so use our token. Only a
        // remote server gets to go tokenless (it trusts the tailnet).
        let token = match std::env::var("ATELIER_TOKEN").ok() {
            Some(t) => Some(t),
            None if is_loopback(&url) => Some(local::token(&home)?),
            None => None,
        };
        return Ok(Target { url, token });
    }
    if let Ok(text) = std::fs::read_to_string(home.join(REMOTE_FILE)) {
        return parse_remote(&text).map_err(|e| format!("{}: {e}", home.join(REMOTE_FILE).display()).into());
    }
    // Create the token now, so it's known before the server finishes starting.
    let token = local::token(&home)?;
    let addr = local_addr();
    let up = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok();
    let bin = server_bin();
    let mut started = false;
    if launchd::applies(&bin) {
        match launchd::ensure(&bin, &home, up) {
            Ok(()) => started = true,
            Err(e) => note(format!(
                "Couldn't hand the server to launchd ({e}); it was started by the app instead and won't survive a restart or logout."
            )),
        }
    }
    if !started && !up {
        spawn_detached(&home)?;
    }
    Ok(Target { url: format!("ws://{addr}/ws"), token: Some(token) })
}

fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or("");
    let host =
        authority.rsplit_once(':').map_or(authority, |(h, p)| if p.parse::<u16>().is_ok() { h } else { authority });
    matches!(host.trim_matches(['[', ']']), "127.0.0.1" | "localhost" | "::1")
}

/// `<home>/remote.json`: `{"url": "my-vps", "token": "…"}`. Written by hand or by the server's install script;
/// delete it to go back to a local server. `url` is a bare host, `host:port` or a full `ws(s)://` URL; `token` is
/// optional because the server trusts your other tailnet devices.
const REMOTE_FILE: &str = "remote.json";

fn parse_remote(text: &str) -> Result<Target, String> {
    #[derive(serde::Deserialize)]
    struct File {
        url: String,
        token: Option<String>,
    }
    let f: File = serde_json::from_str(text).map_err(|e| format!("expected {{\"url\": …, \"token\": …}}: {e}"))?;
    let token = f.token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    Ok(Target { url: ws_url(&f.url)?, token })
}

/// Mirrors `normalizeServerUrl` in `web/src/lib/server.ts` (port 7747 and path `/ws` by default).
fn ws_url(input: &str) -> Result<String, String> {
    let s = input.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, rest)) => (
            match sc.to_ascii_lowercase().as_str() {
                "ws" | "http" => "ws",
                "wss" | "https" => "wss",
                _ => return Err(format!("unsupported scheme in {s:?}")),
            },
            rest,
        ),
        None => ("ws", s),
    };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    if authority.is_empty() || authority.contains(char::is_whitespace) {
        return Err(format!("{s:?} isn't a server address"));
    }
    // A bare IPv6 literal or host has no port yet; `[v6]:port` and `host:port` do.
    let has_port = authority.rsplit_once(':').is_some_and(|(h, p)| !h.ends_with(':') && p.parse::<u16>().is_ok());
    let authority = if has_port { authority.to_owned() } else { format!("{authority}:{}", local::DEFAULT_PORT) };
    let path = if path.is_empty() { "ws" } else { path };
    Ok(format!("{scheme}://{authority}/{path}"))
}

/// Where the local server listens: `ATELIER_LISTEN` (set by `make app`, so a dev server runs beside the installed
/// one), else the default port. The server spawned below inherits the same variable.
fn local_addr() -> SocketAddr {
    std::env::var("ATELIER_LISTEN")
        .ok()
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], local::DEFAULT_PORT)))
}

/// Whether this app's server is the launchd login item. A dev build's isn't, even while the installed one is loaded.
fn under_launchd() -> bool {
    launchd::applies(&server_bin()) && launchd::loaded()
}

fn spawn_detached(home: &std::path::Path) -> std::io::Result<()> {
    let log = std::fs::OpenOptions::new().create(true).append(true).open(home.join("atelierd.log"))?;
    let mut cmd = Command::new(server_bin());
    cmd.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // Its own process group: it must outlive this window (and not get our ^C).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

/// `ATELIER_SERVER_BIN`, else next to this executable (bundled sidecar, or
/// `target/<profile>/` in development), else `atelierd` on PATH.
pub(crate) fn server_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("ATELIER_SERVER_BIN") {
        return p.into();
    }
    let sibling = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("atelierd")));
    match sibling {
        Some(p) if p.exists() => p,
        _ => "atelierd".into(),
    }
}

/// Why the server isn't under launchd, for the UI. Set once at launch.
static NOTE: Mutex<Option<String>> = Mutex::new(None);

fn note(msg: String) {
    *NOTE.lock().unwrap() = Some(msg);
}

#[derive(Serialize)]
pub struct Status {
    /// "launchd" (starts at login, restarts on crash) or "other" (started by hand or by the app).
    manager: &'static str,
    running: bool,
    /// What `/health` reports, e.g. "atelierd/0.1.0".
    server: Option<String>,
    app_version: &'static str,
    log: String,
    note: Option<String>,
}

#[tauri::command]
pub async fn server_status() -> Result<Status, String> {
    let home = local::home().ok_or("no home directory")?;
    let health = health();
    Ok(Status {
        manager: if under_launchd() { "launchd" } else { "other" },
        running: health.is_some(),
        server: health,
        app_version: APP_VERSION,
        log: home.join("atelierd.log").display().to_string(),
        note: NOTE.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub async fn server_restart() -> Result<(), String> {
    if !under_launchd() {
        return Err("This server wasn't started by launchd; restart it where it runs.".into());
    }
    launchd::kickstart().map_err(|e| e.to_string())
}

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `GET /health` → the `server` field. Plain HTTP/1.0 over a socket: the
/// answer is one small JSON object, which isn't worth an HTTP client.
fn health() -> Option<String> {
    use std::io::{Read, Write};
    let addr = local_addr();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    parse_health(&body)
}

fn parse_health(response: &str) -> Option<String> {
    let json = &response[response.find("\r\n\r\n")? + 4..];
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("server")?.as_str().map(str::to_owned)
}

/// "atelierd/0.10.2" → [0, 10, 2]. Numeric, so 0.10 > 0.9.
fn version_of(server: &str) -> Option<Vec<u64>> {
    server.rsplit('/').next()?.split('.').map(|p| p.parse().ok()).collect()
}

fn older_than_app(server: &str) -> bool {
    match (version_of(server), version_of(APP_VERSION)) {
        (Some(s), Some(a)) => s < a,
        _ => false,
    }
}

/// macOS: run the bundled server as a per-user LaunchAgent, so it starts at
/// login, is restarted if it dies, and outlives the app.
mod launchd {
    use super::*;

    const LABEL: &str = "sh.atelier.server";

    /// Only a bundled app owns a login item. A dev build's `target/` binary
    /// must not be installed as one, nor a binary the person pointed us at.
    pub fn applies(bin: &Path) -> bool {
        cfg!(target_os = "macos")
            && std::env::var_os("ATELIER_SERVER_BIN").is_none()
            && bin.components().any(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
    }

    fn run(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("launchctl").args(args).output()
    }

    fn domain() -> std::io::Result<String> {
        let out = Command::new("id").arg("-u").output()?;
        Ok(format!("gui/{}", String::from_utf8_lossy(&out.stdout).trim()))
    }

    fn check(out: std::io::Result<std::process::Output>, what: &str) -> Result<(), Box<dyn std::error::Error>> {
        let out = out?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("launchctl {what}: {}", String::from_utf8_lossy(&out.stderr).trim()).into())
        }
    }

    pub fn loaded() -> bool {
        cfg!(target_os = "macos")
            && domain().and_then(|d| run(&["print", &format!("{d}/{LABEL}")])).is_ok_and(|o| o.status.success())
    }

    pub fn kickstart() -> Result<(), Box<dyn std::error::Error>> {
        let target = format!("{}/{LABEL}", domain()?);
        check(run(&["kickstart", "-k", &target]), "kickstart")
    }

    fn plist_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let home = std::env::var_os("HOME").ok_or("no $HOME")?;
        Ok(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    /// Install or repair the agent, and restart it if it is out of date.
    pub fn ensure(bin: &Path, home: &Path, up: bool) -> Result<(), Box<dyn std::error::Error>> {
        let path = plist_path()?;
        let want = plist(bin, home, &launchd_env(home));
        let current = std::fs::read_to_string(&path).ok();
        let is_loaded = loaded();
        if current.as_deref() == Some(want.as_str()) && is_loaded {
            if up && health().is_some_and(|s| older_than_app(&s)) {
                kickstart()?;
            }
            return Ok(());
        }
        // A server we didn't start under launchd holds the port; a second one would crash-loop on it.
        if up && !is_loaded {
            return Err(
                "a server that isn't managed by launchd is already running; quit it and relaunch Atelier".into()
            );
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &want)?;
        let domain = domain()?;
        // `bootout` rereads nothing by itself; replacing a loaded job means unloading it first.
        if is_loaded {
            let _ = run(&["bootout", &format!("{domain}/{LABEL}")]);
        }
        check(run(&["bootstrap", &domain, &path.to_string_lossy()]), "bootstrap")
    }

    /// Agents get a bare PATH; the server needs to find `claude`, `git` and
    /// whatever the person's tools put in the usual places. `ATELIER_*`
    /// settings (e.g. `ATELIER_LISTEN_TAILSCALE=1`, which the phone needs) come
    /// from `<home>/env`, since an app started from Finder has no shell
    /// environment to inherit them from.
    fn launchd_env(home: &Path) -> Vec<(String, String)> {
        let user = std::env::var("HOME").unwrap_or_default();
        let mut env = std::collections::BTreeMap::new();
        let from_process = std::env::vars().filter(|(k, _)| forwarded(k));
        let from_file = parse_env(&std::fs::read_to_string(home.join("env")).unwrap_or_default());
        env.extend(from_process);
        env.extend(from_file);
        env.insert(
            "PATH".into(),
            format!("{user}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"),
        );
        env.into_iter().collect()
    }

    /// Server settings only: not the client-side `ATELIER_URL`/`ATELIER_TOKEN`, nor the app's own `ATELIER_SERVER_BIN`.
    fn forwarded(key: &str) -> bool {
        key.starts_with("ATELIER_") && !matches!(key, "ATELIER_URL" | "ATELIER_TOKEN" | "ATELIER_SERVER_BIN")
    }

    /// `KEY=VALUE` lines; blank lines and `#` comments are skipped.
    pub fn parse_env(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|l| {
                let (k, v) = l.trim().split_once('=')?;
                let k = k.trim();
                (forwarded(k) && !k.starts_with('#')).then(|| (k.to_owned(), v.trim().to_owned()))
            })
            .collect()
    }

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    pub fn plist(bin: &Path, home: &Path, env: &[(String, String)]) -> String {
        let vars: String =
            env.iter().map(|(k, v)| format!("    <key>{}</key><string>{}</string>\n", esc(k), esc(v))).collect();
        let log = esc(&home.join("atelierd.log").to_string_lossy());
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>EnvironmentVariables</key>
  <dict>
{vars}  </dict>
</dict>
</plist>
"#,
            esc(&bin.to_string_lossy())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(older_than_app("atelierd/0.0.9"));
        assert!(!older_than_app(&format!("atelierd/{APP_VERSION}")));
        assert!(!older_than_app("atelierd/999.0.0"));
        assert!(!older_than_app("garbage"));
        assert!(version_of("atelierd/0.10.1") > version_of("atelierd/0.9.9"));
    }

    #[test]
    fn loopback_urls_are_ours() {
        assert!(is_loopback("ws://127.0.0.1:7747/ws"));
        assert!(is_loopback("ws://localhost/ws"));
        assert!(is_loopback("ws://[::1]:7747/ws"));
        assert!(!is_loopback("ws://my-vps:7747/ws"));
        assert!(!is_loopback("wss://100.1.2.3/ws"));
    }

    #[test]
    fn remote_file_gives_a_websocket_url() {
        let t = parse_remote(r#"{"url": "my-vps", "token": " abc "}"#).unwrap();
        assert_eq!(t, Target { url: "ws://my-vps:7747/ws".into(), token: Some("abc".into()) });
        let t = parse_remote(r#"{"url": "https://vps.tail1.ts.net/custom", "token": ""}"#).unwrap();
        assert_eq!(t, Target { url: "wss://vps.tail1.ts.net:7747/custom".into(), token: None });
        assert_eq!(ws_url("100.1.2.3:9000").unwrap(), "ws://100.1.2.3:9000/ws");
        assert_eq!(ws_url("[fd7a::1]").unwrap(), "ws://[fd7a::1]:7747/ws");
        assert!(ws_url("ftp://x").is_err());
        assert!(ws_url("").is_err());
        assert!(parse_remote("not json").is_err());
    }

    #[test]
    fn health_body_is_found_after_headers() {
        let r = "HTTP/1.0 200 OK\r\ncontent-type: application/json\r\n\r\n{\"server\":\"atelierd/0.1.0\",\"head\":3}";
        assert_eq!(parse_health(r).as_deref(), Some("atelierd/0.1.0"));
        assert_eq!(parse_health("HTTP/1.0 500\r\n\r\nnope"), None);
    }

    #[test]
    fn plist_escapes_and_points_at_the_bundle() {
        let p = launchd::plist(
            Path::new("/Applications/A&B.app/Contents/MacOS/atelierd"),
            Path::new("/Users/x/.atelier"),
            &[("PATH".into(), "/bin".into())],
        );
        assert!(p.contains("/Applications/A&amp;B.app/Contents/MacOS/atelierd"));
        assert!(p.contains("<key>KeepAlive</key><true/>"));
        assert!(p.contains("/Users/x/.atelier/atelierd.log"));
    }

    #[test]
    fn env_file_carries_server_settings_only() {
        let got = launchd::parse_env(
            "# comment\nATELIER_LISTEN_TAILSCALE=1\n\nFOO=bar\nATELIER_TOKEN=x\n ATELIER_HOME = /h \n",
        );
        assert_eq!(got, vec![("ATELIER_LISTEN_TAILSCALE".into(), "1".into()), ("ATELIER_HOME".into(), "/h".into())]);
    }

    #[test]
    fn only_bundled_apps_get_a_login_item() {
        assert!(!launchd::applies(Path::new("/repo/target/debug/atelierd")) || !cfg!(target_os = "macos"));
        assert!(
            launchd::applies(Path::new("/Applications/Atelier.app/Contents/MacOS/atelierd"))
                == cfg!(target_os = "macos")
        );
    }
}
