//! The desktop host. Deliberately thin:
//!
//! 1. Make sure an `atelierd` is running (start one, detached, if not), so
//!    agents keep running after this window closes.
//! 2. Open the web UI with the server's address and token injected.
//!
//! The UI talks to the server over WebSocket exactly like the phone does. The
//! single Tauri command, `open_path`, exists because "open this in my
//! terminal/editor/Finder" must run on the client machine.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod open;
mod server;

use tauri::{WebviewUrl, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![open::open_path])
        .setup(|app| {
            let target = server::ensure_running()?;
            let init = format!("window.__ATELIER__ = {};", serde_json::to_string(&target)?);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Atelier")
                .inner_size(1280.0, 820.0)
                .min_inner_size(640.0, 420.0)
                .initialization_script(&init)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Atelier");
}
