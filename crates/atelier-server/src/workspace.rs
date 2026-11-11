//! Where a session's agent runs: a fresh git worktree (default) or the
//! project directory itself.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use atelier_core::SessionId;
use atelier_core::domain::{Workspace, WorkspaceMode};
use atelier_proto::{ChangeStatus, Changes, FileChange, MergeResult, ProjectRow};
use tokio::process::Command;

/// Free worktrees kept per project. Each holds a warm `target/` and `node_modules/`, so the cap is about disk.
const POOL_CAP: usize = 3;

/// Gitignored directories worth carrying into a new worktree: rebuilding them is what makes a fresh checkout slow.
const SEED_DIRS: &[&str] = &["target", "node_modules", ".venv"];

/// Serializes claiming and releasing pool entries, so two sessions never get the same directory.
static POOL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `in_use` is every path a live (unarchived) session runs in; `start` is the commit a new worktree begins at
/// (`None` for the project's HEAD).
pub async fn prepare(
    worktrees_dir: &Path,
    project: &ProjectRow,
    session: &SessionId,
    title: &str,
    mode: WorkspaceMode,
    in_use: &[String],
    start: Option<&str>,
) -> Result<Workspace> {
    let (ws, fresh) = claim(worktrees_dir, project, session, title, mode, in_use, start).await?;
    if fresh {
        seed(Path::new(&project.path), Path::new(&ws.path)).await;
    }
    Ok(ws)
}

/// Like [`prepare`] but without seeding. The bool says the worktree is brand new and should be passed to
/// [`seed`]; cloning a large `target/` can take a long time, which callers shouldn't make a user wait on.
pub async fn claim(
    worktrees_dir: &Path,
    project: &ProjectRow,
    session: &SessionId,
    title: &str,
    mode: WorkspaceMode,
    in_use: &[String],
    start: Option<&str>,
) -> Result<(Workspace, bool)> {
    match mode {
        WorkspaceMode::Direct => Ok((
            Workspace { mode, path: project.path.clone(), branch: None, base: None, from: None, snapshot: None },
            false,
        )),
        WorkspaceMode::Shared => bail!("a shared workspace is another session's worktree; there's nothing to claim"),
        WorkspaceMode::Worktree => {
            if !project.is_git {
                bail!("{} is not a git repository; use the project directory instead", project.name);
            }
            let repo = Path::new(&project.path);
            let base = match start {
                Some(commit) => commit.to_owned(),
                None => git(repo, &["rev-parse", "HEAD"])
                    .await
                    .context("the repository has no commits yet; use the project directory instead")?,
            };
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
                    let path = free.to_string_lossy().into_owned();
                    return Ok((worktree(mode, path, &branch, base), false));
                }
            }
            let path = pool_dir.join(short);
            let path_s = path.to_string_lossy().into_owned();
            git(repo, &["worktree", "add", "-b", &branch, &path_s, &base]).await?;
            Ok((worktree(mode, path_s, &branch, base), true))
        }
    }
}

fn worktree(mode: WorkspaceMode, path: String, branch: &str, base: String) -> Workspace {
    Workspace { mode, path, branch: Some(branch.to_owned()), base: Some(base), from: None, snapshot: None }
}

/// Another session's workspace, read-only: the same directory, branch and base, owned by `owner`.
pub fn share(owner: &Workspace, owner_id: &SessionId) -> Workspace {
    Workspace {
        mode: WorkspaceMode::Shared,
        path: owner.path.clone(),
        branch: owner.branch.clone(),
        base: owner.base.clone(),
        from: Some(owner_id.clone()),
        snapshot: None,
    }
}

/// Commits the working tree of `ws` as it is now, uncommitted and untracked (non-ignored) files included. A
/// temporary index does the staging, so the source's index, HEAD and files are untouched. A clean tree is just
/// HEAD. The commit stays alive through `refs/atelier/snap/<session>` until [`release`] drops it.
pub async fn snapshot(ws: &Workspace, title: &str, session: &SessionId) -> Result<String> {
    let dir = Path::new(&ws.path);
    let head = git(dir, &["rev-parse", "HEAD"]).await.context("the session's worktree has no commits yet")?;
    let index = std::env::temp_dir().join(format!("atelier-snapshot-{}.index", session.as_str()));
    let tree = write_tree(dir, &index).await;
    let _ = tokio::fs::remove_file(&index).await;
    let tree = tree?;
    if tree == git(dir, &["rev-parse", "HEAD^{tree}"]).await? {
        return Ok(head);
    }
    let message = format!("atelier: snapshot of {title} ({})", session.as_str());
    let commit = git_env(dir, SNAPSHOT_IDENTITY, &["commit-tree", &tree, "-p", &head, "-m", &message]).await?;
    git(dir, &["update-ref", &format!("refs/atelier/snap/{}", session.as_str()), &commit]).await?;
    Ok(commit)
}

/// Commit identity for snapshots, which must not depend on the user's git config.
const SNAPSHOT_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Atelier"),
    ("GIT_AUTHOR_EMAIL", "atelier@localhost"),
    ("GIT_COMMITTER_NAME", "Atelier"),
    ("GIT_COMMITTER_EMAIL", "atelier@localhost"),
];

async fn write_tree(dir: &Path, index: &Path) -> Result<String> {
    let index_s = index.to_string_lossy();
    let env = [("GIT_INDEX_FILE", index_s.as_ref())];
    // Start from the source's own index, so what it has staged stays staged in the snapshot. Only a repo with no
    // index yet starts from HEAD. The source's index is read, never written.
    let source_index = dir.join(git(dir, &["rev-parse", "--git-path", "index"]).await?);
    if tokio::fs::try_exists(&source_index).await? {
        tokio::fs::copy(&source_index, index).await?;
    } else {
        git_env_raw(dir, &env, &["read-tree", "HEAD"]).await?;
    }
    git_env_raw(dir, &env, &["add", "-A"]).await?;
    git_env(dir, &env, &["write-tree"]).await
}

/// Removes the snapshot refs that point at `commit`, so the commit can be garbage-collected. Best effort.
pub async fn drop_snapshot(repo: &Path, commit: &str) {
    let at = format!("--points-at={commit}");
    let Ok(refs) = git_raw(repo, &["for-each-ref", "--format=%(refname)", &at, "refs/atelier/snap/"]).await else {
        return;
    };
    for r in refs.lines() {
        let _ = git(repo, &["update-ref", "-d", r]).await;
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
pub async fn seed(repo: &Path, worktree: &Path) {
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
    if let Some(snap) = &ws.snapshot {
        drop_snapshot(repo, snap).await;
    }
    Ok(pooled)
}

/// Commit all of the worktree's changes, bring the project's current branch into the session branch (so
/// conflicts surface in the worktree, where the agent can fix them), then fast-forward the project to it.
/// With conflicts the merge is left in progress in the worktree; call again once they're resolved.
pub async fn merge(project: &ProjectRow, ws: &Workspace, title: &str) -> Result<MergeResult> {
    let (Some(branch), true) = (&ws.branch, ws.mode == WorkspaceMode::Worktree) else {
        bail!("only worktree sessions can be merged");
    };
    let (repo, dir) = (Path::new(&project.path), Path::new(&ws.path));
    let target = git(repo, &["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .context("the project has a detached HEAD; check out a branch to merge into")?;
    let conflicted = |files: Vec<String>| MergeResult { target: target.clone(), merged: false, conflicts: files };

    let merging = git(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_ok();
    if merging {
        // Don't commit conflict markers the agent hasn't dealt with yet; a file it fixed but didn't `git add`
        // is as good as resolved.
        let mut left = Vec::new();
        for f in unmerged(dir).await? {
            let text = tokio::fs::read(dir.join(&f)).await.unwrap_or_default();
            if text.windows(8).any(|w| w == b"<<<<<<< ") {
                left.push(f);
            }
        }
        if !left.is_empty() {
            return Ok(conflicted(left));
        }
    }
    git(dir, &["add", "-A"]).await?;
    if !git(dir, &["status", "--porcelain"]).await?.is_empty() {
        git(dir, &["commit", "-q", "-m", title]).await?;
    }
    if !merging && let Err(e) = git(dir, &["merge", "-q", "--no-edit", &target]).await {
        let files = unmerged(dir).await?;
        if files.is_empty() {
            let _ = git(dir, &["merge", "--abort"]).await;
            return Err(e);
        }
        return Ok(conflicted(files));
    }
    git(repo, &["merge", "-q", "--ff-only", branch])
        .await
        .with_context(|| format!("couldn't update {target} in the project directory"))?;
    Ok(MergeResult { target, merged: true, conflicts: vec![] })
}

async fn unmerged(dir: &Path) -> Result<Vec<String>> {
    Ok(git(dir, &["diff", "--name-only", "--diff-filter=U"]).await?.lines().map(str::to_owned).collect())
}

/// Is `path` inside a git work tree?
pub async fn is_git(path: &Path) -> bool {
    git(path, &["rev-parse", "--is-inside-work-tree"]).await.is_ok_and(|o| o == "true")
}

/// Every work tree of the repository at `path` (including `path` itself). Empty if it isn't one.
pub async fn worktrees(path: &Path) -> Vec<PathBuf> {
    let Ok(out) = git_raw(path, &["worktree", "list", "--porcelain"]).await else {
        return Vec::new();
    };
    out.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from).collect()
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(git_raw(dir, args).await?.trim().to_owned())
}

/// Like [`git`], with extra environment variables.
async fn git_env(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    Ok(git_env_raw(dir, env, args).await?.trim().to_owned())
}

/// Like [`git`] but keeps whitespace (diffs end in a newline that matters).
async fn git_raw(dir: &Path, args: &[&str]) -> Result<String> {
    git_env_raw(dir, &[], args).await
}

async fn git_env_raw(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(["-c", "core.quotepath=false", "-C"])
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
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
    async fn merge_commits_fast_forwards_and_reports_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let id = ["-c", "user.email=t@t", "-c", "user.name=t"];
        let commit = |m: &'static str| {
            let mut a = id.to_vec();
            a.extend(["commit", "-q", "-am", m]);
            a
        };
        sh(&repo, &["init", "-q"]).await;
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        sh(&repo, &["add", "."]).await;
        sh(&repo, &commit("init")).await;
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
        let ws = prepare(&wt, &project, &SessionId::generate(), "m", WorkspaceMode::Worktree, &[], None).await.unwrap();
        let dir = PathBuf::from(&ws.path);
        // Git config for commits made by `merge` itself.
        sh(&dir, &["config", "user.email", "t@t"]).await;
        sh(&dir, &["config", "user.name", "t"]).await;

        // Uncommitted work (and a new file) lands on the project's branch.
        std::fs::write(dir.join("a.txt"), "one\nmine\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        let r = merge(&project, &ws, "m").await.unwrap();
        assert!(r.merged, "{r:?}");
        assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), "b\n");

        // Both sides touch the same line: conflict, left in the worktree.
        std::fs::write(repo.join("a.txt"), "one\ntheirs\n").unwrap();
        sh(&repo, &commit("theirs")).await;
        std::fs::write(dir.join("a.txt"), "one\nmine2\n").unwrap();
        let r = merge(&project, &ws, "m2").await.unwrap();
        assert_eq!((r.merged, r.conflicts.as_slice()), (false, &["a.txt".to_owned()][..]));
        // Still unresolved: asking again doesn't commit the markers.
        assert!(!merge(&project, &ws, "m2").await.unwrap().merged);
        std::fs::write(dir.join("a.txt"), "one\ntheirs\nmine2\n").unwrap();
        let r = merge(&project, &ws, "m2").await.unwrap();
        assert!(r.merged, "{r:?}");
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\ntheirs\nmine2\n");
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
        let a = prepare(&wt, &project, &s1, "one", WorkspaceMode::Worktree, &[], None).await.unwrap();
        let a_path = PathBuf::from(&a.path);
        // Seeding is best effort (needs copy-on-write), so only check what it must not break.
        if a_path.join("target/artifact").exists() {
            assert!(a_path.join("web/node_modules/pkg/index.js").exists());
        }

        // Still in use: a second session gets its own directory.
        let b = prepare(&wt, &project, &s2, "two", WorkspaceMode::Worktree, std::slice::from_ref(&a.path), None)
            .await
            .unwrap();
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
            None,
        )
        .await
        .unwrap();
        assert_eq!(c.path, a.path);
        assert!(a_path.join("target/warm").exists());
    }

    /// A repo with one commit, a gitignored file, and a dependency directory that the tests can seed from.
    async fn repo_with_history(dir: &Path) -> ProjectRow {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q"]).await;
        std::fs::write(repo.join(".gitignore"), "ignored.log\ntarget/\n").unwrap();
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        sh(&repo, &["add", "."]).await;
        sh(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]).await;
        ProjectRow {
            id: atelier_core::ProjectId("p1".into()),
            name: "repo".into(),
            path: repo.to_string_lossy().into_owned(),
            is_git: true,
            added_at: 0,
            updated_at: 0,
            sessions: 0,
            running: 0,
            waiting: 0,
        }
    }

    fn ws_at(path: &Path) -> Workspace {
        Workspace {
            mode: WorkspaceMode::Worktree,
            path: path.to_string_lossy().into_owned(),
            branch: None,
            base: None,
            from: None,
            snapshot: None,
        }
    }

    async fn out(dir: &Path, args: &[&str]) -> String {
        git(dir, args).await.unwrap()
    }

    #[tokio::test]
    async fn snapshot_has_working_tree_and_leaves_the_source_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let project = repo_with_history(tmp.path()).await;
        let repo = PathBuf::from(&project.path);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap(); // modified, unstaged
        std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
        sh(&repo, &["add", "staged.txt"]).await; // staged
        std::fs::write(repo.join("new.txt"), "n\n").unwrap(); // untracked
        std::fs::write(repo.join("ignored.log"), "x\n").unwrap(); // ignored
        let (head, index, status) = (
            out(&repo, &["rev-parse", "HEAD"]).await,
            out(&repo, &["ls-files", "-s"]).await,
            out(&repo, &["status", "--porcelain"]).await,
        );

        let id = SessionId::generate();
        let commit = snapshot(&ws_at(&repo), "work", &id).await.unwrap();

        assert_ne!(commit, head);
        let files = out(&repo, &["ls-tree", "-r", "--name-only", &commit]).await;
        assert!(files.contains("a.txt") && files.contains("staged.txt") && files.contains("new.txt"), "{files}");
        assert!(!files.contains("ignored.log"), "{files}");
        assert_eq!(out(&repo, &["rev-parse", "HEAD"]).await, head);
        assert_eq!(out(&repo, &["ls-files", "-s"]).await, index);
        assert_eq!(out(&repo, &["status", "--porcelain"]).await, status);
        let r = format!("refs/atelier/snap/{}", id.as_str());
        assert_eq!(out(&repo, &["rev-parse", &r]).await, commit);
    }

    #[tokio::test]
    async fn snapshot_of_a_clean_tree_is_head() {
        let tmp = tempfile::tempdir().unwrap();
        let project = repo_with_history(tmp.path()).await;
        let repo = PathBuf::from(&project.path);
        let head = out(&repo, &["rev-parse", "HEAD"]).await;
        assert_eq!(snapshot(&ws_at(&repo), "clean", &SessionId::generate()).await.unwrap(), head);
    }

    #[tokio::test]
    async fn branch_from_a_snapshot_starts_with_empty_changes_and_release_drops_the_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let project = repo_with_history(tmp.path()).await;
        let wt = tmp.path().join("worktrees");
        let owner =
            prepare(&wt, &project, &SessionId::generate(), "owner", WorkspaceMode::Worktree, &[], None).await.unwrap();
        let owner_dir = PathBuf::from(&owner.path);
        std::fs::write(owner_dir.join("a.txt"), "owner's edit\n").unwrap();

        let (owner_id, child_id) = (SessionId::generate(), SessionId::generate());
        let commit = snapshot(&owner, "owner", &child_id).await.unwrap();
        let (mut child, fresh) =
            claim(&wt, &project, &child_id, "child", WorkspaceMode::Worktree, &[], Some(&commit)).await.unwrap();
        assert!(fresh);
        child.from = Some(owner_id.clone());
        child.snapshot = Some(commit.clone());

        assert_eq!(child.base.as_deref(), Some(commit.as_str()));
        assert!(changes(&child).await.unwrap().files.is_empty());
        // The child starts with the owner's uncommitted edit, and the owner keeps its own.
        assert_eq!(std::fs::read_to_string(PathBuf::from(&child.path).join("a.txt")).unwrap(), "owner's edit\n");
        assert_eq!(std::fs::read_to_string(owner_dir.join("a.txt")).unwrap(), "owner's edit\n");

        let r = format!("refs/atelier/snap/{}", child_id.as_str());
        assert!(!out(Path::new(&project.path), &["for-each-ref", &r]).await.is_empty());
        assert!(release(&project, &child).await.unwrap());
        assert!(out(Path::new(&project.path), &["for-each-ref", &r]).await.is_empty());
    }

    #[tokio::test]
    async fn seed_copies_dependencies_from_the_source_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let project = repo_with_history(tmp.path()).await;
        let wt = tmp.path().join("worktrees");
        let owner =
            prepare(&wt, &project, &SessionId::generate(), "owner", WorkspaceMode::Worktree, &[], None).await.unwrap();
        let owner_dir = PathBuf::from(&owner.path);
        // Only the owner has this: a build product the project checkout doesn't.
        std::fs::create_dir_all(owner_dir.join("target")).unwrap();
        std::fs::write(owner_dir.join("target/owner-only"), "b").unwrap();

        let commit = snapshot(&owner, "owner", &SessionId::generate()).await.unwrap();
        let (child, _) =
            claim(&wt, &project, &SessionId::generate(), "child", WorkspaceMode::Worktree, &[], Some(&commit))
                .await
                .unwrap();
        seed(&owner_dir, Path::new(&child.path)).await;
        // Copy-on-write cloning is best effort (see `seed`); macOS has `cp -c`, so there it must work.
        if cfg!(target_os = "macos") {
            assert!(Path::new(&child.path).join("target/owner-only").exists());
        }
    }

    #[tokio::test]
    async fn sharing_uses_the_owners_directory_and_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let project = repo_with_history(tmp.path()).await;
        let wt = tmp.path().join("worktrees");
        let owner =
            prepare(&wt, &project, &SessionId::generate(), "owner", WorkspaceMode::Worktree, &[], None).await.unwrap();
        let owner_id = SessionId::generate();
        let shared = share(&owner, &owner_id);
        assert_eq!(shared.mode, WorkspaceMode::Shared);
        assert_eq!(
            (shared.path, shared.branch, shared.base),
            (owner.path.clone(), owner.branch.clone(), owner.base.clone())
        );
        assert_eq!(shared.from.as_ref(), Some(&owner_id));
        assert!(claim(&wt, &project, &SessionId::generate(), "x", WorkspaceMode::Shared, &[], None).await.is_err());
    }
}
