//! Mirrors every project's `.atelier/` into its `p/<project>` stream as `file.changed` events.
//!
//! [`ingest`] is the one way a file enters the log: it compares the file on disk with the `files` projection and
//! appends only if they differ, so repeated notifications, rescans and RPC writes of the same file are no-ops.
//! [`start`] scans every project once (catching edits made while the server was down), then watches them live.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_core::domain::{FileChanged, ProjectAdded, ProjectRemoved};
use atelier_core::{Actor, Event, NewEvent, ProjectId, stream};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{App, views};

/// The directory of authored files, relative to a project root.
pub const DIR: &str = ".atelier";
/// Bigger files are not mirrored: their text would ride in every event.
const MAX_LEN: u64 = 256 * 1024;
/// Editors write a file as several syscalls; wait for the burst to end.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Whether a project-relative path (starting with `.atelier/`) is a file we mirror, judging by its names only.
pub fn counts(rel: &str) -> bool {
    rel.split('/').skip(1).all(|name| {
        !name.is_empty()
            && !name.starts_with('.')
            && !name.ends_with('~')
            && !name.ends_with(".swp")
            && !name.ends_with(".tmp")
    })
}

/// One mutex per project, so a watcher ingest and an RPC write of the same file can't both append.
#[derive(Default)]
pub struct Locks(Mutex<HashMap<ProjectId, Arc<tokio::sync::Mutex<()>>>>);

impl Locks {
    async fn lock(&self, project: &ProjectId) -> tokio::sync::OwnedMutexGuard<()> {
        let m = self.0.lock().unwrap().entry(project.clone()).or_default().clone();
        m.lock_owned().await
    }
}

async fn root_of(app: &App, project: &ProjectId) -> Result<PathBuf> {
    let id = project.0.clone();
    let row = app.store.read(move |c| views::projects::get(c, &id)).await?;
    Ok(row.context("no such project")?.path.into())
}

/// Bring `rel_path` (relative to the project root, `/`-separated, under `.atelier/`) in line with the disk.
/// Appends one `file.changed` if the file is new, changed or gone; `None` if the log already has it.
pub async fn ingest(app: &App, project: &ProjectId, rel_path: &str, actor: Actor) -> Result<Option<Arc<Event>>> {
    let root = root_of(app, project).await?;
    ingest_in(app, project, &root, rel_path, actor).await
}

async fn ingest_in(app: &App, project: &ProjectId, root: &Path, rel: &str, actor: Actor) -> Result<Option<Arc<Event>>> {
    anyhow::ensure!(
        rel.strip_prefix(DIR).is_some_and(|r| r.starts_with('/')) && !rel.split('/').any(|s| s == ".."),
        "{rel} is not under {DIR}/"
    );
    let _guard = app.file_locks.lock(project).await;
    let text = read_text(&root.join(rel), rel).await;
    let (id, path) = (project.0.clone(), rel.to_owned());
    let known = app.store.read(move |c| views::files::hash_of(c, &id, &path)).await?;
    let unchanged = match (&text, known) {
        (Some(t), Some(h)) => views::files::hash(t) == h,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return Ok(None);
    }
    let body = FileChanged { path: rel.to_owned(), text };
    Ok(Some(app.store.append_one(NewEvent::new(stream::project(project), actor, &body)).await?))
}

/// The file's text, or `None` if it's gone or not something we mirror.
async fn read_text(abs: &Path, rel: &str) -> Option<String> {
    if !counts(rel) {
        return None;
    }
    let meta = tokio::fs::symlink_metadata(abs).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > MAX_LEN {
        tracing::warn!("{rel} is over {} KB; not mirrored", MAX_LEN / 1024);
        return None;
    }
    String::from_utf8(tokio::fs::read(abs).await.ok()?).ok()
}

/// Ingest everything at or under `rel` on disk, and delete what the log has there but the disk doesn't.
pub async fn rescan(app: &App, project: &ProjectId, rel: &str) -> Result<usize> {
    let root = root_of(app, project).await?;
    let (abs, start) = (root.join(rel), rel.to_owned());
    let mut paths: BTreeSet<String> = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        walk(&abs, &start, &mut out);
        out
    })
    .await?
    .into_iter()
    .collect();
    let (id, prefix) = (project.0.clone(), rel.to_owned());
    paths.extend(app.store.read(move |c| views::files::paths_under(c, &id, &prefix)).await?);
    let mut appended = 0;
    for p in paths {
        appended += ingest_in(app, project, &root, &p, Actor::Atelier).await?.is_some() as usize;
    }
    Ok(appended)
}

fn walk(abs: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(meta) = std::fs::symlink_metadata(abs) else { return };
    if meta.is_file() {
        out.push(rel.to_owned());
    } else if meta.is_dir()
        && let Ok(entries) = std::fs::read_dir(abs)
    {
        for e in entries.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            if !name.starts_with('.') {
                walk(&e.path(), &format!("{rel}/{name}"), out);
            }
        }
    }
}

struct Watched {
    root: PathBuf,
    watcher: RecommendedWatcher,
    /// Whether `.atelier/` itself is watched (it may not exist yet; the root is watched for it to appear).
    deep: bool,
}

struct Watches {
    app: App,
    tx: mpsc::UnboundedSender<(ProjectId, PathBuf)>,
    projects: HashMap<ProjectId, Watched>,
}

/// Scan every project, then keep them mirrored until the server stops. Returns once the scan is done.
pub async fn start(app: &App) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    // Subscribe before listing so a project added in between is seen by one or both (`add` is idempotent).
    let feed = app.store.subscribe();
    let mut w = Watches { app: app.clone(), tx, projects: HashMap::new() };
    for p in app.store.read(views::projects::list).await? {
        w.add(p.id, p.path.into()).await;
    }
    tokio::spawn(w.run(rx, feed));
    Ok(())
}

impl Watches {
    /// Watch first, then scan: a change landing in between is then caught twice rather than never.
    async fn add(&mut self, id: ProjectId, root: PathBuf) {
        if self.projects.contains_key(&id) {
            return;
        }
        let (tx, pid) = (self.tx.clone(), id.clone());
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(ev) if !matches!(ev.kind, EventKind::Access(_)) => {
                for p in ev.paths {
                    let _ = tx.send((pid.clone(), p));
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("file watcher: {e}"),
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => return tracing::warn!("can't watch {}: {e}", root.display()),
        };
        if let Err(e) = watcher.watch(&root, RecursiveMode::NonRecursive) {
            tracing::warn!("can't watch {}: {e}", root.display());
        }
        let mut w = Watched { root, watcher, deep: false };
        sync_deep(&mut w);
        self.projects.insert(id.clone(), w);
        if let Err(e) = rescan(&self.app, &id, DIR).await {
            tracing::warn!("scanning {DIR}/ of {id}: {e:#}");
        }
    }

    async fn run(
        mut self,
        mut rx: mpsc::UnboundedReceiver<(ProjectId, PathBuf)>,
        mut feed: tokio::sync::broadcast::Receiver<Arc<atelier_store::Commit>>,
    ) {
        use tokio::sync::broadcast::error::RecvError;
        let mut stopping = self.app.stopping.subscribe();
        let mut pending: HashMap<(ProjectId, String), Instant> = HashMap::new();
        loop {
            let next = pending.values().min().copied();
            tokio::select! {
                _ = async { let _ = stopping.wait_for(|s| *s).await; } => return,
                Some((id, path)) = rx.recv() => {
                    if let Some(rel) = self.relative(&id, &path) {
                        pending.insert((id, rel), Instant::now() + DEBOUNCE);
                    }
                }
                commit = feed.recv() => match commit {
                    Ok(c) => {
                        for ev in &c.events {
                            if let Some(Ok(p)) = ev.parse::<ProjectAdded>() {
                                self.add(p.id, p.path.into()).await;
                            } else if let Some(Ok(p)) = ev.parse::<ProjectRemoved>() {
                                self.projects.remove(&p.id);
                                pending.retain(|(id, _), _| *id != p.id);
                            }
                        }
                    }
                    Err(RecvError::Lagged(_)) => self.resync().await,
                    Err(RecvError::Closed) => return,
                },
                _ = tokio::time::sleep_until(next.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600))), if next.is_some() => {
                    let now = Instant::now();
                    let due: Vec<_> = pending.iter().filter(|(_, at)| **at <= now).map(|(k, _)| k.clone()).collect();
                    for key in due {
                        pending.remove(&key);
                        self.settle(&key.0, &key.1).await;
                    }
                }
            }
        }
    }

    /// After missing commits: make the watched set match the projects table.
    async fn resync(&mut self) {
        let Ok(rows) = self.app.store.read(views::projects::list).await else { return };
        self.projects.retain(|id, _| rows.iter().any(|r| r.id == *id));
        for r in rows {
            self.add(r.id, r.path.into()).await;
        }
    }

    /// `path` relative to the project root as `/`-joined text, if it's at or under `.atelier/`.
    fn relative(&self, id: &ProjectId, path: &Path) -> Option<String> {
        let w = self.projects.get(id)?;
        let rel = path.strip_prefix(&w.root).ok()?;
        let rel = rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<Vec<_>>>()?.join("/");
        (rel == DIR || rel.strip_prefix(DIR).is_some_and(|r| r.starts_with('/'))).then_some(rel)
    }

    async fn settle(&mut self, id: &ProjectId, rel: &str) {
        let Some(w) = self.projects.get_mut(id) else { return };
        let was_deep = w.deep;
        sync_deep(w);
        // `.atelier/` appearing or disappearing means everything under it may have changed.
        let rel = if w.deep != was_deep { DIR } else { rel };
        if let Err(e) = rescan(&self.app, id, rel).await {
            tracing::warn!("ingesting {rel} of {id}: {e:#}");
        }
    }
}

/// Make the recursive watch on `.atelier/` match whether the directory exists.
fn sync_deep(w: &mut Watched) {
    let dir = w.root.join(DIR);
    let exists = dir.is_dir();
    if exists && !w.deep {
        w.deep = w.watcher.watch(&dir, RecursiveMode::Recursive).is_ok();
    } else if !exists && w.deep {
        let _ = w.watcher.unwatch(&dir);
        w.deep = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_rules() {
        for ok in [".atelier/tickets/AT-035.md", ".atelier/board.toml", ".atelier/a/b/c.txt"] {
            assert!(counts(ok), "{ok}");
        }
        for skip in [
            ".atelier/.DS_Store",
            ".atelier/tickets/.hidden.md",
            ".atelier/a.md~",
            ".atelier/a.md.swp",
            ".atelier/a.tmp",
            ".atelier/.git/config",
            ".atelier//x",
        ] {
            assert!(!counts(skip), "{skip}");
        }
    }
}
