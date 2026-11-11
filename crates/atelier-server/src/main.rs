use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_server::tailnet::Tailscale;
use atelier_server::{App, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env(std::env::args())?;
    let listen = config.listen;
    tracing::info!("home {}", config.home.display());

    let app = App::start(config).await?;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen} (is another atelierd running?)"))?;
    tracing::info!("listening on {listen}");
    // Agents started from here on get Atelier's MCP tools.
    app.supervisor.set_mcp_addr(listener.local_addr()?);

    if app.config.listen_tailscale {
        tokio::spawn(serve_tailnet(app.clone(), listen.port()));
    }

    let supervisor = app.supervisor.clone();
    let stopping = app.stopping.clone();
    axum::serve(listener, app.router())
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // Open WebSockets would otherwise hold graceful shutdown forever.
            let _ = stopping.send(true);
        })
        .await?;
    supervisor.shutdown().await;
    Ok(())
}

/// Listen on the tailnet address too. Tailscale may not be up yet (e.g. at
/// login, under launchd), so keep trying rather than give up on remote access.
async fn serve_tailnet(app: App, port: u16) {
    let ts = Tailscale::new(app.config.tailscale_bin.clone());
    let mut stopping = app.stopping.subscribe();
    let listener = loop {
        let bound = match ts.ipv4().await {
            Ok(ip) => tokio::net::TcpListener::bind((ip, port)).await.with_context(|| format!("binding {ip}:{port}")),
            Err(e) => Err(e),
        };
        match bound {
            Ok(l) => break l,
            Err(e) => tracing::warn!("tailnet listener unavailable, retrying in 30s: {e:#}"),
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            _ = stopping.wait_for(|s| *s) => return,
        }
    };
    if let Ok(addr) = listener.local_addr() {
        tracing::info!("listening on {addr} (tailnet)");
    }
    let service = app.tailnet_router().into_make_service_with_connect_info::<SocketAddr>();
    let result = axum::serve(listener, service)
        .with_graceful_shutdown(async move {
            let _ = stopping.wait_for(|s| *s).await;
        })
        .await;
    if let Err(e) = result {
        tracing::error!("tailnet listener failed: {e:#}");
    }
}

/// Ctrl-C, or SIGTERM (launchd, systemd, `kill`).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
