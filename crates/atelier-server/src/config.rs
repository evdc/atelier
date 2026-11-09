use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub use atelier_proto::local::DEFAULT_PORT;

#[derive(Debug, Clone)]
pub struct Config {
    /// Data directory: database, token, worktrees. `ATELIER_HOME`, default `~/.atelier`.
    pub home: PathBuf,
    /// `ATELIER_LISTEN` / `--listen`, default `127.0.0.1:7747`.
    pub listen: SocketAddr,
    /// `ATELIER_LISTEN_TAILSCALE=1`: also listen on this node's tailnet IPv4
    /// address (same port as `listen`). Never 0.0.0.0.
    pub listen_tailscale: bool,
    /// `ATELIER_TAILSCALE_BIN`, default: the usual install locations, then PATH.
    pub tailscale_bin: PathBuf,
    /// `ATELIER_CLAUDE_BIN`, default `claude` (from PATH).
    pub claude_bin: PathBuf,
    /// Shared secret clients present in `hello`. Generated on first run and
    /// stored in `<home>/token`. `None` disables auth (tests only).
    pub token: Option<String>,
}

impl Config {
    pub fn from_env(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut home: Option<PathBuf> = None;
        let mut listen = std::env::var("ATELIER_LISTEN").ok();
        let mut args = args.skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--home" => home = args.next().map(PathBuf::from),
                "--listen" => listen = args.next(),
                "-h" | "--help" => {
                    println!("atelierd [--home DIR] [--listen ADDR:PORT]");
                    println!("  ATELIER_LISTEN_TAILSCALE=1  also listen on this node's tailnet address");
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument {other}"),
            }
        }
        let home = home.or_else(atelier_proto::local::home).context("no home directory")?;
        std::fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
        let listen =
            listen.unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_PORT}")).parse().context("bad listen address")?;
        let claude_bin = std::env::var_os("ATELIER_CLAUDE_BIN").map(PathBuf::from).unwrap_or_else(|| "claude".into());
        let listen_tailscale =
            std::env::var("ATELIER_LISTEN_TAILSCALE").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let tailscale_bin = crate::tailnet::Tailscale::find_bin();
        let token = Some(atelier_proto::local::token(&home)?);
        Ok(Self { home, listen, listen_tailscale, tailscale_bin, claude_bin, token })
    }

    pub fn db_path(&self) -> PathBuf {
        self.home.join("atelier.db")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.home.join("worktrees")
    }

    pub fn token_ok(&self, presented: Option<&str>) -> bool {
        match (&self.token, presented) {
            (None, _) => true,
            (Some(t), Some(p)) => constant_time_eq(t.as_bytes(), p.as_bytes()),
            (Some(_), None) => false,
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
