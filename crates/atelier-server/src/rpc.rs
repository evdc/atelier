//! Command handlers. Each validates, appends events, and returns a small
//! result (usually an id). State reaches clients through subs and watches,
//! never through RPC results.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use atelier_core::domain::{AgentKind, ProjectAdded, ProjectRemoved, SessionArchived, SessionCreated};
use atelier_core::{Actor, NewEvent, SessionId, stream};
use atelier_proto::{Call, ProjectRow, SessionRow};
use serde_json::{Value, json};

use crate::{App, views, workspace};

pub async fn handle(app: &App, call: Call) -> Result<Value> {
    match call {
        Call::ProjectAdd { path, name } => project_add(app, path, name).await,
        Call::ProjectRemove { project } => {
            let p = project_row(app, project.as_str()).await?;
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
            let ws = workspace::prepare(&app.config.worktrees_dir(), &p, &id, &title, workspace).await?;
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
        Call::SessionStop { session } => {
            app.supervisor.stop(&session);
            Ok(Value::Null)
        }
        Call::SessionArchive { session } => {
            let s = session_row(app, &session).await?;
            app.supervisor.stop(&s.id);
            app.store.append_one(NewEvent::new(&s.stream, Actor::User, &SessionArchived {})).await?;
            Ok(Value::Null)
        }
    }
}

async fn project_add(app: &App, path: String, name: Option<String>) -> Result<Value> {
    let path = expand_home(&path);
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

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| p.into()),
        None => p.into(),
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
