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
    AgentKind, ProjectAdded, ProjectRemoved, SessionArchived, SessionCreated, SessionRenamed, WorkspaceMode,
    WorkspaceRemoved,
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
        Call::SessionCreate { project, prompt, workspace, title, model } => {
            let p = project_row(app, project.as_str()).await?;
            if prompt.trim().is_empty() {
                bail!("the first message can't be empty");
            }
            let id = SessionId::generate();
            let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| title_from(&prompt));
            let in_use: Vec<String> = {
                let pid = p.id.0.clone();
                let rows = app.store.read(move |c| views::sessions::list(c, Some(&pid))).await?;
                rows.into_iter().filter(|s| !s.archived).map(|s| s.workspace.path).collect()
            };
            let ws = workspace::prepare(&app.config.worktrees_dir(), &p, &id, &title, workspace, &in_use).await?;
            let s_stream = stream::session(&p.id, &id);
            let created = SessionCreated {
                id: id.clone(),
                project: p.id,
                title,
                agent: AgentKind::ClaudeCode,
                workspace: ws,
                model,
            };
            app.store.append_one(NewEvent::new(&s_stream, Actor::User, &created)).await?;
            let s = session_row(app, &id).await?;
            app.supervisor.send(&s, prompt).await?;
            Ok(json!({ "id": id, "stream": s_stream }))
        }
        Call::SessionSend { session, text } => {
            if text.trim().is_empty() {
                bail!("message is empty");
            }
            let s = session_row(app, &session).await?;
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
            if s.workspace.mode == WorkspaceMode::Worktree && !s.archived {
                let p = project_row(app, s.project.as_str()).await?;
                match workspace::unsafe_to_remove(&p, &s.workspace).await {
                    Ok(None) => match workspace::release(&p, &s.workspace).await {
                        Ok(pooled) => {
                            let gone = WorkspaceRemoved {
                                path: s.workspace.path.clone(),
                                branch: s.workspace.branch.clone(),
                                pooled,
                            };
                            app.store.append_one(NewEvent::new(&s.stream, Actor::User, &gone)).await?;
                            result.removed = true;
                        }
                        Err(e) => result.kept = Some(format!("couldn't remove the worktree: {e}")),
                    },
                    Ok(Some(why)) => result.kept = Some(why),
                    Err(e) => result.kept = Some(format!("couldn't inspect the worktree: {e}")),
                }
            }
            Ok(serde_json::to_value(result)?)
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
        Call::FsList { path, hidden } => Ok(serde_json::to_value(browse::list(path, hidden.unwrap_or(false)).await?)?),
        Call::FsSuggest {} => Ok(serde_json::to_value(browse::suggest().await?)?),
    }
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
fn title_from(prompt: &str) -> String {
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
