//! How a client on the same machine finds and authenticates to atelierd.
//! Shared by the server and the desktop host so they can't disagree.

use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 7747;

/// `ATELIER_HOME`, or `~/.atelier`.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("ATELIER_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".atelier")))
}

/// The shared secret in `<home>/token`, created (mode 0600) if missing.
/// Whoever runs first — server or desktop app — creates it.
pub fn token(home: &Path) -> std::io::Result<String> {
    let path = home.join("token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.to_owned());
        }
    }
    std::fs::create_dir_all(home)?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let tmp = home.join(".token.tmp");
    std::fs::write(&tmp, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    // Atomic, so a concurrently starting server and app agree on one token.
    match std::fs::hard_link(&tmp, &path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(token)
        }
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(std::fs::read_to_string(&path)?.trim().to_owned())
        }
    }
}
