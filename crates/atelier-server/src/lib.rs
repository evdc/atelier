//! atelierd: owns the event log, the agent processes and client sync.
//!
//! ```text
//!   rpc ──▶ handlers ──▶ store.append ──▶ projections (same tx)
//!                │                            │
//!                ▼                            ▼ Commit feed
//!           supervisor ◀── driver events   sync: subs (events) + watches (rows)
//! ```

pub mod config;
pub mod rpc;
pub mod supervisor;
pub mod sync;
pub mod tailnet;
pub mod views;
pub mod workspace;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use atelier_driver::{AgentDriver, ClaudeCode};
use atelier_store::Store;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};

pub use config::Config;
pub use supervisor::Supervisor;
use tailnet::PeerAuth;

#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    pub store: Store,
    pub supervisor: Supervisor,
    /// Flips to `true` when the server is stopping; connections close on it.
    pub stopping: tokio::sync::watch::Sender<bool>,
    /// Vouches for token-less peers on the tailnet listener.
    pub peers: Arc<dyn PeerAuth>,
}

impl App {
    pub async fn start(config: Config) -> Result<Self> {
        let driver: Arc<dyn AgentDriver> =
            Arc::new(ClaudeCode { bin: config.claude_bin.clone(), ..Default::default() });
        Self::with_driver(config, driver).await
    }

    pub async fn with_driver(config: Config, driver: Arc<dyn AgentDriver>) -> Result<Self> {
        let store = Store::open(config.db_path(), views::projections())?;
        let supervisor = Supervisor::new(store.clone(), driver);
        supervisor.recover().await?;
        supervisor.spawn_reaper(config.idle_timeout);
        let (stopping, _) = tokio::sync::watch::channel(false);
        let peers = Arc::new(tailnet::Tailscale::new(config.tailscale_bin.clone()));
        Ok(Self { config: Arc::new(config), store, supervisor, stopping, peers })
    }

    /// For the loopback listener: every client needs the token.
    pub fn router(&self) -> Router {
        Router::new().route("/ws", get(ws)).route("/health", get(health)).with_state(self.clone())
    }

    /// For the tailnet listener: the token, or a peer Tailscale vouches for.
    /// Serve with `into_make_service_with_connect_info::<SocketAddr>()`.
    pub fn tailnet_router(&self) -> Router {
        Router::new().route("/ws", get(ws_tailnet)).route("/health", get(health)).with_state(self.clone())
    }
}

async fn ws(State(app): State<App>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| sync::connection(app, socket, None))
}

async fn ws_tailnet(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let origin = headers.get("origin").map(|o| o.to_str().unwrap_or("?"));
    // A page from a foreign origin may still use a token; it just can't borrow the device's identity.
    let peer = tailnet::origin_ok(origin).then_some(peer.ip());
    upgrade.on_upgrade(move |socket| sync::connection(app, socket, peer))
}

async fn health(State(app): State<App>) -> impl IntoResponse {
    Json(serde_json::json!({
        "server": concat!("atelierd/", env!("CARGO_PKG_VERSION")),
        "protocol": atelier_proto::PROTOCOL,
        "head": app.store.head(),
    }))
}
