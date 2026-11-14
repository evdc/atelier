//! `tickets`: one row per `.atelier/tickets/<ID>.md`, folded from `file.changed`.
//!
//! Status is a free-form string; which ones exist, and in what order, is project config (the board), not code.

use std::cmp::Ordering;

use anyhow::Result;
use atelier_core::Event;
use atelier_core::domain::FileChanged;
use atelier_proto::TicketRow;
use atelier_store::{Change, Projection};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::frontmatter;
use super::parse;

pub const VIEW: &str = "tickets";

pub struct Tickets;

/// What a ticket file says, without the body.
#[derive(Debug, PartialEq, Eq)]
pub struct Ticket {
    pub id: String,
    pub title: String,
    pub status: String,
    pub size: String,
    pub milestone: String,
    pub area: String,
    pub depends: Vec<String>,
    pub checks_done: u32,
    pub checks_total: u32,
    /// Front-matter keys we have no column for, so nothing in the file is lost.
    pub extra: std::collections::BTreeMap<String, String>,
    pub problem: Option<String>,
}

/// `<ID>` for `.atelier/tickets/<ID>.md`; `None` for any other path and for the README.
pub fn ticket_stem(path: &str) -> Option<&str> {
    let stem = path.strip_prefix(".atelier/tickets/")?.strip_suffix(".md")?;
    (!stem.is_empty() && !stem.contains('/') && stem != "README").then_some(stem)
}

/// Reads a ticket file. `stem` (the filename without `.md`) is the ticket's identity.
pub fn parse_ticket(stem: &str, text: &str) -> Ticket {
    let fm = frontmatter::parse(text);
    let mut f = fm.fields;
    let mut text_of = |k: &str| f.remove(k).map(|v| v.into_text().trim().to_string()).unwrap_or_default();
    let (fm_id, title, status) = (text_of("id"), text_of("title"), text_of("status"));
    let (size, milestone, area) = (text_of("size"), text_of("milestone"), text_of("area"));
    let depends = f.remove("depends").map(|v| v.into_list()).unwrap_or_default();

    let mut problems: Vec<String> = fm.problem.into_iter().collect();
    if !fm_id.is_empty() && fm_id != stem {
        problems.push(format!("front matter says id {fm_id}, but the file is {stem}.md"));
    }
    let title = if !title.is_empty() { title } else { heading(fm.body).unwrap_or(stem).to_string() };
    let (checks_done, checks_total) = checks(fm.body);
    Ticket {
        id: stem.to_string(),
        title,
        status,
        size,
        milestone,
        area,
        depends,
        checks_done,
        checks_total,
        extra: f.into_iter().map(|(k, v)| (k, v.into_text())).collect(),
        problem: (!problems.is_empty()).then(|| problems.join("; ")),
    }
}

/// `text` with its front matter's `status:` line set to `status`, and nothing else touched: every other line,
/// its line ending, and a BOM stay as they were, so the change is one line in a diff. Inserts the line after
/// `title:` (or at the end of the front matter) when there isn't one. Refused without front matter, since
/// there's nowhere to put the status.
pub fn set_status(text: &str, status: &str) -> Result<String, String> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let mut lines: Vec<String> = body.split_inclusive('\n').map(str::to_owned).collect();
    if lines.first().map(|l| trim_eol(l)) != Some("---") {
        return Err("the ticket has no front matter to hold `status`".into());
    }
    let close = (1..lines.len())
        .find(|&i| trim_eol(&lines[i]).trim_end() == "---")
        .ok_or("the front matter is not closed with `---`")?;
    // The file's own line ending, for a line we insert.
    let eol = if lines[0].ends_with("\r\n") { "\r\n" } else { "\n" };

    let (mut status_at, mut title_at) = (None, None);
    for (i, line) in lines.iter().enumerate().take(close).skip(1) {
        if let Some((key, _)) = trim_eol(line).split_once(':') {
            match key.trim() {
                "status" => status_at = Some(i),
                "title" => title_at = title_at.or(Some(i)),
                _ => {}
            }
        }
    }
    let value = render_scalar(status);
    match status_at {
        Some(i) => {
            // Keep the line's indentation, key and line ending; replace only the value.
            let line = trim_eol(&lines[i]).to_owned();
            let key = &line[..line.find(':').unwrap_or(0)];
            let tail = lines[i][line.len()..].to_owned();
            lines[i] = format!("{key}: {value}{tail}");
        }
        None => {
            let at = title_at.map_or(close, |t| t + 1);
            lines.insert(at, format!("status: {value}{eol}"));
        }
    }
    Ok(format!("{bom}{}", lines.concat()))
}

/// A status as a scalar the front-matter parser reads back unchanged: bare when that's safe, double-quoted
/// otherwise.
fn render_scalar(v: &str) -> String {
    let bare = !v.is_empty() && v == v.trim() && !v.contains(|c: char| "\"'[]#:,\\".contains(c));
    if bare { v.to_owned() } else { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) }
}

fn trim_eol(l: &str) -> &str {
    l.strip_suffix('\n').map_or(l, |l| l.strip_suffix('\r').unwrap_or(l))
}

fn heading(body: &str) -> Option<&str> {
    body.lines().find_map(|l| l.strip_prefix("# ")).map(str::trim).filter(|t| !t.is_empty())
}

/// `(done, total)` of the `- [x]` / `- [ ]` items.
fn checks(body: &str) -> (u32, u32) {
    body.lines().fold((0, 0), |(done, total), l| {
        let l = l.trim_start();
        match (l.starts_with("- [x]") || l.starts_with("- [X]"), l.starts_with("- [ ]")) {
            (true, _) => (done + 1, total + 1),
            (_, true) => (done, total + 1),
            _ => (done, total),
        }
    })
}

impl Projection for Tickets {
    fn name(&self) -> &'static str {
        "tickets"
    }
    fn version(&self) -> u32 {
        1
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS tickets (
                key          TEXT PRIMARY KEY,   -- <project>/<path>
                project      TEXT NOT NULL,
                path         TEXT NOT NULL,
                id           TEXT NOT NULL,
                title        TEXT NOT NULL,
                status       TEXT NOT NULL,
                size         TEXT NOT NULL,
                milestone    TEXT NOT NULL,
                area         TEXT NOT NULL,
                depends      TEXT NOT NULL,      -- JSON array
                checks_done  INTEGER NOT NULL,
                checks_total INTEGER NOT NULL,
                extra        TEXT NOT NULL,      -- JSON object
                problem      TEXT,
                updated_at   INTEGER NOT NULL
             ) STRICT;
             CREATE INDEX IF NOT EXISTS tickets_by_project ON tickets(project);",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS tickets;")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> Result<()> {
        if ev.kind != "file.changed" {
            return Ok(());
        }
        let Some(project) = ev.stream.strip_prefix("p/").filter(|p| !p.contains('/')) else { return Ok(()) };
        let Some(f) = parse::<FileChanged>(ev) else { return Ok(()) };
        let Some(stem) = ticket_stem(&f.path) else { return Ok(()) };
        let key = format!("{project}/{}", f.path);

        // The previous row is still in the table here, for when a status change should emit a signal.
        match f.text {
            None => {
                db.execute("DELETE FROM tickets WHERE key = ?1", [&key])?;
            }
            Some(text) => {
                let t = parse_ticket(stem, &text);
                db.execute(
                    "INSERT INTO tickets (key, project, path, id, title, status, size, milestone, area, depends,
                                          checks_done, checks_total, extra, problem, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                     ON CONFLICT(key) DO UPDATE SET
                        id = ?4, title = ?5, status = ?6, size = ?7, milestone = ?8, area = ?9, depends = ?10,
                        checks_done = ?11, checks_total = ?12, extra = ?13, problem = ?14, updated_at = ?15",
                    params![
                        key,
                        project,
                        f.path,
                        t.id,
                        t.title,
                        t.status,
                        t.size,
                        t.milestone,
                        t.area,
                        serde_json::to_string(&t.depends)?,
                        t.checks_done,
                        t.checks_total,
                        serde_json::to_string(&t.extra)?,
                        t.problem,
                        ev.ts
                    ],
                )?;
            }
        }
        out.push(Change::new(VIEW, key, Some(project.to_string())));
        Ok(())
    }
}

const SELECT: &str = "SELECT key, project, path, id, title, status, size, milestone, area, depends,
                             checks_done, checks_total, problem, updated_at FROM tickets";

fn row(r: &Row) -> rusqlite::Result<TicketRow> {
    let depends: String = r.get(9)?;
    Ok(TicketRow {
        key: r.get(0)?,
        project: r.get::<_, String>(1)?.as_str().into(),
        path: r.get(2)?,
        id: r.get(3)?,
        title: r.get(4)?,
        status: r.get(5)?,
        size: r.get(6)?,
        milestone: r.get(7)?,
        area: r.get(8)?,
        depends: serde_json::from_str(&depends)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e)))?,
        checks_done: r.get(10)?,
        checks_total: r.get(11)?,
        problem: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

/// Tickets in natural order of id (AT-9 before AT-10).
pub fn list(c: &Connection, project: Option<&str>) -> Result<Vec<TicketRow>> {
    let mut rows = match project {
        Some(p) => c
            .prepare_cached(&format!("{SELECT} WHERE project = ?1"))?
            .query_map([p], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        None => c.prepare_cached(SELECT)?.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?,
    };
    rows.sort_by(|a, b| natural_cmp(&a.id, &b.id).then_with(|| a.key.cmp(&b.key)));
    Ok(rows)
}

pub fn get(c: &Connection, key: &str) -> Result<Option<TicketRow>> {
    Ok(c.prepare_cached(&format!("{SELECT} WHERE key = ?1"))?.query_row([key], row).optional()?)
}

/// Compares runs of digits by value and everything else bytewise.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while let (Some(&x), Some(&y)) = (a.first(), b.first()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let (na, nb) = (
                a.iter().take_while(|c| c.is_ascii_digit()).count(),
                b.iter().take_while(|c| c.is_ascii_digit()).count(),
            );
            let (da, db) = (trim_zeros(&a[..na]), trim_zeros(&b[..nb]));
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            (a, b) = (&a[na..], &b[nb..]);
        } else {
            if x != y {
                return x.cmp(&y);
            }
            (a, b) = (&a[1..], &b[1..]);
        }
    }
    a.len().cmp(&b.len())
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|&&c| c == b'0').count();
    &d[n..]
}

#[cfg(test)]
mod tests {
    use atelier_core::{Actor, NewEvent};
    use atelier_store::Store;

    use super::*;

    fn change(path: &str, text: Option<&str>) -> NewEvent {
        NewEvent::new("p/P1", Actor::Atelier, &FileChanged { path: path.into(), text: text.map(Into::into) })
    }

    fn ticket(id: &str, status: &str) -> String {
        format!("---\nid: {id}\ntitle: \"T {id}\"\nstatus: {status}\n---\n\n## Acceptance\n- [x] a\n- [ ] b\n")
    }

    #[test]
    fn every_ticket_in_the_repo_parses() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.atelier/tickets");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let Some(stem) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| ticket_stem(&format!(".atelier/tickets/{n}")).map(str::to_string))
            else {
                continue;
            };
            let t = parse_ticket(&stem, &std::fs::read_to_string(&p).unwrap());
            // Id mismatches are legitimate data problems (and reported); nothing else may be wrong.
            assert!(
                t.problem.as_deref().is_none_or(|p| p.starts_with("front matter says id")),
                "{stem}: {:?}",
                t.problem
            );
            assert!(!t.title.is_empty() && !t.status.is_empty(), "{stem}: {t:?}");
            assert!(t.extra.keys().all(|k| k == "parent"), "{stem}: {:?}", t.extra);
            n += 1;
        }
        assert!(n > 30);
    }

    #[test]
    fn parses_fields_checks_and_fallback_title() {
        let t = parse_ticket(
            "AT-5",
            "---\nid: AT-5\ntitle: \"`ask`: sync\"\nstatus: ' in progress '\nsize: M\narea: server, proto\ndepends: [AT-1, AT-2]\nparent: AT-3\n---\n- [x] a\n  - [X] b\n- [ ] c\n",
        );
        assert_eq!(
            (t.title.as_str(), t.status.as_str(), t.size.as_str(), t.area.as_str()),
            ("`ask`: sync", "in progress", "M", "server, proto")
        );
        assert_eq!(
            (t.depends, t.checks_done, t.checks_total, t.problem),
            (vec!["AT-1".into(), "AT-2".into()], 2, 3, None)
        );
        assert_eq!(t.extra["parent"], "AT-3");

        let t = parse_ticket("note", "# A heading\ntext\n");
        assert_eq!((t.title.as_str(), t.status.as_str()), ("A heading", ""));
        assert_eq!(parse_ticket("bare", "no heading").title, "bare");
        // A broken fence is a problem on the row, not a lost ticket.
        let t = parse_ticket("AT-6", "---\nid: AT-6\n# T\n");
        assert!(t.problem.unwrap().contains("not closed"));
    }

    #[test]
    fn path_filter() {
        assert_eq!(ticket_stem(".atelier/tickets/AT-1.md"), Some("AT-1"));
        for p in [
            ".atelier/tickets/README.md",
            ".atelier/tickets/a/b.md",
            ".atelier/tickets/x.txt",
            ".atelier/notes/n.md",
            ".atelier/tickets/.md",
        ] {
            assert_eq!(ticket_stem(p), None, "{p}");
        }
    }

    /// Lines of `a` and `b` that differ, as (index, line of `a`, line of `b`). Panics if the counts differ.
    fn changed_lines<'a>(a: &'a str, b: &'a str) -> Vec<(usize, &'a str, &'a str)> {
        let (x, y): (Vec<_>, Vec<_>) = (a.split_inclusive('\n').collect(), b.split_inclusive('\n').collect());
        assert_eq!(x.len(), y.len(), "line count changed");
        x.into_iter().zip(y).enumerate().filter(|(_, (p, q))| p != q).map(|(i, (p, q))| (i, p, q)).collect()
    }

    #[test]
    fn set_status_changes_only_the_status_line_of_every_real_ticket() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.atelier/tickets");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_str().unwrap().to_owned();
            let rel = format!(".atelier/tickets/{name}");
            let Some(stem) = ticket_stem(&rel) else { continue };
            let text = std::fs::read_to_string(&p).unwrap();
            let before = parse_ticket(stem, &text);

            // Writing the status it already has changes nothing.
            assert_eq!(set_status(&text, &before.status).unwrap(), text, "{stem}");

            let moved = set_status(&text, "zz-moved").unwrap();
            let changed = changed_lines(&text, &moved);
            assert_eq!(changed.len(), 1, "{stem}: {changed:?}");
            assert!(changed[0].1.trim_start().starts_with("status:"), "{stem}: {changed:?}");
            let after = parse_ticket(stem, &moved);
            assert_eq!(after.status, "zz-moved");
            assert_eq!(
                (after.title, after.checks_total, after.depends, after.problem),
                (before.title, before.checks_total, before.depends, before.problem),
                "{stem}"
            );
            n += 1;
        }
        assert!(n > 30);
    }

    #[test]
    fn set_status_inserts_when_missing_and_keeps_line_endings_and_bom() {
        assert_eq!(
            set_status("---\nid: X\ntitle: \"T\"\nsize: M\n---\nbody", "open").unwrap(),
            "---\nid: X\ntitle: \"T\"\nstatus: open\nsize: M\n---\nbody"
        );
        assert_eq!(set_status("---\nid: X\n---\n", "open").unwrap(), "---\nid: X\nstatus: open\n---\n");
        assert_eq!(
            set_status("---\r\nid: X\r\ntitle: T\r\n---\r\nbody\r\n", "open").unwrap(),
            "---\r\nid: X\r\ntitle: T\r\nstatus: open\r\n---\r\nbody\r\n"
        );
        assert_eq!(
            set_status("---\r\nid: X\r\nstatus: open\r\n---\r\nbody\r\n", "done").unwrap(),
            "---\r\nid: X\r\nstatus: done\r\n---\r\nbody\r\n"
        );
        assert_eq!(set_status("\u{feff}---\nstatus: a\n---\n", "b").unwrap(), "\u{feff}---\nstatus: b\n---\n");
    }

    #[test]
    fn set_status_quotes_values_the_parser_would_misread() {
        for status in ["in: progress", "[x]", "a # b", "say \"hi\"", " padded "] {
            let text = set_status("---\nid: X\nstatus: open\n---\n", status).unwrap();
            assert_eq!(parse_ticket("X", &text).status, status.trim(), "{text}");
        }
    }

    #[test]
    fn set_status_refuses_text_without_a_usable_front_matter() {
        assert!(set_status("# no front matter\n", "open").is_err());
        assert!(set_status("---\nid: X\n", "open").is_err());
    }

    #[test]
    fn natural_order() {
        let mut v = ["AT-10", "AT-9", "AT-010x", "AT-2", "B-1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["AT-2", "AT-9", "AT-10", "AT-010x", "B-1"]);
    }

    #[tokio::test]
    async fn create_update_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("t.db"), vec![Box::new(Tickets)]).unwrap();
        let p = |n: &str| format!(".atelier/tickets/{n}.md");
        let ids = || store.read(|c| Ok(list(c, Some("P1"))?.into_iter().map(|t| (t.id, t.status)).collect::<Vec<_>>()));

        store.append_one(change(&p("AT-10"), Some(&ticket("AT-10", "open")))).await.unwrap();
        store.append_one(change(&p("AT-9"), Some(&ticket("AT-9", "open")))).await.unwrap();
        // Not tickets.
        store.append_one(change(".atelier/tickets/README.md", Some("# Tickets"))).await.unwrap();
        store.append_one(change(".atelier/notes/n.md", Some("x"))).await.unwrap();
        assert_eq!(ids().await.unwrap(), [("AT-9".into(), "open".into()), ("AT-10".into(), "open".into())]);

        // A status change reports only its own row.
        let mut commits = store.subscribe();
        store.append_one(change(&p("AT-9"), Some(&ticket("AT-9", "done")))).await.unwrap();
        let commit = commits.recv().await.unwrap();
        assert_eq!(commit.changes.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["P1/.atelier/tickets/AT-9.md"]);
        let row = store.read(|c| get(c, "P1/.atelier/tickets/AT-9.md")).await.unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.checks_done, row.checks_total), ("done", 1, 2));
        assert_eq!(ids().await.unwrap()[1].1, "open");

        // Id mismatch keeps the stem and flags it.
        store.append_one(change(&p("AT-34"), Some(&ticket("AT-33", "open")))).await.unwrap();
        let row = store.read(|c| get(c, "P1/.atelier/tickets/AT-34.md")).await.unwrap().unwrap();
        assert_eq!(row.id, "AT-34");
        assert!(row.problem.unwrap().contains("AT-33"));

        store.append_one(change(&p("AT-9"), None)).await.unwrap();
        assert!(store.read(|c| get(c, "P1/.atelier/tickets/AT-9.md")).await.unwrap().is_none());
        assert_eq!(ids().await.unwrap().len(), 2);
    }
}
