//! Command handlers. Each validates, appends events, and returns a small
//! result (usually an id). State reaches clients through subs and watches,
//! never through RPC results.
//!
//! Deliberate exceptions: `session.changes`, `session.diff`, `fs.list` and
//! `fs.suggest` return a computed snapshot of the filesystem (git's view of a
//! worktree, a directory listing). That's a read, like `cat`, not state of the
//! system; nothing is logged for it.
//!
//! Every call runs *as* someone: the person (`Actor::User`) or an agent (`Actor::Agent`, from the MCP server).
//! The actor and the event that caused the call go onto everything the call appends. Agents may not make
//! every call ([`Call::agent_allowed`]).

use anyhow::{Context, Result, bail};
use atelier_core::domain::{
    AgentKind, ProjectAdded, ProjectRemoved, SessionArchived, SessionCreated, SessionRenamed, SessionUnarchived,
    WorkspaceMode, WorkspaceRemoved,
};
use atelier_core::{Actor, NewEvent, Seq, SessionId, stream};
use atelier_proto::{Archived, Call, ProjectRow, SessionRow, SessionStatus};
use serde_json::{Value, json};

use crate::{App, browse, views, workspace};

const MAX_TITLE_CHARS: usize = 200;

pub async fn handle(app: &App, actor: Actor, cause: Option<Seq>, call: Call) -> Result<Value> {
    if matches!(actor, Actor::Agent(_)) && !call.agent_allowed() {
        bail!("only a person can do that");
    }
    match call {
        Call::ProjectAdd { path, name } => project_add(app, &actor, cause, path, name).await,
        Call::ProjectRemove { project } => {
            let p = project_row(app, project.as_str()).await?;
            let id = p.id.0.clone();
            let live = app.store.read(move |c| views::sessions::list(c, Some(&id))).await?;
            if live.iter().any(|s| s.alive) {
                bail!("{} has running agents; stop them first", p.name);
            }
            app.store.append_one(event(stream::GLOBAL, &actor, cause, &ProjectRemoved { id: p.id })).await?;
            Ok(Value::Null)
        }
        Call::SessionCreate { project, prompt, workspace, title, model, from, parent } => {
            let p = project_row(app, project.as_str()).await?;
            if let Some(parent) = &parent {
                let parent_row = session_row(app, parent).await?;
                if parent_row.project.as_str() != p.id.as_str() {
                    bail!("{} is in another project", parent_row.title);
                }
                if parent_row.archived {
                    bail!("{} is archived", parent_row.title);
                }
                let pid = p.id.0.clone();
                let rows = app.store.read(move |c| views::sessions::list(c, Some(&pid))).await?;
                crate::spawn::check_limits(&rows, parent, &app.config.spawn).map_err(anyhow::Error::msg)?;
            }
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
            let prompt = match &source {
                Some(owner) if workspace == WorkspaceMode::Worktree => {
                    branch_prompt(owner.status == SessionStatus::Running, &prompt)
                }
                _ => prompt,
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
                parent,
            };
            app.store.append_one(event(&s_stream, &actor, cause, &created)).await?;
            let s = session_row(app, &id).await?;
            // Seeding can take a long time on a big repo. The session exists already, so answer now
            // and start the agent once the worktree is warm.
            let app = app.clone();
            tokio::spawn(async move {
                if let Some(src) = seed_from {
                    workspace::seed(std::path::Path::new(&src), std::path::Path::new(&ws_path)).await;
                }
                if let Err(e) = app.supervisor.send(&s, actor, cause, prompt).await {
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
            app.supervisor.send(&s, actor.clone(), cause, text).await?;
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
            app.store.append_one(event(&s.stream, &actor, cause, &SessionRenamed { title })).await?;
            Ok(Value::Null)
        }
        Call::SessionStop { session } => {
            app.supervisor.stop(&session);
            Ok(Value::Null)
        }
        Call::SessionArchive { session } => {
            // Held from the readers check to the release, so two archives of one worktree can't both release it.
            let _archive = ARCHIVE_LOCK.lock().await;
            let s = session_row(app, &session).await?;
            app.supervisor.stop(&s.id);
            app.store.append_one(event(&s.stream, &actor, cause, &SessionArchived {})).await?;
            let mut result = Archived { removed: false, kept: None };
            if !s.archived {
                let p = project_row(app, s.project.as_str()).await?;
                match s.workspace.mode {
                    WorkspaceMode::Worktree => match readers_of(app, &s).await?.first() {
                        // The worktree outlives its owner while someone reads it; the last reader to leave
                        // releases it (see the `Shared` arm).
                        Some(r) => result.kept = Some(format!("{} is reading this worktree", r.title)),
                        None => result = remove_worktree(app, &actor, cause, &p, &s).await?,
                    },
                    WorkspaceMode::Shared => {
                        // A reader never touches the worktree; the last one out releases an archived owner's.
                        let owner_id = s.workspace.from.clone().context("a shared workspace without its owner")?;
                        let owner = session_row(app, &owner_id).await?;
                        if owner.archived && readers_of(app, &owner).await?.is_empty() {
                            result = remove_worktree(app, &actor, cause, &p, &owner).await?;
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
                app.store.append_one(event(&s.stream, &actor, cause, &SessionUnarchived {})).await?;
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
                app.supervisor.send(&s, actor.clone(), cause, text).await?;
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
pub(crate) async fn source_session(app: &App, p: &ProjectRow, from: &SessionId) -> Result<SessionRow> {
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

fn event<B: atelier_core::EventBody>(
    stream: impl Into<String>,
    actor: &Actor,
    cause: Option<Seq>,
    body: &B,
) -> NewEvent {
    let mut e = NewEvent::new(stream, actor.clone(), body);
    e.cause = cause;
    e
}

/// Deletes the worktree of `s` if nothing in it would be lost, and records the removal on `s`'s stream.
async fn remove_worktree(
    app: &App,
    actor: &Actor,
    cause: Option<Seq>,
    p: &ProjectRow,
    s: &SessionRow,
) -> Result<Archived> {
    // Re-checked under the archive lock: the worktree may already have gone with an earlier archive.
    let id = s.id.as_str().to_owned();
    if app.store.read(move |c| views::sessions::worktree_removed(c, &id)).await? {
        return Ok(Archived { removed: false, kept: None });
    }
    let kept = match workspace::unsafe_to_remove(p, &s.workspace).await {
        Ok(None) => match workspace::release(p, &s.workspace).await {
            Ok(pooled) => {
                let gone =
                    WorkspaceRemoved { path: s.workspace.path.clone(), branch: s.workspace.branch.clone(), pooled };
                app.store.append_one(event(&s.stream, actor, cause, &gone)).await?;
                return Ok(Archived { removed: true, kept: None });
            }
            Err(e) => format!("couldn't remove the worktree: {e}"),
        },
        Ok(Some(why)) => why,
        Err(e) => format!("couldn't inspect the worktree: {e}"),
    };
    Ok(Archived { removed: false, kept: Some(kept) })
}

/// Serializes session archives, so the last reader and the owner can't both release the same worktree.
static ARCHIVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Put in front of a branch's first message: the branch's base is a snapshot, so merging it back can conflict.
const BRANCH_NOTE: &str = "This session's branch starts at a snapshot of its source's working tree, not at a commit \
    the source made. If the source commits the same edits and this branch is merged back into it, the merge may \
    conflict; if it does, keep both sides' intent.";
/// Added when the source was mid-turn: some of its files may have been half-written.
const RUNNING_NOTE: &str = "The source was running when this snapshot was taken, so some of its files may have been \
    mid-edit.";

fn branch_prompt(source_running: bool, prompt: &str) -> String {
    let running = if source_running { format!("{RUNNING_NOTE}\n\n") } else { String::new() };
    format!("{running}{BRANCH_NOTE}\n\n{prompt}")
}

async fn project_add(
    app: &App,
    actor: &Actor,
    cause: Option<Seq>,
    path: String,
    name: Option<String>,
) -> Result<Value> {
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
    app.store.append_one(event(stream::GLOBAL, actor, cause, &added)).await?;
    Ok(json!({ "id": id }))
}

pub(crate) async fn project_row(app: &App, id: &str) -> Result<ProjectRow> {
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

    #[test]
    fn branch_prompt_adds_the_running_note_only_when_the_source_was_running() {
        let idle = branch_prompt(false, "try it");
        assert_eq!(idle, format!("{BRANCH_NOTE}\n\ntry it"));
        let running = branch_prompt(true, "try it");
        assert_eq!(running, format!("{RUNNING_NOTE}\n\n{BRANCH_NOTE}\n\ntry it"));
    }
}
