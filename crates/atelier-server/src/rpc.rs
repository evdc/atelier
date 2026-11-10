//! Command handlers. Each validates, appends events, and returns a small
//! result (usually an id). State reaches clients through subs and watches,
//! never through RPC results.
//!
//! Deliberate exceptions: `session.changes`, `session.diff`, `fs.list` and
//! `fs.suggest` return a computed snapshot of the filesystem (git's view of a
//! worktree, a directory listing). That's a read, like `cat`, not state of the
//! system; nothing is logged for it.

use anyhow::{Context, Result, bail};
use atelier_core::domain::{
    AgentKind, ProjectAdded, ProjectRemoved, SessionArchived, SessionCreated, SessionRenamed, SessionUnarchived,
    WorkspaceMode, WorkspaceRemoved,
};
use atelier_core::{Actor, NewEvent, SessionId, stream};
use atelier_proto::{Archived, Call, ProjectRow, SessionRow, SessionStatus};
use serde_json::{Value, json};

use crate::{App, browse, views, workspace};

const MAX_TITLE_CHARS: usize = 200;

pub async fn handle(app: &App, call: Call) -> Result<Value> {
    match call {
        Call::ProjectAdd { path, name } => project_add(app, path, name).await,
        Call::ProjectRemove { project } => {
            let p = project_row(app, project.as_str()).await?;
            let id = p.id.0.clone();
            let live = app.store.read(move |c| views::sessions::list(c, Some(&id))).await?;
            if live.iter().any(|s| s.alive) {
                bail!("{} has running agents; stop them first", p.name);
            }
            app.store.append_one(NewEvent::new(stream::GLOBAL, Actor::User, &ProjectRemoved { id: p.id })).await?;
            Ok(Value::Null)
        }
        Call::SessionCreate { project, prompt, workspace, title, model, from } => {
            let p = project_row(app, project.as_str()).await?;
            if prompt.trim().is_empty() {
                bail!("the first message can't be empty");
            }
            let source = match (&from, workspace) {
                (None, WorkspaceMode::Shared) => bail!("sharing a worktree needs the session to share"),
                (None, _) => None,
                (Some(_), WorkspaceMode::Direct) => {
                    bail!("the project directory has no worktree of its own to share or branch from")
                }
                (Some(f), _) => Some(source_session(app, &p, f).await?),
            };
            let id = SessionId::generate();
            let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| title_from(&prompt));
            let in_use: Vec<String> = {
                let pid = p.id.0.clone();
                let rows = app.store.read(move |c| views::sessions::list(c, Some(&pid))).await?;
                rows.into_iter().filter(|s| !s.archived).map(|s| s.workspace.path).collect()
            };
            // (workspace, whether to seed it from `seed_from`)
            let (ws, seed_from) = match &source {
                Some(owner) if workspace == WorkspaceMode::Shared => {
                    (workspace::share(&owner.workspace, &owner.id), None)
                }
                Some(owner) => {
                    // Branch from a snapshot of the owner's working tree; the owner itself is never touched.
                    let snap = workspace::snapshot(&owner.workspace, &title, &id).await?;
                    match workspace::claim(
                        &app.config.worktrees_dir(),
                        &p,
                        &id,
                        &title,
                        workspace,
                        &in_use,
                        Some(&snap),
                    )
                    .await
                    {
                        Ok((mut ws, fresh)) => {
                            ws.from = Some(owner.id.clone());
                            ws.snapshot = Some(snap);
                            (ws, fresh.then(|| owner.workspace.path.clone()))
                        }
                        Err(e) => {
                            workspace::drop_snapshot(std::path::Path::new(&p.path), &snap).await;
                            return Err(e);
                        }
                    }
                }
                None => {
                    let (ws, fresh) =
                        workspace::claim(&app.config.worktrees_dir(), &p, &id, &title, workspace, &in_use, None)
                            .await?;
                    (ws, fresh.then(|| p.path.clone()))
                }
            };
            let ws_path = ws.path.clone();
            let s_stream = stream::session(&p.id, &id);
            let created = SessionCreated {
                id: id.clone(),
                project: p.id,
                title,
                agent: AgentKind::ClaudeCode,
                workspace: ws,
                model,
                origin: None,
                started_at: None,
            };
            app.store.append_one(NewEvent::new(&s_stream, Actor::User, &created)).await?;
            let s = session_row(app, &id).await?;
            // Seeding can take a long time on a big repo. The session exists already, so answer now
            // and start the agent once the worktree is warm.
            let app = app.clone();
            tokio::spawn(async move {
                if let Some(src) = seed_from {
                    workspace::seed(std::path::Path::new(&src), std::path::Path::new(&ws_path)).await;
                }
                if let Err(e) = app.supervisor.send(&s, prompt).await {
                    tracing::error!("starting session {}: {e:#}", s.id.as_str());
                }
            });
            Ok(json!({ "id": id, "stream": s_stream }))
        }
        Call::SessionSend { session, text } => {
            if text.trim().is_empty() {
                bail!("message is empty");
            }
            let s = session_row(app, &session).await?;
            // Archiving released the worktree (its directory may be pooled for another session), and
            // un-archiving doesn't bring it back. Refuse before the message is logged.
            let id = s.id.as_str().to_owned();
            if app.store.read(move |c| views::sessions::worktree_removed(c, &id)).await? {
                bail!("its worktree was removed when it was archived; start a new session");
            }
            app.supervisor.send(&s, text).await?;
            Ok(Value::Null)
        }
        Call::SessionInterrupt { session } => {
            app.supervisor.interrupt(&session_row(app, &session).await?).await?;
            Ok(Value::Null)
        }
        Call::SessionAnswer { session, request_id, decision } => {
            app.supervisor.answer(&session_row(app, &session).await?, request_id, decision).await?;
            Ok(Value::Null)
        }
        Call::SessionRename { session, title } => {
            let title = title.trim().to_owned();
            if title.is_empty() {
                bail!("the title can't be empty");
            }
            if title.chars().count() > MAX_TITLE_CHARS {
                bail!("the title is longer than {MAX_TITLE_CHARS} characters");
            }
            let s = session_row(app, &session).await?;
            app.store.append_one(NewEvent::new(&s.stream, Actor::User, &SessionRenamed { title })).await?;
            Ok(Value::Null)
        }
        Call::SessionStop { session } => {
            app.supervisor.stop(&session);
            Ok(Value::Null)
        }
        Call::SessionArchive { session } => {
            let s = session_row(app, &session).await?;
            app.supervisor.stop(&s.id);
            app.store.append_one(NewEvent::new(&s.stream, Actor::User, &SessionArchived {})).await?;
            let mut result = Archived { removed: false, kept: None };
            if !s.archived {
                let p = project_row(app, s.project.as_str()).await?;
                match s.workspace.mode {
                    WorkspaceMode::Worktree => match readers_of(app, &s).await?.first() {
                        // The worktree outlives its owner while someone reads it; the last reader to leave
                        // releases it (see the `Shared` arm).
                        Some(r) => result.kept = Some(format!("{} is reading this worktree", r.title)),
                        None => result = remove_worktree(app, &p, &s).await?,
                    },
                    WorkspaceMode::Shared => {
                        // A reader never touches the worktree; the last one out releases an archived owner's.
                        let owner_id = s.workspace.from.clone().context("a shared workspace without its owner")?;
                        let owner = session_row(app, &owner_id).await?;
                        let oid = owner_id.as_str().to_owned();
                        let gone = app.store.read(move |c| views::sessions::worktree_removed(c, &oid)).await?;
                        if owner.archived && !gone && readers_of(app, &owner).await?.is_empty() {
                            result = remove_worktree(app, &p, &owner).await?;
                        }
                    }
                    WorkspaceMode::Direct => {}
                }
            }
            Ok(serde_json::to_value(result)?)
        }
        Call::SessionUnarchive { session } => {
            let s = session_row(app, &session).await?;
            if s.archived {
                app.store.append_one(NewEvent::new(&s.stream, Actor::User, &SessionUnarchived {})).await?;
            }
            Ok(Value::Null)
        }
        Call::SessionChanges { session } => {
            let s = session_row(app, &session).await?;
            Ok(serde_json::to_value(workspace::changes(&s.workspace).await?)?)
        }
        Call::SessionMerge { session } => {
            let s = session_row(app, &session).await?;
            if s.archived {
                bail!("session is archived");
            }
            if s.status == SessionStatus::Running || s.status == SessionStatus::Waiting {
                bail!("wait for the agent to finish its turn before merging");
            }
            let p = project_row(app, s.project.as_str()).await?;
            let r = workspace::merge(&p, &s.workspace, &s.title).await?;
            if !r.merged {
                let list = r.conflicts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n");
                let text = format!(
                    "Merging `{}` into this branch hit conflicts, left in progress in the worktree:\n{list}\n\n\
                     Resolve them (keep both sides' intent), `git add` the files and run `git commit --no-edit`. \
                     Don't abort the merge.",
                    r.target
                );
                app.supervisor.send(&s, text).await?;
            }
            Ok(serde_json::to_value(r)?)
        }
        Call::SessionDiff { session, path } => {
            let s = session_row(app, &session).await?;
            Ok(json!({ "patch": workspace::diff(&s.workspace, &path).await? }))
        }
        Call::ProjectImport { project } => {
            let p = project_row(app, project.as_str()).await?;
            Ok(serde_json::to_value(crate::import::project(app, &p).await?)?)
        }
        Call::FsList { path, hidden } => Ok(serde_json::to_value(browse::list(path, hidden.unwrap_or(false)).await?)?),
        Call::FsSuggest {} => Ok(serde_json::to_value(browse::suggest().await?)?),
    }
}

/// The session a new session starts from: `from` itself, or the owner of the worktree when `from` is a reader.
async fn source_session(app: &App, p: &ProjectRow, from: &SessionId) -> Result<SessionRow> {
    let s = session_row(app, from).await?;
    if s.project.as_str() != p.id.as_str() {
        bail!("{} is in another project", s.title);
    }
    if s.archived {
        bail!("{} is archived", s.title);
    }
    let owner = if s.workspace.mode == WorkspaceMode::Shared {
        let owner_id = s.workspace.from.clone().context("a shared workspace without its owner")?;
        let owner = session_row(app, &owner_id).await?;
        if owner.archived {
            bail!("{} is archived, so its worktree is only kept for the sessions reading it", owner.title);
        }
        owner
    } else {
        s
    };
    if owner.workspace.mode != WorkspaceMode::Worktree {
        bail!(
            "{} works in the project directory, which has no worktree of its own to share or branch from",
            owner.title
        );
    }
    Ok(owner)
}

/// Live sessions reading `owner`'s worktree.
async fn readers_of(app: &App, owner: &SessionRow) -> Result<Vec<SessionRow>> {
    let (pid, oid) = (owner.project.as_str().to_owned(), owner.id.as_str().to_owned());
    let rows = app.store.read(move |c| views::sessions::list(c, Some(&pid))).await?;
    Ok(rows
        .into_iter()
        .filter(|r| {
            !r.archived
                && r.workspace.mode == WorkspaceMode::Shared
                && r.workspace.from.as_ref().is_some_and(|f| f.as_str() == oid)
        })
        .collect())
}

/// Deletes the worktree of `s` if nothing in it would be lost, and records the removal on `s`'s stream.
async fn remove_worktree(app: &App, p: &ProjectRow, s: &SessionRow) -> Result<Archived> {
    let kept = match workspace::unsafe_to_remove(p, &s.workspace).await {
        Ok(None) => match workspace::release(p, &s.workspace).await {
            Ok(pooled) => {
                let gone =
                    WorkspaceRemoved { path: s.workspace.path.clone(), branch: s.workspace.branch.clone(), pooled };
                app.store.append_one(NewEvent::new(&s.stream, Actor::User, &gone)).await?;
                return Ok(Archived { removed: true, kept: None });
            }
            Err(e) => format!("couldn't remove the worktree: {e}"),
        },
        Ok(Some(why)) => why,
        Err(e) => format!("couldn't inspect the worktree: {e}"),
    };
    Ok(Archived { removed: false, kept: Some(kept) })
}

async fn project_add(app: &App, path: String, name: Option<String>) -> Result<Value> {
    let path = browse::expand_home(&path);
    let path = tokio::fs::canonicalize(&path).await.with_context(|| format!("{} doesn't exist", path.display()))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    let path_s = path.to_string_lossy().into_owned();
    let existing = {
        let p = path_s.clone();
        app.store.read(move |c| views::projects::find_by_path(c, &p)).await?
    };
    if let Some(p) = existing {
        return Ok(json!({ "id": p.id, "existing": true }));
    }
    let name = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path_s.clone());
    let added = ProjectAdded {
        id: atelier_core::ProjectId::generate(),
        name,
        path: path_s,
        is_git: workspace::is_git(&path).await,
    };
    let id = added.id.clone();
    app.store.append_one(NewEvent::new(stream::GLOBAL, Actor::User, &added)).await?;
    Ok(json!({ "id": id }))
}

async fn project_row(app: &App, id: &str) -> Result<ProjectRow> {
    let id = id.to_owned();
    app.store.read(move |c| views::projects::get(c, &id)).await?.context("no such project")
}

async fn session_row(app: &App, id: &SessionId) -> Result<SessionRow> {
    let id = id.clone();
    app.store.read(move |c| views::sessions::get(c, id.as_str())).await?.context("no such session")
}

/// First line of the prompt, cut at a word boundary near 60 chars.
pub(crate) fn title_from(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled");
    if line.chars().count() <= 60 {
        return line.to_owned();
    }
    let cut: String = line.chars().take(60).collect();
    match cut.rfind(' ') {
        Some(i) if i > 30 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_from("\n  Fix the bug\nmore"), "Fix the bug");
        let long = "Rate limit login attempts per IP and per account, but never lock anyone out for good";
        let t = title_from(long);
        assert!(t.ends_with('…') && t.chars().count() <= 61, "{t}");
    }
}
