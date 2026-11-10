use anyhow::Result;
use atelier_core::domain::*;
use atelier_core::{Event, stream};
use atelier_proto::{SessionRow, SessionStatus};
use atelier_store::{Change, Projection};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::parse;

pub const VIEW: &str = "sessions";

/// Session summaries: status, liveness, pending requests, spend.
///
/// Status is a pure function of lifecycle events:
///
/// ```text
/// user.message ─▶ running ─(permission.requested)─▶ waiting
///                    ▲  └──────(turn.ended)──────▶ idle | failed
///                    └──(permission.answered, none left pending)
/// process.exited while running/waiting ─▶ failed (idle if Atelier stopped it)
/// ```
///
/// Vendor events (`cc.*`) are deliberately ignored: they don't change the
/// summary, and touching rows per message would churn every watcher.
pub struct Sessions;

impl Projection for Sessions {
    fn name(&self) -> &'static str {
        "sessions"
    }
    fn version(&self) -> u32 {
        4
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id         TEXT PRIMARY KEY,
                project    TEXT NOT NULL,
                stream     TEXT NOT NULL,
                title      TEXT NOT NULL,
                agent      TEXT NOT NULL,
                workspace  TEXT NOT NULL,      -- JSON (domain::Workspace)
                model      TEXT,
                native_id  TEXT,
                status     TEXT NOT NULL,
                alive      INTEGER NOT NULL DEFAULT 0,
                pending    INTEGER NOT NULL DEFAULT 0,
                turns      INTEGER NOT NULL DEFAULT 0,
                cost_usd   REAL NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                archived   INTEGER NOT NULL DEFAULT 0,
                imported   INTEGER NOT NULL DEFAULT 0,
                -- Archiving released the worktree; the directory may be pooled for another session.
                worktree_removed INTEGER NOT NULL DEFAULT 0
             ) STRICT;
             -- One row per imported history file; `offset` is how far into it the log has seen.
             CREATE TABLE IF NOT EXISTS imports (
                path      TEXT PRIMARY KEY,
                native_id TEXT NOT NULL,
                session   TEXT NOT NULL,
                offset    INTEGER NOT NULL DEFAULT 0
             ) STRICT;
             CREATE INDEX IF NOT EXISTS sessions_by_project ON sessions(project, updated_at);",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS sessions; DROP TABLE IF EXISTS imports;")
    }

    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> Result<()> {
        // Only lifecycle kinds matter; bail early on everything else (most events).
        let kind = ev.kind.as_str();
        if !(kind.starts_with("session.")
            || kind.starts_with("workspace.")
            || kind.starts_with("user.")
            || kind.starts_with("turn.")
            || kind.starts_with("permission.")
            || kind.starts_with("process.")
            || kind.starts_with("agent.")
            || kind.starts_with("import."))
        {
            return Ok(());
        }
        let Some((project, id)) = stream::parse_session(&ev.stream) else {
            return Ok(());
        };
        let id = id.as_str();

        if let Some(p) = parse::<ImportProgressed>(ev) {
            db.execute("UPDATE imports SET offset = ?1 WHERE path = ?2", params![p.offset as i64, p.path])?;
            if let Some(at) = p.at {
                db.execute("UPDATE sessions SET updated_at = MAX(updated_at, ?1) WHERE id = ?2", params![at, id])?;
                out.push(Change::new(VIEW, id, Some(project.0.clone())));
                out.push(Change::new(super::projects::VIEW, project.0, None));
            }
            return Ok(());
        }

        if let Some(s) = parse::<SessionCreated>(ev) {
            let at = s.started_at.unwrap_or(ev.ts);
            let (imported, native_id) = match &s.origin {
                Some(Origin::Imported { native_id, path }) => {
                    db.execute(
                        "INSERT OR IGNORE INTO imports (path, native_id, session) VALUES (?1, ?2, ?3)",
                        params![path, native_id, id],
                    )?;
                    (true, Some(native_id.as_str()))
                }
                None => (false, None),
            };
            db.execute(
                "INSERT OR IGNORE INTO sessions
                   (id, project, stream, title, agent, workspace, model, native_id, imported, status,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?9, ?10, 'idle', ?8, ?8)",
                params![
                    id,
                    project.as_str(),
                    ev.stream,
                    s.title,
                    serde_json::to_value(s.agent)?.as_str(),
                    serde_json::to_string(&s.workspace)?,
                    s.model,
                    at,
                    native_id,
                    imported
                ],
            )?;
        } else {
            let Some((status, pending)) = db
                .query_row("SELECT status, pending FROM sessions WHERE id = ?1", [id], |r| {
                    Ok((SessionStatus::parse(&r.get::<_, String>(0)?), r.get::<_, u32>(1)?))
                })
                .optional()?
            else {
                return Ok(()); // events for a session we never saw created
            };

            // `UPDATE sessions SET <sql>, updated_at = <ts> WHERE id = <id>`
            let set = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<usize> {
                let n = args.len();
                let q = format!("UPDATE sessions SET {sql}, updated_at = ?{} WHERE id = ?{}", n + 1, n + 2);
                let mut all = args.to_vec();
                all.push(&ev.ts);
                all.push(&id);
                db.prepare_cached(&q)?.execute(all.as_slice())
            };
            use SessionStatus::*;

            if parse::<UserMessage>(ev).is_some() {
                set("status = ?1", &[&Running.as_str()])?;
            } else if let Some(t) = parse::<TurnEnded>(ev) {
                let next = if pending > 0 {
                    Waiting
                } else if t.is_error {
                    Failed
                } else {
                    Idle
                };
                set(
                    "status = ?1, turns = turns + 1, cost_usd = cost_usd + ?2",
                    &[&next.as_str(), &t.cost_usd.unwrap_or(0.0)],
                )?;
            } else if parse::<PermissionRequested>(ev).is_some() {
                set("status = ?1, pending = pending + 1", &[&Waiting.as_str()])?;
            } else if parse::<PermissionAnswered>(ev).is_some() {
                let left = pending.saturating_sub(1);
                let next = if left > 0 { Waiting } else { Running };
                set("status = ?1, pending = ?2", &[&next.as_str(), &left])?;
            } else if let Some(p) = parse::<ProcessStarted>(ev) {
                set("alive = 1, native_id = COALESCE(?1, native_id)", &[&p.native_id])?;
            } else if let Some(b) = parse::<AgentBound>(ev) {
                set("native_id = ?1", &[&b.native_id])?;
            } else if let Some(x) = parse::<ProcessExited>(ev) {
                let next = match (status, x.reason) {
                    (Running | Waiting, ExitReason::Stopped) => Idle,
                    (Running | Waiting, _) => Failed,
                    (s, _) => s,
                };
                set("alive = 0, pending = 0, status = ?1", &[&next.as_str()])?;
            } else if let Some(r) = parse::<SessionRenamed>(ev) {
                set("title = ?1", &[&r.title])?;
            } else if parse::<SessionArchived>(ev).is_some() {
                set("archived = 1", &[])?;
            } else if parse::<SessionUnarchived>(ev).is_some() {
                set("archived = 0", &[])?;
            } else if parse::<WorkspaceRemoved>(ev).is_some() {
                set("worktree_removed = 1", &[])?;
            } else {
                return Ok(()); // e.g. user.interrupt: the turn.ended that follows is what counts
            }
        }

        out.push(Change::new(VIEW, id, Some(project.0.clone())));
        out.push(Change::new(super::projects::VIEW, project.0, None));
        Ok(())
    }
}

const SELECT: &str = "SELECT id, project, stream, title, agent, workspace, model, status, alive, pending,
                             turns, cost_usd, created_at, updated_at, archived, imported
                      FROM sessions";

pub fn list(c: &Connection, project: Option<&str>) -> Result<Vec<SessionRow>> {
    let rows = match project {
        Some(p) => {
            let mut q = c.prepare_cached(&format!("{SELECT} WHERE project = ?1 ORDER BY updated_at DESC"))?;
            q.query_map([p], row)?.collect::<rusqlite::Result<Vec<_>>>()?
        }
        None => {
            let mut q = c.prepare_cached(&format!("{SELECT} ORDER BY updated_at DESC"))?;
            q.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    Ok(rows)
}

pub fn get(c: &Connection, id: &str) -> Result<Option<SessionRow>> {
    let mut q = c.prepare_cached(&format!("{SELECT} WHERE id = ?1"))?;
    Ok(q.query_row([id], row).optional()?)
}

/// Whether archiving let go of this session's worktree, so its directory no longer belongs to it.
pub fn worktree_removed(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row("SELECT worktree_removed FROM sessions WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .unwrap_or(false))
}

/// The agent's own session id, for resuming.
pub fn native_id(c: &Connection, id: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT native_id FROM sessions WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
}

/// Sessions the log says have a live process (used for recovery on startup).
pub fn alive(c: &Connection) -> Result<Vec<SessionRow>> {
    let mut q = c.prepare_cached(&format!("{SELECT} WHERE alive = 1"))?;
    Ok(q.query_map([], row)?.collect::<rusqlite::Result<_>>()?)
}

fn row(r: &Row) -> rusqlite::Result<SessionRow> {
    let json_err = |i, e: serde_json::Error| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    };
    let agent: String = r.get(4)?;
    let workspace: String = r.get(5)?;
    Ok(SessionRow {
        id: r.get::<_, String>(0)?.as_str().into(),
        project: r.get::<_, String>(1)?.as_str().into(),
        stream: r.get(2)?,
        title: r.get(3)?,
        agent: serde_json::from_value(agent.into()).map_err(|e| json_err(4, e))?,
        workspace: serde_json::from_str(&workspace).map_err(|e| json_err(5, e))?,
        model: r.get(6)?,
        status: SessionStatus::parse(&r.get::<_, String>(7)?),
        alive: r.get(8)?,
        pending: r.get(9)?,
        turns: r.get(10)?,
        cost_usd: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
        archived: r.get(14)?,
        imported: r.get(15)?,
    })
}

/// Where an imported history file has been read up to, if it has been imported.
pub fn import_offset(c: &Connection, path: &str) -> Result<Option<(String, u64)>> {
    Ok(c.query_row("SELECT session, offset FROM imports WHERE path = ?1", [path], |r| {
        Ok((r.get(0)?, r.get::<_, i64>(1)? as u64))
    })
    .optional()?)
}

/// Whether a session already has this agent session id (started here, or imported).
pub fn has_native_id(c: &Connection, native_id: &str) -> Result<bool> {
    Ok(c.query_row("SELECT 1 FROM sessions WHERE native_id = ?1", [native_id], |_| Ok(())).optional()?.is_some())
}
