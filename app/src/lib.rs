//! The app host, desktop and phone. Deliberately thin:
//!
//! - Desktop: make sure an `atelierd` is running (start one, detached, if not),
//!   so agents keep running after this window closes, and open the web UI with
//!   the server's address and token injected.
//! - Phone: a pure client. There's no server to start; the web UI connects with
//!   the config it stored in `localStorage` (Settings).
//!
//! The UI talks to the server over WebSocket on both. The single Tauri command, `open_path` (desktop only),
//! exists because "open this in my terminal/editor/Finder" must run on the client machine.

#[cfg(desktop)]
mod open;
#[cfg(desktop)]
mod server;

use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.invoke_handler(tauri::generate_handler![open::open_path]);
    builder
        .setup(|app| {
            let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default());
            if let Some(init) = init_script()? {
                window = window.initialization_script(&init);
            }
            #[cfg(desktop)]
            {
                window = window.title("Atelier").inner_size(1280.0, 820.0).min_inner_size(640.0, 420.0);
            }
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Atelier");
}

#[cfg(desktop)]
fn init_script() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let target = server::ensure_running()?;
    Ok(Some(format!("window.__ATELIER__ = {};", serde_json::to_string(&target)?)))
}

// Nothing to inject: the web UI uses the server it saved in Settings. (Build-time `ATELIER_URL` overrides
// don't work here: the Tauri CLI's Xcode build step runs cargo with only TAURI_*/CARGO_*/RUST_* env vars.)
#[cfg(mobile)]
fn init_script() -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(None)
}
