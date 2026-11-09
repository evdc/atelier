//! `open_path`: hand a directory to the person's own tools. Allowlisted by
//! construction: a fixed set of kinds, each a fixed program, and the path is
//! only ever passed as an argument to an existing directory.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Terminal,
    Editor,
    Finder,
}

#[tauri::command]
pub fn open_path(kind: Kind, path: String) -> Result<(), String> {
    let dir = Path::new(&path);
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(format!("{path} is not a directory on this machine"));
    }
    let argv: Vec<String> = match kind {
        Kind::Finder => vec![if cfg!(target_os = "macos") { "open" } else { "xdg-open" }.into()],
        Kind::Terminal if cfg!(target_os = "macos") => vec!["open".into(), "-a".into(), "Terminal".into()],
        Kind::Terminal => return Err("opening a terminal isn't supported on this platform yet".into()),
        Kind::Editor => editor(),
    };
    let program = &argv[0];
    Command::new(program)
        .args(&argv[1..])
        .arg(dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("couldn't run {program}: {e}"))
}

/// `$VISUAL` (which may carry flags), else the first of `code`/`zed` on PATH.
/// A GUI app often doesn't inherit the shell's environment, so the fallbacks matter.
fn editor() -> Vec<String> {
    if let Ok(v) = std::env::var("VISUAL")
        && !v.trim().is_empty()
    {
        return v.split_whitespace().map(str::to_owned).collect();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for name in ["code", "zed"] {
        if std::env::split_paths(&path).any(|d| d.join(name).is_file()) {
            return vec![name.into()];
        }
    }
    vec!["code".into()]
}
