use anyhow::Result;
use atelier_core::Event;
use atelier_core::domain::{ProjectAdded, ProjectRemoved};
use atelier_proto::ProjectRow;
use atelier_store::{Change, Projection};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::parse;

pub const VIEW: &str = "projects";

pub struct Projects;

impl Projection for Projects {
    fn name(&self) -> &'static str {
        "projects"
    }
    fn version(&self) -> u32 {
        1
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS projects (
                id       TEXT PRIMARY KEY,
                name     TEXT NOT NULL,
                path     TEXT NOT NULL,
                is_git   INTEGER NOT NULL,
                added_at INTEGER NOT NULL,
                removed  INTEGER NOT NULL DEFAULT 0
             ) STRICT;",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS projects;")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> Result<()> {
        if let Some(p) = parse::<ProjectAdded>(ev) {
            db.execute(
                "INSERT INTO projects (id, name, path, is_git, added_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET name = ?2, path = ?3, is_git = ?4, removed = 0",
                params![p.id.as_str(), p.name, p.path, p.is_git, ev.ts],
            )?;
            out.push(Change::new(VIEW, p.id.0, None));
        } else if let Some(p) = parse::<ProjectRemoved>(ev) {
            db.execute("UPDATE projects SET removed = 1 WHERE id = ?1", [p.id.as_str()])?;
            out.push(Change::new(VIEW, p.id.0, None));
        }
        Ok(())
    }
}

// Session counts come from the sessions projection's table.
const SELECT: &str = "
    SELECT p.id, p.name, p.path, p.is_git, p.added_at,
           MAX(p.added_at, COALESCE(MAX(s.updated_at), 0)),
           COUNT(s.id),
           COALESCE(SUM(s.status = 'running'), 0),
           COALESCE(SUM(s.status = 'waiting'), 0)
    FROM projects p
    LEFT JOIN sessions s ON s.project = p.id AND s.archived = 0
    WHERE p.removed = 0";

pub fn list(c: &Connection) -> Result<Vec<ProjectRow>> {
    let mut q = c.prepare_cached(&format!("{SELECT} GROUP BY p.id ORDER BY 6 DESC"))?;
    Ok(q.query_map([], row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn get(c: &Connection, id: &str) -> Result<Option<ProjectRow>> {
    let mut q = c.prepare_cached(&format!("{SELECT} AND p.id = ?1 GROUP BY p.id"))?;
    Ok(q.query_row([id], row).optional()?)
}

pub fn find_by_path(c: &Connection, path: &str) -> Result<Option<ProjectRow>> {
    let id: Option<String> =
        c.query_row("SELECT id FROM projects WHERE path = ?1 AND removed = 0", [path], |r| r.get(0)).optional()?;
    match id {
        Some(id) => get(c, &id),
        None => Ok(None),
    }
}

fn row(r: &Row) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: r.get::<_, String>(0)?.as_str().into(),
        name: r.get(1)?,
        path: r.get(2)?,
        is_git: r.get(3)?,
        added_at: r.get(4)?,
        updated_at: r.get(5)?,
        sessions: r.get(6)?,
        running: r.get(7)?,
        waiting: r.get(8)?,
    })
}
