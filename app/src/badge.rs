//! `set_badge_count`: the dock badge, the number of sessions waiting on the person.
//! Desktop only; the web UI calls it only when it finds the Tauri channel.

use tauri::Manager;

#[tauri::command]
pub fn set_badge_count(app: tauri::AppHandle, count: Option<i64>) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    window.set_badge_count(count).map_err(|e| e.to_string())
}
