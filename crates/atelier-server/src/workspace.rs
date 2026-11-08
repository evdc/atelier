//! Where a session's agent runs: a fresh git worktree (default) or the
//! project directory itself.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use atelier_core::SessionId;
use atelier_core::domain::{Workspace, WorkspaceMode};
use atelier_proto::ProjectRow;
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

/// Is `path` inside a git work tree?
pub async fn is_git(path: &Path) -> bool {
    git(path, &["rev-parse", "--is-inside-work-tree"]).await.is_ok_and(|o| o == "true")
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().await.context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
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
