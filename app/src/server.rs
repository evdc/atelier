use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use atelier_proto::local;
use serde::Serialize;

/// What the web UI needs to connect (`window.__ATELIER__`).
#[derive(Serialize)]
pub struct Target {
    url: String,
    token: String,
}

/// Connect to `ATELIER_URL` + `ATELIER_TOKEN` if set (a remote server);
/// otherwise to the local server, starting it if it isn't running.
///
/// Doesn't wait for a freshly started server: the UI shows "connecting" and
/// retries, so the window appears immediately.
pub fn ensure_running() -> Result<Target, Box<dyn std::error::Error>> {
    if let (Ok(url), Ok(token)) = (std::env::var("ATELIER_URL"), std::env::var("ATELIER_TOKEN")) {
        return Ok(Target { url, token });
    }
    let home = local::home().ok_or("no home directory")?;
    // Create the token now, so it's known before the server finishes starting.
    let token = local::token(&home)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], local::DEFAULT_PORT));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_err() {
        spawn_detached(&home)?;
    }
    Ok(Target { url: format!("ws://{addr}/ws"), token })
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
fn server_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("ATELIER_SERVER_BIN") {
        return p.into();
    }
    let sibling = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("atelierd")));
    match sibling {
        Some(p) if p.exists() => p,
        _ => "atelierd".into(),
    }
}
