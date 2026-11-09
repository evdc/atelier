//! Where a session's agent runs: a fresh git worktree (default) or the
//! project directory itself.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use atelier_core::SessionId;
use atelier_core::domain::{Workspace, WorkspaceMode};
use atelier_proto::{ChangeStatus, Changes, FileChange, ProjectRow};
use tokio::process::Command;

pub async fn prepare(
    worktrees_dir: &Path,
    project: &ProjectRow,
    session: &SessionId,
    title: &str,
    mode: WorkspaceMode,
) -> Result<Workspace> {
    match mode {
        WorkspaceMode::Direct => Ok(Workspace { mode, path: project.path.clone(), branch: None, base: None }),
        WorkspaceMode::Worktree => {
            if !project.is_git {
                bail!("{} is not a git repository; use the project directory instead", project.name);
            }
            let repo = Path::new(&project.path);
            let base = git(repo, &["rev-parse", "HEAD"])
                .await
                .context("the repository has no commits yet; use the project directory instead")?;
            let short = short_id(session);
            let branch = format!("atelier/{}-{short}", slug(title));
            let path: PathBuf = worktrees_dir.join(project.id.as_str()).join(short);
            tokio::fs::create_dir_all(path.parent().expect("has parent")).await?;
            let path_s = path.to_string_lossy().into_owned();
            git(repo, &["worktree", "add", "-b", &branch, &path_s, &base]).await?;
            Ok(Workspace { mode, path: path_s, branch: Some(branch), base: Some(base) })
        }
    }
}

/// Everything the workspace differs from its base by: committed, staged,
/// unstaged and untracked.
pub async fn changes(ws: &Workspace) -> Result<Changes> {
    let dir = Path::new(&ws.path);
    let base = ws.base.as_deref().unwrap_or("HEAD");
    let numstat = git(dir, &["diff", "--numstat", "--no-renames", base]).await?;
    let names = git(dir, &["diff", "--name-status", "--no-renames", base]).await?;
    let status: HashMap<&str, ChangeStatus> = names
        .lines()
        .filter_map(|l| {
            let (s, path) = l.split_once('\t')?;
            let st = match s {
                "A" => ChangeStatus::Added,
                "D" => ChangeStatus::Deleted,
                _ => ChangeStatus::Modified,
            };
            Some((path, st))
        })
        .collect();
    let mut files: Vec<FileChange> = numstat
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let (a, r, path) = (it.next()?, it.next()?, it.next()?);
            Some(FileChange {
                path: path.to_owned(),
                status: status.get(path).copied().unwrap_or(ChangeStatus::Modified),
                // Binary files report "-".
                added: a.parse().unwrap_or(0),
                removed: r.parse().unwrap_or(0),
            })
        })
        .collect();
    for path in git(dir, &["ls-files", "--others", "--exclude-standard"]).await?.lines() {
        let added = match tokio::fs::read(dir.join(path)).await {
            Ok(b) if !b.contains(&0) => text_lines(&b),
            _ => 0,
        };
        files.push(FileChange { path: path.to_owned(), status: ChangeStatus::Untracked, added, removed: 0 });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changes { base: base.to_owned(), files })
}

fn text_lines(b: &[u8]) -> u32 {
    let n = b.iter().filter(|&&c| c == b'\n').count() + usize::from(b.last().is_some_and(|&c| c != b'\n'));
    n as u32
}

/// Unified diff of one file against the base (untracked files diff against nothing).
pub async fn diff(ws: &Workspace, path: &str) -> Result<String> {
    let dir = Path::new(&ws.path);
    // `path` comes from a client: keep it inside the workspace and out of option position.
    if Path::new(path).components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("bad path");
    }
    let base = ws.base.as_deref().unwrap_or("HEAD");
    let tracked = git_raw(dir, &["ls-files", "--error-unmatch", "--", path]).await.is_ok();
    if tracked || !dir.join(path).exists() {
        return git_raw(dir, &["diff", "--no-renames", base, "--", path]).await;
    }
    // --no-index exits 1 when the files differ.
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff", "--no-index", "--", "/dev/null", path])
        .output()
        .await
        .context("running git")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Why a worktree can't be deleted without losing work, or `None` if it can.
pub async fn unsafe_to_remove(project: &ProjectRow, ws: &Workspace) -> Result<Option<String>> {
    let (Some(branch), Some(base)) = (&ws.branch, &ws.base) else { return Ok(None) };
    let dir = Path::new(&ws.path);
    if !dir.exists() {
        return Ok(None);
    }
    if !git(dir, &["status", "--porcelain"]).await?.is_empty() {
        return Ok(Some("the worktree has uncommitted changes".into()));
    }
    let repo = Path::new(&project.path);
    let range = format!("{base}..{branch}");
    let ahead = git(repo, &["rev-list", "--count", &range]).await?;
    let merged = git_raw(repo, &["merge-base", "--is-ancestor", branch, "HEAD"]).await.is_ok();
    if ahead != "0" && !merged {
        return Ok(Some(format!("branch {branch} has commits that aren't merged")));
    }
    Ok(None)
}

/// Delete the worktree and its branch. Call only after [`unsafe_to_remove`] said `None`.
pub async fn remove(project: &ProjectRow, ws: &Workspace) -> Result<()> {
    let repo = Path::new(&project.path);
    if Path::new(&ws.path).exists() {
        git(repo, &["worktree", "remove", &ws.path]).await?;
    }
    if let Some(branch) = &ws.branch {
        // -D: we've checked it's merged or empty, but not necessarily into the current branch.
        git(repo, &["branch", "-D", branch]).await?;
    }
    Ok(())
}

/// Is `path` inside a git work tree?
pub async fn is_git(path: &Path) -> bool {
    git(path, &["rev-parse", "--is-inside-work-tree"]).await.is_ok_and(|o| o == "true")
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(git_raw(dir, args).await?.trim().to_owned())
}

/// Like [`git`] but keeps whitespace (diffs end in a newline that matters).
async fn git_raw(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(["-c", "core.quotepath=false", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .await
        .context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The random tail of a UUIDv7 (the head is a timestamp, shared by
/// sessions created close together).
fn short_id(id: &SessionId) -> &str {
    let s = id.as_str();
    &s[s.len().saturating_sub(8)..]
}

/// `"Fix the flaky merge test!"` → `"fix-the-flaky-merge-test"`, at most 40 chars.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "session".into() } else { out.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Fix the flaky merge test!"), "fix-the-flaky-merge-test");
        assert_eq!(slug("¿?"), "session");
        assert!(slug(&"word ".repeat(30)).len() <= 40);
    }
}
