//! Reaching atelierd from other devices over Tailscale.
//!
//! With `ATELIER_LISTEN_TAILSCALE=1` the server also listens on this node's
//! tailnet address. A connection arriving there may skip the token if
//! Tailscale vouches that the peer device belongs to the same person as this
//! one (`tailscale whois`). Tailscale has already authenticated the device;
//! asking it again is cheaper and safer than shipping secrets to phones.
//! Loopback connections always need the token.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures::future::BoxFuture;
use serde::Deserialize;

/// Decides whether a peer on the tailnet listener is trusted without a token.
/// Injectable so tests can stand in for the `tailscale` CLI.
pub trait PeerAuth: Send + Sync {
    /// Must fail closed: any doubt (CLI missing, error, odd output) is `false`.
    fn trusted(&self, ip: IpAddr) -> BoxFuture<'_, bool>;
}

/// How long a whois verdict is reused for one peer address.
const CACHE_FOR: Duration = Duration::from_secs(60);
const CLI_TIMEOUT: Duration = Duration::from_secs(5);

/// The local `tailscale` CLI.
pub struct Tailscale {
    bin: PathBuf,
    cache: Mutex<HashMap<IpAddr, (Instant, bool)>>,
}

impl Tailscale {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin, cache: Mutex::default() }
    }

    /// `ATELIER_TAILSCALE_BIN`, else the first of the usual install locations
    /// that exists, else `tailscale` from PATH.
    pub fn find_bin() -> PathBuf {
        if let Some(b) = std::env::var_os("ATELIER_TAILSCALE_BIN") {
            return b.into();
        }
        [
            "/usr/local/bin/tailscale",
            "/opt/homebrew/bin/tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .map_or_else(|| "tailscale".into(), Path::to_path_buf)
    }

    /// This node's tailnet IPv4 address (`tailscale ip -4`).
    pub async fn ipv4(&self) -> Result<Ipv4Addr> {
        let out = self.run(&["ip", "-4"]).await?;
        let text = String::from_utf8_lossy(&out);
        text.lines()
            .next()
            .unwrap_or("")
            .trim()
            .parse()
            .with_context(|| format!("unexpected `tailscale ip -4`: {text}"))
    }

    async fn check(&self, ip: IpAddr) -> Result<bool> {
        let status = self.run(&["status", "--json", "--peers=false"]).await?;
        let whois = self.run(&["whois", "--json", &ip.to_string()]).await?;
        same_user(&status, &whois)
    }

    async fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let child = tokio::process::Command::new(&self.bin).args(args).kill_on_drop(true).output();
        let out = tokio::time::timeout(CLI_TIMEOUT, child)
            .await
            .context("tailscale timed out")?
            .with_context(|| format!("running {}", self.bin.display()))?;
        if !out.status.success() {
            bail!("tailscale {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout)
    }
}

impl PeerAuth for Tailscale {
    fn trusted(&self, ip: IpAddr) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let cached = self.cache.lock().unwrap().get(&ip).filter(|(at, _)| at.elapsed() < CACHE_FOR).map(|c| c.1);
            if let Some(v) = cached {
                return v;
            }
            match self.check(ip).await {
                Ok(v) => {
                    let mut cache = self.cache.lock().unwrap();
                    cache.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
                    cache.insert(ip, (Instant::now(), v));
                    v
                }
                // Not cached: a transient CLI failure shouldn't lock a device out for a minute.
                Err(e) => {
                    tracing::warn!("tailscale whois {ip} failed; refusing: {e:#}");
                    false
                }
            }
        })
    }
}

#[derive(Deserialize)]
struct Status {
    #[serde(rename = "Self")]
    me: Node,
}

#[derive(Deserialize)]
struct Whois {
    #[serde(rename = "Node")]
    node: Node,
}

/// The fields we need of a node, in both `status` (`Self`) and `whois` (`Node`),
/// which name the owner differently.
#[derive(Deserialize)]
struct Node {
    #[serde(alias = "UserID", alias = "User")]
    user: i64,
    /// Tagged nodes are owned by the tailnet, not a person; their user id is a
    /// shared placeholder, so it proves nothing.
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

impl Node {
    fn personal(&self) -> bool {
        self.tags.as_ref().is_none_or(Vec::is_empty) && self.user != 0
    }
}

/// Whether the peer described by `tailscale whois --json` belongs to the same
/// person as this node (`tailscale status --json`).
pub fn same_user(status: &[u8], whois: &[u8]) -> Result<bool> {
    let me = serde_json::from_slice::<Status>(status).context("parsing tailscale status")?.me;
    let peer = serde_json::from_slice::<Whois>(whois).context("parsing tailscale whois")?.node;
    Ok(me.personal() && peer.personal() && me.user == peer.user)
}

/// Browsers send `Origin` on WebSocket upgrades, and any web page can open a
/// socket to a tailnet address. Without this, a page visited on a trusted
/// phone could drive the server with the phone's identity. Only our own
/// clients' origins (the Tauri webviews, a local dev server) or no origin at
/// all (native clients) may use token-less auth.
pub fn origin_ok(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return true };
    if matches!(o, "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost") {
        return true;
    }
    let Some(rest) = o.strip_prefix("http://") else { return false };
    let host = rest.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or(rest, |(h, _)| h);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `tailscale status --json --peers=false` / `whois --json`
    // (1.102), trimmed and anonymized.
    const STATUS: &str = r#"{
      "Version": "1.102.1", "BackendState": "Running",
      "TailscaleIPs": ["100.73.45.61", "fd7a:115c:a1e0::4801:2d98"],
      "Self": {
        "ID": "ndGR6Hd8NH11CNTRL", "NodeID": 2095667266819442, "HostName": "desk",
        "DNSName": "desk.tail0000.ts.net.", "OS": "macOS", "UserID": 6547622781339430,
        "TailscaleIPs": ["100.73.45.61"], "Online": true,
        "CapMap": {"https": null, "tailnet-display-name": ["me@example.com"]}
      },
      "User": {"6547622781339430": {"ID": 6547622781339430, "LoginName": "me@example.com", "DisplayName": "Me"}},
      "CurrentTailnet": {"Name": "me@example.com", "MagicDNSSuffix": "tail0000.ts.net", "MagicDNSEnabled": true}
    }"#;

    fn whois(user: i64, tags: &str) -> String {
        format!(
            r#"{{
              "Node": {{
                "ID": 8137402413082457, "StableID": "nxi52FmSY621CNTRL", "Name": "phone.tail0000.ts.net.",
                "User": {user}, {tags}
                "Addresses": ["100.79.251.30/32"], "MachineAuthorized": true,
                "Hostinfo": {{"Hostname": "phone"}}, "ComputedName": "phone"
              }},
              "UserProfile": {{"ID": {user}, "LoginName": "someone@example.com", "DisplayName": "Someone"}},
              "CapMap": null
            }}"#
        )
    }

    #[test]
    fn same_person_is_trusted() {
        assert!(same_user(STATUS.as_bytes(), whois(6547622781339430, "").as_bytes()).unwrap());
    }

    #[test]
    fn someone_else_is_not() {
        assert!(!same_user(STATUS.as_bytes(), whois(42, "").as_bytes()).unwrap());
    }

    #[test]
    fn tagged_nodes_are_not() {
        let tagged = whois(6547622781339430, r#""Tags": ["tag:server"],"#);
        assert!(!same_user(STATUS.as_bytes(), tagged.as_bytes()).unwrap());
        let tagged_me = STATUS.replace(r#""OS": "macOS","#, r#""OS": "linux", "Tags": ["tag:server"],"#);
        assert!(!same_user(tagged_me.as_bytes(), whois(6547622781339430, "").as_bytes()).unwrap());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(same_user(b"{}", whois(1, "").as_bytes()).is_err());
        assert!(same_user(STATUS.as_bytes(), b"peer not found").is_err());
    }

    #[test]
    fn origins() {
        for ok in [None, Some("tauri://localhost"), Some("http://tauri.localhost"), Some("http://localhost:5173")] {
            assert!(origin_ok(ok), "{ok:?}");
        }
        for bad in ["https://evil.example", "http://localhost.evil.example", "http://100.73.45.61:7747", "null"] {
            assert!(!origin_ok(Some(bad)), "{bad}");
        }
    }
}
