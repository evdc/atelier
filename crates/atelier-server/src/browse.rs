//! Read-only views of the server's directories, for the add-project picker.
//! Like `session.changes`, these are snapshots of the filesystem; nothing is
//! logged.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use atelier_proto::{DirEntry, DirListing, Suggestion, Suggestions};

/// Enough for any ordinary directory; the picker says when it's been cut.
const MAX_ENTRIES: usize = 500;
const MAX_SUGGESTIONS: usize = 8;
/// Where people keep repositories, one level under home.
const SUGGEST_ROOTS: [&str; 3] = ["Projects", "src", "code"];

pub async fn list(path: Option<String>, hidden: bool) -> Result<DirListing> {
    let path = match path {
        Some(p) => expand_home(&p),
        None => home()?,
    };
    tokio::task::spawn_blocking(move || list_blocking(&path, hidden)).await?
}

pub async fn suggest() -> Result<Suggestions> {
    let home = home()?;
    Ok(Suggestions { entries: tokio::task::spawn_blocking(move || suggest_blocking(&home)).await? })
}

fn list_blocking(path: &Path, hidden: bool) -> Result<DirListing> {
    let dir = std::fs::canonicalize(path).with_context(|| format!("{} doesn't exist", path.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let read = std::fs::read_dir(&dir).with_context(|| format!("can't read {}", dir.display()))?;
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|e| hidden || !e.file_name().to_string_lossy().starts_with('.'))
        // Follows symlinks, so a link to a directory is browsable.
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| (n.to_lowercase(), n.clone()));
    names.truncate(MAX_ENTRIES);
    // `.git` is checked only for what's shown: a stat per entry, not a git process.
    let entries = names
        .into_iter()
        .map(|name| {
            let is_git = dir.join(&name).join(".git").exists();
            DirEntry { name, is_dir: true, is_git }
        })
        .collect();
    Ok(DirListing {
        path: dir.to_string_lossy().into_owned(),
        parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
    })
}

/// Repositories directly under the suggestion roots, most recently changed first.
fn suggest_blocking(home: &Path) -> Vec<Suggestion> {
    let mut found: Vec<(SystemTime, Suggestion)> = Vec::new();
    for root in SUGGEST_ROOTS {
        let Ok(read) = std::fs::read_dir(home.join(root)) else { continue };
        for e in read.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if name.starts_with('.') || !path.is_dir() || !path.join(".git").exists() {
                continue;
            }
            // The index moves with staging and commits; a worktree's `.git` is a file, so fall back to it.
            let Some(when) = [path.join(".git/index"), path.join(".git")]
                .iter()
                .find_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            else {
                continue;
            };
            found.push((when, Suggestion { path: path.to_string_lossy().into_owned(), name }));
        }
    }
    found.sort_by_key(|b| std::cmp::Reverse(b.0));
    found.into_iter().take(MAX_SUGGESTIONS).map(|(_, s)| s).collect()
}

/// `~/x` → the home directory's `x`.
pub fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| p.into()),
        None => p.into(),
    }
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("the server has no home directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        for d in ["beta", "Alpha", "repo/.git", ".hidden"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        std::fs::write(p.join("file.txt"), "x").unwrap();
        t
    }

    #[test]
    fn lists_directories_only_sorted_and_marks_git() {
        let t = tree();
        let got = list_blocking(t.path(), false).unwrap();
        let names: Vec<_> = got.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "repo"]);
        assert!(got.entries.iter().all(|e| e.is_dir));
        assert!(!got.entries[0].is_git && got.entries[2].is_git);
        assert_eq!(got.path, std::fs::canonicalize(t.path()).unwrap().to_string_lossy());
        assert!(got.parent.is_some());
    }

    #[test]
    fn hidden_entries_only_when_asked() {
        let t = tree();
        assert!(!list_blocking(t.path(), false).unwrap().entries.iter().any(|e| e.name == ".hidden"));
        assert!(list_blocking(t.path(), true).unwrap().entries.iter().any(|e| e.name == ".hidden"));
    }

    #[test]
    fn caps_entries_and_rejects_files() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..(MAX_ENTRIES + 20) {
            std::fs::create_dir(t.path().join(format!("d{i:04}"))).unwrap();
        }
        assert_eq!(list_blocking(t.path(), false).unwrap().entries.len(), MAX_ENTRIES);
        let file = t.path().join("plain.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(list_blocking(&file, false).is_err());
        assert!(list_blocking(&t.path().join("missing"), false).is_err());
    }

    #[test]
    fn suggests_recent_repos_under_roots() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join("Projects/old/.git")).unwrap();
        std::fs::write(h.join("Projects/old/.git/index"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(h.join("src/new/.git")).unwrap();
        std::fs::write(h.join("src/new/.git/index"), "").unwrap();
        std::fs::create_dir_all(h.join("Projects/plain")).unwrap();
        std::fs::create_dir_all(h.join("elsewhere/repo/.git")).unwrap();
        let names: Vec<_> = suggest_blocking(h).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["new", "old"]);
    }
}
