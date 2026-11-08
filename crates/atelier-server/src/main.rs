use anyhow::{Context, Result};
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
