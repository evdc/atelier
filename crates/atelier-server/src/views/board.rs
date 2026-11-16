//! `board`: each project's effective board config, from its `.atelier/board.json` as the log has it. A project
//! without the file has no table row; its view row is the defaults.

use anyhow::Result;
use atelier_core::domain::FileChanged;
use atelier_core::{Event, ProjectId};
use atelier_proto::BoardRow;
use atelier_store::{Change, Projection};
use rusqlite::{Connection, OptionalExtension, params};

use super::parse;
use crate::board::{self, PATH};

pub const VIEW: &str = "board";

pub struct Board;

impl Projection for Board {
    fn name(&self) -> &'static str {
        "board"
    }
    fn version(&self) -> u32 {
        1
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS board (
                project  TEXT PRIMARY KEY,
                config   TEXT NOT NULL,
                problems TEXT NOT NULL
             ) STRICT;",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS board;")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> Result<()> {
        let Some(f) = parse::<FileChanged>(ev) else { return Ok(()) };
        let Some(project) = ev.stream.strip_prefix("p/").filter(|p| !p.contains('/')) else { return Ok(()) };
        if f.path != PATH {
            return Ok(());
        }
        match f.text {
            Some(text) => {
                let (config, problems) = board::effective(Some(&text));
                db.execute(
                    "INSERT INTO board (project, config, problems) VALUES (?1, ?2, ?3)
                     ON CONFLICT(project) DO UPDATE SET config = ?2, problems = ?3",
                    params![project, serde_json::to_string(&config)?, serde_json::to_string(&problems)?],
                )?;
            }
            None => {
                db.execute("DELETE FROM board WHERE project = ?1", [project])?;
            }
        }
        out.push(Change::new(VIEW, project, Some(project.to_string())));
        Ok(())
    }
}

/// The row for `project`: its file's config, or the defaults if it has none.
pub fn get(c: &Connection, project: &str) -> Result<BoardRow> {
    let stored: Option<(String, String)> = c
        .query_row("SELECT config, problems FROM board WHERE project = ?1", [project], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let (config, problems, from_file) = match stored {
        Some((config, problems)) => (serde_json::from_str(&config)?, serde_json::from_str(&problems)?, true),
        None => (board::defaults(), Vec::new(), false),
    };
    Ok(BoardRow { id: ProjectId::from(project), config, problems, from_file })
}

/// Every project that has a board file, in project order.
pub fn list(c: &Connection) -> Result<Vec<BoardRow>> {
    let mut q = c.prepare_cached("SELECT project FROM board ORDER BY project")?;
    let projects: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    projects.iter().map(|p| get(c, p)).collect()
}

#[cfg(test)]
mod tests {
    use atelier_core::domain::FileChanged;
    use atelier_core::{Actor, NewEvent};
    use atelier_store::Store;

    use super::*;

    fn write(path: &str, text: Option<&str>) -> NewEvent {
        NewEvent::new("p/P1", Actor::Atelier, &FileChanged { path: path.into(), text: text.map(Into::into) })
    }

    #[tokio::test]
    async fn file_sets_and_deletion_reverts_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("t.db"), vec![Box::new(Board)]).unwrap();
        let get_row = || store.read(|c| get(c, "P1"));

        let row = get_row().await.unwrap();
        assert!(!row.from_file);
        assert_eq!(row.config.statuses, ["open", "in-progress", "done"]);

        store
            .append_one(write(
                PATH,
                Some(r#"{"statuses": ["open", "review"], "columns": [{"title": "R", "statuses": ["review"]}]}"#),
            ))
            .await
            .unwrap();
        let row = get_row().await.unwrap();
        assert!(row.from_file && row.problems.is_empty());
        assert_eq!(row.config.statuses, ["open", "review"]);
        assert_eq!(row.config.columns[0].title, "R");

        store.append_one(write(PATH, Some("{ broken"))).await.unwrap();
        let row = get_row().await.unwrap();
        assert!(row.from_file);
        assert_eq!(row.config.statuses, ["open", "in-progress", "done"]);
        assert_eq!(row.problems.len(), 1);

        store.append_one(write(PATH, None)).await.unwrap();
        let row = get_row().await.unwrap();
        assert!(!row.from_file && row.problems.is_empty());
        assert_eq!(row.config.statuses, ["open", "in-progress", "done"]);
    }

    #[tokio::test]
    async fn other_files_and_projects_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("t.db"), vec![Box::new(Board)]).unwrap();
        store.append_one(write(".atelier/tickets/AT-1.md", Some("x"))).await.unwrap();
        store
            .append_one(NewEvent::new(
                "p/P2/s/x",
                Actor::Atelier,
                &FileChanged { path: PATH.into(), text: Some(r#"{"statuses": ["x"]}"#.into()) },
            ))
            .await
            .unwrap();
        assert!(store.read(list).await.unwrap().is_empty());
    }
}
