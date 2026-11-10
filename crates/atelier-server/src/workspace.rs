//! Where a session's agent runs: a fresh git worktree (default) or the
//! project directory itself.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use atelier_core::SessionId;
use atelier_core::domain::{Workspace, WorkspaceMode};
use atelier_proto::{ChangeStatus, Changes, FileChange, ProjectRow};
use tokio::process::Command;

/// Free worktrees kept per project. Each holds a warm `target/` and `node_modules/`, so the cap is about disk.
const POOL_CAP: usize = 3;

/// Gitignored directories worth carrying into a new worktree: rebuilding them is what makes a fresh checkout slow.
const SEED_DIRS: &[&str] = &["target", "node_modules", ".venv"];

/// Serializes claiming and releasing pool entries, so two sessions never get the same directory.
static POOL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `in_use` is every path a live (unarchived) session runs in.
pub async fn prepare(
    worktrees_dir: &Path,
    project: &ProjectRow,
    session: &SessionId,
    title: &str,
    mode: WorkspaceMode,
    in_use: &[String],
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
            let pool_dir = worktrees_dir.join(project.id.as_str());
            tokio::fs::create_dir_all(&pool_dir).await?;

            let _guard = POOL.lock().await;
            // Stale registrations (a worktree deleted by hand) would otherwise look like pool entries.
            let _ = git(repo, &["worktree", "prune"]).await;
            for free in free_worktrees(repo, &pool_dir, in_use).await? {
                // Ignored files (deps, build output) survive a checkout; that's the point of reusing.
                if git(&free, &["checkout", "-q", "-b", &branch, &base]).await.is_ok() {
                    return Ok(Workspace {
                        mode,
                        path: free.to_string_lossy().into_owned(),
                        branch: Some(branch),
                        base: Some(base),
                    });
                }
            }
            let path = pool_dir.join(short);
            let path_s = path.to_string_lossy().into_owned();
            git(repo, &["worktree", "add", "-b", &branch, &path_s, &base]).await?;
            seed(repo, &path).await;
            Ok(Workspace { mode, path: path_s, branch: Some(branch), base: Some(base) })
        }
    }
}

/// Detached, clean worktrees under `pool_dir` that no session is using, oldest first.
async fn free_worktrees(repo: &Path, pool_dir: &Path, in_use: &[String]) -> Result<Vec<PathBuf>> {
    let listing = git_raw(repo, &["worktree", "list", "--porcelain"]).await?;
    // git reports real paths; `pool_dir` may go through a symlink.
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let (real_pool, in_use): (PathBuf, Vec<PathBuf>) =
        (real(pool_dir), in_use.iter().map(|u| real(Path::new(u))).collect());
    let mut free = Vec::new();
    for block in listing.split("\n\n") {
        let Some(path) = block.lines().find_map(|l| l.strip_prefix("worktree ")) else { continue };
        let path = PathBuf::from(path);
        let detached = block.lines().any(|l| l == "detached");
        if !detached || path.parent() != Some(&real_pool) || in_use.contains(&path) {
            continue;
        }
        let path = pool_dir.join(path.file_name().expect("has name"));
        if git(&path, &["status", "--porcelain"]).await.is_ok_and(|s| s.is_empty()) {
            free.push(path);
        }
    }
    free.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    Ok(free)
}

/// Copy-on-write clone of the project's dependency and build directories into a new worktree, so the agent
/// can run `make check` immediately. Best effort: without CoW support a full copy would cost more than it saves.
async fn seed(repo: &Path, worktree: &Path) {
    let Ok(ignored) = git(repo, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory"]).await
    else {
        return;
    };
    for rel in ignored.lines().map(|l| l.trim_end_matches('/')) {
        let rel = Path::new(rel);
        if !rel.file_name().is_some_and(|n| SEED_DIRS.iter().any(|d| n == *d)) {
            continue;
        }
        let (src, dst) = (repo.join(rel), worktree.join(rel));
        let Some(parent) = dst.parent() else { continue };
        if !src.is_dir() || dst.exists() || !parent.is_dir() {
            continue;
        }
        let cow = if cfg!(target_os = "macos") { "-c" } else { "--reflink=always" };
        let ok = Command::new("cp").args(["-R", cow]).arg(&src).arg(parent).status().await.is_ok_and(|s| s.success());
        if !ok {
            tracing::warn!("couldn't clone {} into {}", src.display(), worktree.display());
            let _ = tokio::fs::remove_dir_all(&dst).await;
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

/// Give the worktree back: detach it and delete its branch, but keep the directory (and its dependencies) for
/// the next session. Call only after [`unsafe_to_remove`] said `None`. Returns whether it was pooled; over
/// the cap, or if resetting fails, the worktree is deleted instead.
pub async fn release(project: &ProjectRow, ws: &Workspace) -> Result<bool> {
    let repo = Path::new(&project.path);
    let dir = Path::new(&ws.path);
    let mut pooled = false;
    if dir.exists() {
        let _guard = POOL.lock().await;
        let pool_dir = dir.parent().context("worktree has no parent")?;
        pooled = git(dir, &["checkout", "-q", "--detach"]).await.is_ok() && git(dir, &["clean", "-fdq"]).await.is_ok();
        if pooled {
            // The oldest beyond the cap go; this one is the newest.
            let free = free_worktrees(repo, pool_dir, &[]).await.unwrap_or_default();
            for old in free.iter().take(free.len().saturating_sub(POOL_CAP)) {
                let _ = git(repo, &["worktree", "remove", "--force", &old.to_string_lossy()]).await;
            }
            pooled = dir.exists();
        } else {
            git(repo, &["worktree", "remove", &ws.path]).await?;
        }
    }
    if let Some(branch) = &ws.branch {
        // -D: we've checked it's merged or empty, but not necessarily into the current branch.
        git(repo, &["branch", "-D", branch]).await?;
    }
    Ok(pooled)
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

    async fn sh(dir: &Path, args: &[&str]) {
        git(dir, args).await.unwrap();
    }

    #[tokio::test]
    async fn new_worktrees_inherit_dependencies_and_get_pooled() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("web/node_modules/pkg")).unwrap();
        std::fs::create_dir_all(repo.join("target")).unwrap();
        sh(&repo, &["init", "-q"]).await;
        std::fs::write(repo.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        std::fs::write(repo.join("web/a.txt"), "a").unwrap();
        std::fs::write(repo.join("web/node_modules/pkg/index.js"), "js").unwrap();
        std::fs::write(repo.join("target/artifact"), "bin").unwrap();
        sh(&repo, &["add", "."]).await;
        sh(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]).await;

        let project = ProjectRow {
            id: atelier_core::ProjectId("p1".into()),
            name: "repo".into(),
            path: repo.to_string_lossy().into_owned(),
            is_git: true,
            added_at: 0,
            updated_at: 0,
            sessions: 0,
            running: 0,
            waiting: 0,
        };
        let wt = tmp.path().join("worktrees");
        let (s1, s2) = (SessionId::generate(), SessionId::generate());
        let a = prepare(&wt, &project, &s1, "one", WorkspaceMode::Worktree, &[]).await.unwrap();
        let a_path = PathBuf::from(&a.path);
        // Seeding is best effort (needs copy-on-write), so only check what it must not break.
        if a_path.join("target/artifact").exists() {
            assert!(a_path.join("web/node_modules/pkg/index.js").exists());
        }

        // Still in use: a second session gets its own directory.
        let b =
            prepare(&wt, &project, &s2, "two", WorkspaceMode::Worktree, std::slice::from_ref(&a.path)).await.unwrap();
        assert_ne!(a.path, b.path);

        // Released, it's reused and keeps ignored files.
        std::fs::write(a_path.join("target/warm"), "w").unwrap();
        assert!(release(&project, &a).await.unwrap());
        assert!(git(&repo, &["branch", "--list", a.branch.as_deref().unwrap()]).await.unwrap().is_empty());
        let c = prepare(
            &wt,
            &project,
            &SessionId::generate(),
            "three",
            WorkspaceMode::Worktree,
            std::slice::from_ref(&b.path),
        )
        .await
        .unwrap();
        assert_eq!(c.path, a.path);
        assert!(a_path.join("target/warm").exists());
    }
}
