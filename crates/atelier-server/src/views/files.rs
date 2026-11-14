//! `files`: which `.atelier/` files the log knows about, and a hash of their latest text. It has no view; the
//! watcher reads it to skip no-op ingests, and `at://` lookups will later.

use anyhow::Result;
use atelier_core::Event;
use atelier_core::domain::FileChanged;
use atelier_store::{Change, Projection};
use rusqlite::{Connection, OptionalExtension, params};

use super::parse;

pub struct Files;

/// FNV-1a, 64 bit. Stable across runs and platforms (unlike `DefaultHasher`), which the table relies on.
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

impl Projection for Files {
    fn name(&self) -> &'static str {
        "files"
    }
    fn version(&self) -> u32 {
        1
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                project TEXT NOT NULL,
                path    TEXT NOT NULL,
                hash    INTEGER NOT NULL,
                len     INTEGER NOT NULL,
                seq     INTEGER NOT NULL,
                PRIMARY KEY (project, path)
             ) STRICT, WITHOUT ROWID;",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS files;")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, _out: &mut Vec<Change>) -> Result<()> {
        let Some(f) = parse::<FileChanged>(ev) else { return Ok(()) };
        let Some(project) = ev.stream.strip_prefix("p/").filter(|p| !p.contains('/')) else { return Ok(()) };
        match f.text {
            Some(text) => {
                db.execute(
                    "INSERT INTO files (project, path, hash, len, seq) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(project, path) DO UPDATE SET hash = ?3, len = ?4, seq = ?5",
                    // SQLite integers are signed; the bit pattern round-trips.
                    params![project, f.path, hash(&text) as i64, text.len() as i64, ev.seq],
                )?;
            }
            None => {
                db.execute("DELETE FROM files WHERE project = ?1 AND path = ?2", params![project, f.path])?;
            }
        }
        Ok(())
    }
}

/// The hash of the last known text of `path`, if the file exists in the log.
pub fn hash_of(c: &Connection, project: &str, path: &str) -> Result<Option<u64>> {
    Ok(c.query_row("SELECT hash FROM files WHERE project = ?1 AND path = ?2", [project, path], |r| {
        r.get::<_, i64>(0).map(|h| h as u64)
    })
    .optional()?)
}

/// Known paths equal to `prefix` or below it (`prefix/…`).
pub fn paths_under(c: &Connection, project: &str, prefix: &str) -> Result<Vec<String>> {
    // `/` + 1 = `0`: the range is exactly the strings starting with `prefix/`.
    let mut q = c.prepare_cached(
        "SELECT path FROM files WHERE project = ?1 AND (path = ?2 OR (path >= ?2 || '/' AND path < ?2 || '0'))",
    )?;
    Ok(q.query_map([project, prefix], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use atelier_core::{Actor, NewEvent};
    use atelier_store::Store;

    use super::*;

    fn change(path: &str, text: Option<&str>) -> NewEvent {
        NewEvent::new("p/P1", Actor::Atelier, &FileChanged { path: path.into(), text: text.map(Into::into) })
    }

    #[test]
    fn hash_is_stable() {
        // Pinned: changing the function would make every stored hash stale.
        assert_eq!(hash(""), 0xcbf29ce484222325);
        assert_eq!(hash("a"), 0xaf63dc4c8601ec8c);
        assert_ne!(hash("a"), hash("b"));
    }

    #[tokio::test]
    async fn insert_update_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("t.db"), vec![Box::new(Files)]).unwrap();
        let get = |path: &'static str| {
            store.read(move |c| {
                Ok((
                    hash_of(c, "P1", path)?,
                    c.query_row("SELECT COALESCE(MAX(len), -1) FROM files WHERE path = ?1", [path], |r| {
                        r.get::<_, i64>(0)
                    })?,
                ))
            })
        };

        store.append_one(change(".atelier/a.md", Some("hello"))).await.unwrap();
        assert_eq!(get(".atelier/a.md").await.unwrap(), (Some(hash("hello")), 5));

        store.append_one(change(".atelier/a.md", Some("hello!"))).await.unwrap();
        assert_eq!(get(".atelier/a.md").await.unwrap(), (Some(hash("hello!")), 6));

        store.append_one(change(".atelier/a.md", None)).await.unwrap();
        assert_eq!(get(".atelier/a.md").await.unwrap().0, None);
    }

    #[tokio::test]
    async fn paths_under_matches_whole_segments() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("t.db"), vec![Box::new(Files)]).unwrap();
        for p in [".atelier/t/a.md", ".atelier/t/b/c.md", ".atelier/t2/x.md", ".atelier/t"] {
            store.append_one(change(p, Some("x"))).await.unwrap();
        }
        let mut got = store.read(|c| paths_under(c, "P1", ".atelier/t")).await.unwrap();
        got.sort();
        assert_eq!(got, [".atelier/t", ".atelier/t/a.md", ".atelier/t/b/c.md"]);
    }
}
