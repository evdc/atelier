//! The tool table. Output is text for the model, not JSON.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use atelier_core::{Event, Seq, SessionId};
use atelier_proto::SessionRow;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

use super::BoxFut;
use crate::App;
use crate::views::{self, text};

pub enum ToolError {
    /// The arguments don't fit the tool's schema: a protocol error.
    Params(String),
    /// The tool ran and failed in a way the model can read.
    Failed(String),
}

impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self {
        ToolError::Failed(format!("{e:#}"))
    }
}

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub run: for<'a> fn(&'a App, &'a SessionId, Value) -> BoxFut<'a, Result<String, ToolError>>,
}

impl Tool {
    pub fn listing(&self) -> Value {
        json!({ "name": self.name, "description": self.description, "inputSchema": (self.schema)() })
    }
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "sessions",
        description: "List Atelier sessions (other agents' conversations): id, title, status, branch, last activity. \
            Defaults to your own project, without archived sessions. Look here for a session that may already know \
            what you need, then use `read`.",
        schema: sessions_schema,
        run: |app, me, args| Box::pin(async move { sessions(app, me, parse(args)?).await }),
    },
    Tool {
        name: "read",
        description: "Read what was said in a session: its user messages, the agent's replies and a line per tool \
            call (not tool output). Newest last. Shows the latest part by default; use `before` to go further \
            back or `around` to see the context of one message.",
        schema: read_schema,
        run: |app, _me, args| Box::pin(async move { read(app, parse(args)?).await }),
    },
    Tool {
        name: "send",
        description: "Tell another session something. It arrives as a new turn in that session, waking it if \
            it is idle; if it is busy, it sees the message after its current turn (it is never interrupted). \
            Asynchronous: you get no reply here. If it answers, its reply arrives to you the same way, as a \
            message. `to` is a session id or a unique prefix of at least 4 characters. `refs` are event numbers \
            (#n from `read`) in your own session that the text points at. Messages are for things the other \
            session needs to act on or know; the person can see both transcripts.",
        schema: send_schema,
        run: |app, me, args| Box::pin(async move { send(app, me, parse(args)?).await }),
    },
    Tool {
        name: "spawn",
        description: "Start a new session (a full, persistent agent of its own, visible to the person) to do a task, \
            and return its id. Choose its `workspace`: `{\"branch\": \"self\"}` (default) a new worktree at a \
            snapshot of your working tree, uncommitted changes included, to try something or split off a subtask \
            without touching yours; `{\"share\": \"self\"}` read-only in your own worktree, to see your live \
            edits (a reviewer); `\"fresh\"` a new worktree from the project's HEAD; `\"project\"` the project \
            checkout. A session id in place of `self` uses that session's worktree instead. \
            `review: \"self\"` is shorthand for a read-only reviewer of your changes. The child runs on its own: \
            it reports back when it's done and can be asked follow-ups. Limited in depth and in how many children \
            may be running at once.",
        schema: spawn_schema,
        run: |app, me, args| Box::pin(async move { spawn(app, me, parse(args)?).await }),
    },
];

fn parse<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::Params(format!("invalid arguments: {e}")))
}

// ─── sessions ────────────────────────────────────────────────────────────────

const MAX_SESSIONS: usize = 50;

fn sessions_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string", "description": "Project id. Default: your own project." },
            "status": { "type": "string", "enum": ["idle", "running", "waiting", "failed"] },
            "include_archived": { "type": "boolean", "description": "Default false." },
        },
    })
}

#[derive(Deserialize)]
struct SessionsArgs {
    project: Option<String>,
    status: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

async fn sessions(app: &App, me: &SessionId, args: SessionsArgs) -> Result<String, ToolError> {
    if let Some(s) = &args.status
        && !["idle", "running", "waiting", "failed"].contains(&s.as_str())
    {
        return Err(ToolError::Params(format!("status `{s}` is not one of idle, running, waiting, failed")));
    }
    let id = me.clone();
    let own = app.store.read(move |c| views::sessions::get(c, id.as_str())).await?.context("your session is gone")?;
    let project = args.project.unwrap_or_else(|| own.project.0.clone());
    let p = project.clone();
    if app.store.read(move |c| views::projects::get(c, &p)).await?.is_none() {
        return Err(ToolError::Failed(format!("no such project `{project}`")));
    }
    let rows = app.store.read(move |c| views::sessions::list(c, Some(&project))).await?;
    let rows: Vec<SessionRow> = rows
        .into_iter()
        .filter(|r| args.include_archived || !r.archived)
        .filter(|r| args.status.as_deref().is_none_or(|s| r.status.as_str() == s))
        .collect();
    Ok(format_sessions(&rows, me, now_ms()))
}

fn format_sessions(rows: &[SessionRow], me: &SessionId, now: i64) -> String {
    if rows.is_empty() {
        return "No sessions match.".into();
    }
    let mut out: Vec<String> = rows.iter().take(MAX_SESSIONS).map(|r| session_line(r, me, now)).collect();
    if rows.len() > MAX_SESSIONS {
        out.push(format!("(+{} more, narrow with `status`, or `project`)", rows.len() - MAX_SESSIONS));
    }
    out.join("\n")
}

/// `id · title · status · branch · updated · from <parent>`, with your own row marked.
fn session_line(r: &SessionRow, me: &SessionId, now: i64) -> String {
    let title = r.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts = vec![
        r.id.0.clone(),
        title,
        r.status.as_str().to_owned(),
        r.workspace.branch.clone().unwrap_or_else(|| "-".into()),
        ago(now, r.updated_at),
    ];
    if let Some(from) = &r.workspace.from {
        parts.push(format!("from {from}"));
    }
    if r.archived {
        parts.push("archived".into());
    }
    let line = parts.join(" · ");
    if &r.id == me { format!("{line} (you)") } else { line }
}

fn ago(now: i64, then: i64) -> String {
    let s = ((now - then) / 1000).max(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

// ─── send ────────────────────────────────────────────────────────────────────

fn send_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "to": { "type": "string", "description": "Session id, or a unique prefix of at least 4 characters." },
            "text": { "type": "string", "description": "What to tell it. Self-contained: it hasn't seen your conversation." },
            "refs": { "type": "array", "items": { "type": "integer" }, "description": "Event numbers in your own session the text refers to." },
        },
        "required": ["to", "text"],
    })
}

#[derive(Deserialize)]
struct SendArgs {
    to: String,
    text: String,
    #[serde(default)]
    refs: Vec<Seq>,
}

async fn send(app: &App, me: &SessionId, args: SendArgs) -> Result<String, ToolError> {
    use atelier_proto::SessionStatus::{Running, Waiting};
    let rows = app.store.read(|c| views::sessions::list(c, None)).await?;
    let target = crate::message::resolve(&rows, &args.to).map_err(ToolError::Failed)?;
    if target.archived {
        return Err(ToolError::Failed(format!("session {} is archived and can't receive messages", target.id)));
    }
    // The call that made this one is the latest thing the sender said: the agent is blocked on us.
    let stream = rows.iter().find(|r| &r.id == me).map(|r| r.stream.clone()).context("your session is gone")?;
    let cause = app
        .store
        .read(move |c| {
            Ok(c.query_row(
                "SELECT MAX(seq) FROM events WHERE stream = ?1 AND kind = 'cc.assistant'",
                [&stream],
                |r| r.get::<_, Option<Seq>>(0),
            )?)
        })
        .await?;
    let busy = matches!(target.status, Running | Waiting);
    let call = atelier_proto::Call::SessionSend { session: target.id.clone(), text: args.text, refs: args.refs };
    crate::rpc::handle(app, atelier_core::Actor::Agent(me.clone()), cause, call).await?;
    let state = if busy { "running — it will see this after its current turn" } else { "idle — woke it" };
    Ok(format!("delivered to {} ({state})", target.id))
}

// ─── spawn ───────────────────────────────────────────────────────────────────

fn spawn_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task": { "type": "string", "description": "What the new session should do. It starts knowing nothing else." },
            "title": { "type": "string", "description": "Default: the start of the task." },
            "workspace": {
                "description": "Default {\"branch\": \"self\"}. One of: {\"branch\": \"self\" | <session id>} a new worktree at a snapshot of that session's working tree; {\"share\": \"self\" | <session id>} that session's worktree, read-only; \"fresh\" a new worktree from the project's HEAD; \"project\" the project checkout.",
            },
            "review": { "type": "string", "description": "\"self\" or a session id: start a read-only reviewer of that session's changes. `task` then adds what to look at, and is optional." },
            "model": { "type": "string" },
        },
    })
}

#[derive(Deserialize)]
struct SpawnArgs {
    task: Option<String>,
    title: Option<String>,
    workspace: Option<Value>,
    review: Option<String>,
    model: Option<String>,
}

async fn spawn(app: &App, me: &SessionId, args: SpawnArgs) -> Result<String, ToolError> {
    use crate::spawn::{Envelope, Wanted, envelope, parse_workspace, review_prompt};
    let id = me.clone();
    let own = app.store.read(move |c| views::sessions::get(c, id.as_str())).await?.context("your session is gone")?;
    let task = args.task.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let wanted = match (&args.review, &args.workspace) {
        (Some(_), Some(_)) => return Err(ToolError::Params("use either `review` or `workspace`, not both".into())),
        (Some(r), None) if r.is_empty() => return Err(ToolError::Params("`review` is empty".into())),
        (Some(r), None) => Wanted::Share(if r == "self" { me.clone() } else { r.as_str().into() }),
        (None, ws) => parse_workspace(ws.as_ref(), me).map_err(ToolError::Params)?,
    };
    if task.is_none() && args.review.is_none() {
        return Err(ToolError::Params("`task` is required".into()));
    }
    let (mode, from) = wanted.mode_and_from();
    let project = crate::rpc::project_row(app, own.project.as_str()).await?;
    // Resolve the source first, so a bad one is refused before anything is created.
    let source = match &from {
        Some(f) => Some(crate::rpc::source_session(app, &project, f).await.map_err(|e| {
            let hint = if matches!(wanted, Wanted::Branch(_)) && from.as_ref() == Some(me) {
                " (use workspace \"fresh\" or \"project\" to start elsewhere)"
            } else {
                ""
            };
            ToolError::Failed(format!("{e:#}{hint}"))
        })?),
        None => None,
    };
    let src_title = source.as_ref().map(|s| s.title.split_whitespace().collect::<Vec<_>>().join(" "));
    let can_send = TOOLS.iter().any(|t| t.name == "send");
    let describe = match (&wanted, &source) {
        (Wanted::Share(_), Some(s)) => format!(
            "read-only in session {} (\"{}\")'s worktree{} — don't modify tracked files; build and test output is fine",
            s.id,
            src_title.clone().unwrap_or_default(),
            s.workspace.branch.as_ref().map(|b| format!(", branch `{b}`")).unwrap_or_default(),
        ),
        (Wanted::Branch(_), Some(s)) => format!(
            "a new worktree branched from a snapshot of session {} (\"{}\")'s working tree, uncommitted changes included",
            s.id,
            src_title.clone().unwrap_or_default()
        ),
        (Wanted::Project, _) => "the project directory, shared with everything else working there".to_owned(),
        _ => "a new worktree from the project's HEAD".to_owned(),
    };
    let task_text = match (&args.review, &source) {
        (Some(_), Some(s)) => {
            let base = review_prompt(
                &s.title,
                s.workspace.base.as_deref(),
                s.workspace.branch.as_deref(),
                can_send.then_some(me),
            );
            match task {
                Some(extra) => format!("{base}\n\n{extra}"),
                None => base,
            }
        }
        _ => task.unwrap_or_default().to_owned(),
    };
    let title = args
        .title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| args.review.as_ref().zip(src_title.as_ref()).map(|(_, t)| format!("Review: {t}")))
        .unwrap_or_else(|| crate::rpc::title_from(task.unwrap_or_default()));
    let prompt =
        envelope(&Envelope { parent: me, parent_title: &own.title, workspace: &describe, can_send }, &task_text);
    let call = atelier_proto::Call::SessionCreate {
        project: own.project.clone(),
        prompt,
        workspace: mode,
        title: Some(title.clone()),
        model: args.model,
        from,
        parent: Some(me.clone()),
    };
    let result = crate::rpc::handle(app, atelier_core::Actor::Agent(me.clone()), None, call).await?;
    let child: SessionId = result["id"].as_str().context("session.create returned no id")?.into();
    let cid = child.clone();
    let row =
        app.store.read(move |c| views::sessions::get(c, cid.as_str())).await?.context("the new session is gone")?;
    let place = match (row.workspace.mode, &row.workspace.branch) {
        (atelier_core::domain::WorkspaceMode::Direct, _) => "the project directory".to_owned(),
        (atelier_core::domain::WorkspaceMode::Shared, b) => format!(
            "{}'s worktree{}, read-only",
            src_title.unwrap_or_default(),
            b.as_ref().map(|b| format!(" {b}")).unwrap_or_default()
        ),
        (_, Some(b)) => format!("worktree {b}"),
        (_, None) => "a new worktree".to_owned(),
    };
    Ok(format!("started {child} '{title}' in {place}"))
}

// ─── read ────────────────────────────────────────────────────────────────────

const DEFAULT_CHARS: usize = 8_000;
const MAX_CHARS: usize = 32_000;

fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "session": { "type": "string", "description": "Session id, from `sessions`." },
            "around": { "type": "integer", "description": "Event number (#n): show the messages around it." },
            "before": { "type": "integer", "description": "Event number (#n): show the messages before it." },
            "max_chars": { "type": "integer", "description": "Default 8000, at most 32000." },
        },
        "required": ["session"],
    })
}

#[derive(Deserialize)]
struct ReadArgs {
    session: String,
    around: Option<Seq>,
    before: Option<Seq>,
    max_chars: Option<usize>,
}

async fn read(app: &App, args: ReadArgs) -> Result<String, ToolError> {
    if args.around.is_some() && args.before.is_some() {
        return Err(ToolError::Params("use either `around` or `before`, not both".into()));
    }
    let max = args.max_chars.unwrap_or(DEFAULT_CHARS).clamp(1, MAX_CHARS);
    let id = args.session.clone();
    let Some(s) = app.store.read(move |c| views::sessions::get(c, &id)).await? else {
        return Err(ToolError::Failed(format!("no such session `{}`", args.session)));
    };
    let window = match (args.around, args.before) {
        (Some(a), _) => Window::Around(a),
        (_, Some(b)) => Window::Before(b),
        _ => Window::Tail,
    };
    let stream = s.stream.clone();
    let page = app.store.read(move |c| page(c, &stream, window, max)).await?;
    Ok(format!("Session {} · {}\n{}", s.id, s.title.split_whitespace().collect::<Vec<_>>().join(" "), page.render()))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Window {
    /// The newest messages.
    Tail,
    /// The newest messages with a seq below this.
    Before(Seq),
    /// Messages on both sides of this seq.
    Around(Seq),
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Page {
    /// `(seq, role, text)`, oldest first.
    pub lines: Vec<(Seq, &'static str, String)>,
    /// Conversation events before the first line / after the last.
    pub earlier: i64,
    pub later: i64,
    pub first: Option<Seq>,
    pub last: Option<Seq>,
    around: bool,
}

impl Page {
    pub fn render(&self) -> String {
        let mut out = Vec::new();
        if let (n @ 1.., Some(first)) = (self.earlier, self.first) {
            out.push(format!("({n} earlier — read with before={first})"));
        }
        out.extend(self.lines.iter().map(|(seq, role, text)| format!("#{seq} {role}: {text}")));
        if let (n @ 1.., Some(last), true) = (self.later, self.last, self.around) {
            out.push(format!("({n} later — read with around={last})"));
        }
        if self.lines.is_empty() && out.is_empty() {
            out.push("(nothing said here yet)".into());
        }
        out.join("\n")
    }
}

const CONVERSATION: &str = "kind IN ('user.message', 'agent.message', 'cc.assistant')";

/// Up to `budget` chars of one side of `pivot`, cut at line boundaries (the first line is cut to fit if it
/// alone is too long). Reads rows lazily, so cost follows the budget, not the session's length.
fn scan(
    c: &Connection,
    stream: &str,
    lo: Seq,
    hi: Seq,
    newest_first: bool,
    mut budget: usize,
) -> anyhow::Result<Vec<(Seq, &'static str, String)>> {
    let order = if newest_first { "DESC" } else { "ASC" };
    let mut q = c.prepare_cached(&format!(
        "SELECT seq, stream, ts, kind, actor, cause, body FROM events
         WHERE stream = ?1 AND seq >= ?2 AND seq <= ?3 AND {CONVERSATION} ORDER BY seq {order}"
    ))?;
    let mut rows = q.query(rusqlite::params![stream, lo, hi])?;
    let mut out: Vec<(Seq, &'static str, String)> = Vec::new();
    'rows: while let Some(r) = rows.next()? {
        let body: String = r.get(6)?;
        let Ok(body) = serde_json::value::RawValue::from_string(body) else { continue };
        let ev = Event {
            seq: r.get(0)?,
            stream: r.get(1)?,
            ts: r.get(2)?,
            kind: r.get(3)?,
            actor: r.get(4)?,
            cause: r.get(5)?,
            body,
        };
        let mut docs = text::docs(&ev);
        if newest_first {
            docs.reverse();
        }
        for d in docs {
            let cost = d.text.chars().count() + 16;
            if cost > budget && !out.is_empty() {
                break 'rows;
            }
            let text =
                if cost > budget { d.text.chars().take(budget.saturating_sub(16).max(1)).collect() } else { d.text };
            budget = budget.saturating_sub(cost);
            out.push((ev.seq, d.role.as_str(), text));
        }
        if budget == 0 {
            break;
        }
    }
    if newest_first {
        out.reverse();
    }
    Ok(out)
}

fn count(c: &Connection, stream: &str, lo: Seq, hi: Seq) -> anyhow::Result<i64> {
    Ok(c.query_row(
        &format!("SELECT COUNT(*) FROM events WHERE stream = ?1 AND seq >= ?2 AND seq <= ?3 AND {CONVERSATION}"),
        rusqlite::params![stream, lo, hi],
        |r| r.get(0),
    )?)
}

pub(crate) fn page(c: &Connection, stream: &str, window: Window, max_chars: usize) -> anyhow::Result<Page> {
    let (lines, around) = match window {
        Window::Tail => (scan(c, stream, 0, Seq::MAX, true, max_chars)?, false),
        Window::Before(b) => (scan(c, stream, 0, b.saturating_sub(1), true, max_chars)?, false),
        Window::Around(a) => {
            let mut lines = scan(c, stream, 0, a, true, max_chars.div_ceil(2))?;
            let used: usize = lines.iter().map(|l| l.2.chars().count() + 16).sum();
            lines.extend(scan(c, stream, a.saturating_add(1), Seq::MAX, false, max_chars.saturating_sub(used))?);
            (lines, true)
        }
    };
    let (first, last) = (lines.first().map(|l| l.0), lines.last().map(|l| l.0));
    // Counted from the edges of what was returned; with nothing returned, the edge is the window's own.
    let earlier = match (first, window) {
        (Some(f), _) => count(c, stream, 0, f - 1)?,
        (None, Window::Before(b)) => count(c, stream, 0, b.saturating_sub(1))?,
        (None, Window::Around(a)) => count(c, stream, 0, a)?,
        (None, Window::Tail) => 0,
    };
    let later = match last {
        Some(l) if around => count(c, stream, l + 1, Seq::MAX)?,
        _ => 0,
    };
    Ok(Page { lines, earlier, later, first, last, around })
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_core::domain::{UserMessage, Workspace, WorkspaceMode};
    use atelier_core::{Actor, NewEvent};
    use atelier_proto::SessionStatus;
    use atelier_store::Store;
    use serde_json::value::RawValue;

    const STREAM: &str = "p/p1/s/s1";

    fn row(id: &str, title: &str, updated_at: i64) -> SessionRow {
        SessionRow {
            id: id.into(),
            project: "p1".into(),
            stream: format!("p/p1/s/{id}"),
            title: title.into(),
            agent: atelier_core::domain::AgentKind::ClaudeCode,
            workspace: Workspace {
                mode: WorkspaceMode::Worktree,
                path: "/w".into(),
                branch: Some(format!("atelier/{id}")),
                base: None,
                from: None,
                snapshot: None,
            },
            model: None,
            status: SessionStatus::Idle,
            alive: false,
            pending: 0,
            turns: 0,
            cost_usd: 0.0,
            created_at: 0,
            updated_at,
            archived: false,
            imported: false,
            parent: None,
        }
    }

    #[test]
    fn sessions_list_marks_you_and_shows_relative_time() {
        let now = 10_000_000_000;
        let mut other = row("b", "Fix  the\nWAL", now - 3 * 3_600_000);
        other.workspace.from = Some("a".into());
        other.status = SessionStatus::Running;
        let mut old = row("c", "Old", now - 3 * 86_400_000);
        old.archived = true;
        old.workspace.branch = None;
        let out = format_sessions(&[row("a", "Me", now - 5 * 60_000), other, old], &"a".into(), now);
        assert_eq!(
            out,
            "a · Me · idle · atelier/a · 5m ago (you)\n\
             b · Fix the WAL · running · atelier/b · 3h ago · from a\n\
             c · Old · idle · - · 3d ago · archived"
        );
        assert_eq!(format_sessions(&[], &"a".into(), now), "No sessions match.");
    }

    #[test]
    fn sessions_list_is_capped() {
        let rows: Vec<_> = (0..60).map(|i| row(&format!("s{i}"), "t", 0)).collect();
        let out = format_sessions(&rows, &"x".into(), 0);
        assert_eq!(out.lines().count(), MAX_SESSIONS + 1);
        assert!(out.ends_with("(+10 more, narrow with `status`, or `project`)"));
    }

    async fn store_with(messages: &[&str]) -> (Store, tempfile::TempDir, Vec<Seq>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("db"), vec![]).unwrap();
        let mut seqs = Vec::new();
        for (i, m) in messages.iter().enumerate() {
            // Alternate user and agent, with a tool result in between that must never show up.
            let ev = if i % 2 == 0 {
                NewEvent::new(STREAM, Actor::User, &UserMessage { text: (*m).into() })
            } else {
                let body = json!({"message": {"content": [{"type": "text", "text": m}]}}).to_string();
                NewEvent::raw(STREAM, "cc.assistant", Actor::Agent("s1".into()), RawValue::from_string(body).unwrap())
            };
            seqs.push(store.append_one(ev).await.unwrap().seq);
            let noise = json!({"message": {"content": [{"type": "tool_result", "content": "NOISE"}]}}).to_string();
            let ev = NewEvent::raw(STREAM, "cc.user", Actor::Agent("s1".into()), RawValue::from_string(noise).unwrap());
            store.append_one(ev).await.unwrap();
        }
        (store, dir, seqs)
    }

    fn texts(p: &Page) -> Vec<&str> {
        p.lines.iter().map(|l| l.2.as_str()).collect()
    }

    #[tokio::test]
    async fn read_defaults_to_the_newest_messages_oldest_first() {
        let (store, _d, seqs) = store_with(&["one", "two", "three", "four"]).await;
        let p = store.read(|c| page(c, STREAM, Window::Tail, 8_000)).await.unwrap();
        assert_eq!(texts(&p), ["one", "two", "three", "four"]);
        assert_eq!(p.earlier, 0);
        assert_eq!(
            p.render(),
            format!(
                "#{} user: one\n#{} agent: two\n#{} user: three\n#{} agent: four",
                seqs[0], seqs[1], seqs[2], seqs[3]
            )
        );
        assert!(!p.render().contains("NOISE"));
    }

    #[tokio::test]
    async fn read_cuts_at_the_budget_and_says_how_to_continue() {
        let (store, _d, seqs) = store_with(&["one", "two", "three", "four"]).await;
        // Each line costs its text plus 16; room for two lines of ~4 chars, not three.
        let p = store.read(|c| page(c, STREAM, Window::Tail, 45)).await.unwrap();
        assert_eq!(texts(&p), ["three", "four"]);
        assert_eq!(p.earlier, 2);
        assert!(p.render().starts_with(&format!("(2 earlier — read with before={})\n", seqs[2])));

        let before = seqs[2];
        let p = store.read(move |c| page(c, STREAM, Window::Before(before), 8_000)).await.unwrap();
        assert_eq!(texts(&p), ["one", "two"]);
        assert_eq!(p.earlier, 0);
    }

    #[tokio::test]
    async fn read_around_takes_both_sides() {
        let (store, _d, seqs) = store_with(&["a1", "b2", "c3", "d4", "e5", "f6"]).await;
        let at = seqs[2];
        // Half the budget goes back from #c3 (inclusive), the rest forward.
        let p = store.read(move |c| page(c, STREAM, Window::Around(at), 80)).await.unwrap();
        assert_eq!(texts(&p), ["b2", "c3", "d4", "e5"]);
        assert_eq!((p.earlier, p.later), (1, 1));
        let r = p.render();
        assert!(r.starts_with("(1 earlier"), "{r}");
        assert!(r.ends_with(&format!("(1 later — read with around={})", seqs[4])), "{r}");
    }

    #[tokio::test]
    async fn read_cuts_an_oversized_first_line_instead_of_returning_nothing() {
        let long = "x".repeat(500);
        let (store, _d, _) = store_with(&[&long]).await;
        let p = store.read(|c| page(c, STREAM, Window::Tail, 100)).await.unwrap();
        assert_eq!(p.lines.len(), 1);
        assert!(p.lines[0].2.len() < 100);
    }

    #[tokio::test]
    async fn an_empty_session_says_so() {
        let (store, _d, _) = store_with(&[]).await;
        let p = store.read(|c| page(c, STREAM, Window::Tail, 100)).await.unwrap();
        assert_eq!(p.render(), "(nothing said here yet)");
    }
}
