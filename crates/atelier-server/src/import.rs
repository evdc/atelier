//! Copies Claude Code's own session history (`<claude home>/projects/<slug>/<id>.jsonl`) into the log, when
//! a person asks (`project.import`). Nothing here runs by itself.
//!
//! Only sessions whose cwd is the project, or one of its git worktrees, are taken. Each file is remembered by
//! byte offset (folded from `import.progressed`), so importing again appends only what's new.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use atelier_core::domain::{AgentKind, ImportProgressed, Origin, SessionCreated, Workspace, WorkspaceMode};
use atelier_core::{Actor, NewEvent, SessionId, stream};
use atelier_driver::claude::strip_signatures;
use atelier_proto::{Imported, ProjectRow};
use serde_json::Value;

use crate::{App, rpc, views, workspace};

/// Events per append, so a big history doesn't starve live traffic.
const BATCH: usize = 500;

pub async fn project(app: &App, p: &ProjectRow) -> Result<Imported> {
    // One import at a time: two presses must not both see "not imported yet".
    let _guard = app.import_lock.lock().await;

    let root = PathBuf::from(&p.path);
    let mut cwds = vec![root.clone()];
    for w in workspace::worktrees(&root).await {
        if !cwds.contains(&w) {
            cwds.push(w);
        }
    }

    let mut total = Imported { sessions: 0, messages: 0 };
    for cwd in &cwds {
        let dir = app.config.claude_home.join("projects").join(claude_slug(&cwd.to_string_lossy()));
        for file in jsonl_files(&dir).await {
            let (sessions, messages) = import_file(app, p, &cwds, &file).await?;
            total.sessions += sessions;
            total.messages += messages;
        }
    }
    Ok(total)
}

/// Claude Code names a session's directory after its cwd with every non-alphanumeric character as `-`.
pub fn claude_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

async fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };
    let mut files = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "jsonl") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Returns (sessions created, messages appended).
async fn import_file(app: &App, p: &ProjectRow, cwds: &[PathBuf], file: &Path) -> Result<(u32, u32)> {
    let path_s = file.to_string_lossy().into_owned();
    let Some(native_id) = file.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return Ok((0, 0));
    };

    let known = {
        let path_s = path_s.clone();
        let native_id = native_id.clone();
        app.store
            .read(move |c| {
                Ok(match views::sessions::import_offset(c, &path_s)? {
                    Some(known) => Some(known),
                    // Atelier started this one itself: its events are already in the log.
                    None if views::sessions::has_native_id(c, &native_id)? => Some((String::new(), u64::MAX)),
                    None => None,
                })
            })
            .await?
    };
    let from = known.as_ref().map_or(0, |(_, off)| *off);
    if from == u64::MAX {
        return Ok((0, 0));
    }

    let f = file.to_owned();
    let scan = tokio::task::spawn_blocking(move || scan(&f, from)).await??;
    if scan.messages.is_empty() {
        return Ok((0, 0));
    }

    let (session, mut events, created) = match known {
        Some((session, _)) => (SessionId::from(session.as_str()), Vec::new(), false),
        None => {
            // `-` collapses distinct paths, so check the cwd the file itself records.
            let cwd = scan.cwd.as_deref().map(PathBuf::from);
            if cwd.as_ref().is_some_and(|c| !cwds.contains(c)) {
                return Ok((0, 0));
            }
            let id = SessionId::generate();
            let created = SessionCreated {
                id: id.clone(),
                project: p.id.clone(),
                title: scan.title.clone().unwrap_or_else(|| "Imported session".into()),
                agent: AgentKind::ClaudeCode,
                workspace: Workspace {
                    mode: WorkspaceMode::Direct,
                    from: None,
                    snapshot: None,
                    path: cwd.unwrap_or_else(|| PathBuf::from(&p.path)).to_string_lossy().into_owned(),
                    branch: scan.branch.clone(),
                    base: None,
                },
                model: None,
                origin: Some(Origin::Imported { native_id, path: path_s.clone() }),
                started_at: scan.messages.first().and_then(|m| m.at),
                parent: None,
            };
            let s_stream = stream::session(&p.id, &id);
            (id, vec![NewEvent::new(&s_stream, Actor::Atelier, &created)], true)
        }
    };

    let s_stream = stream::session(&p.id, &session);
    let count = scan.messages.len();
    let last = scan.messages.len().saturating_sub(1);
    for (i, m) in scan.messages.into_iter().enumerate() {
        events.push(NewEvent::raw(&s_stream, m.kind, Actor::Atelier, m.body));
        let batch_full = events.len() >= BATCH - 1;
        if batch_full || i == last {
            // The final batch also covers any trailing lines that weren't messages.
            let offset = if i == last { scan.end } else { m.end };
            let progressed = ImportProgressed { path: path_s.clone(), offset, at: m.at };
            events.push(NewEvent::new(&s_stream, Actor::Atelier, &progressed));
            app.store.append(std::mem::take(&mut events)).await?;
        }
    }
    Ok((u32::from(created), count as u32))
}

struct Message {
    kind: &'static str,
    body: Box<serde_json::value::RawValue>,
    /// Byte offset just past this line.
    end: u64,
    at: Option<i64>,
}

struct Scan {
    messages: Vec<Message>,
    /// Offset past the last complete line read.
    end: u64,
    title: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
}

/// Read the complete lines of `file` from byte `from`. A trailing partial line (the agent is mid-write) is
/// left for next time.
fn scan(file: &Path, from: u64) -> Result<Scan> {
    let mut f = std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    f.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;

    let mut out = Scan { messages: Vec::new(), end: from, title: None, cwd: None, branch: None };
    let mut pos = 0;
    while let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') {
        let line = &buf[pos..pos + nl];
        pos += nl + 1;
        out.end = from + pos as u64;
        let Ok(mut v) = serde_json::from_slice::<Value>(line) else { continue };

        if out.cwd.is_none() {
            out.cwd = v.get("cwd").and_then(Value::as_str).map(str::to_owned);
            out.branch = v.get("gitBranch").and_then(Value::as_str).map(str::to_owned);
        }
        // Other types are bookkeeping; sidechains belong to subagents (not shown yet).
        let kind = match v.get("type").and_then(Value::as_str) {
            Some("user") => "cc.user",
            Some("assistant") => "cc.assistant",
            _ => continue,
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if kind == "cc.assistant" {
            strip_signatures(&mut v);
        } else if out.title.is_none() && v.get("isMeta").and_then(Value::as_bool) != Some(true) {
            out.title = typed_text(&v).filter(|t| !t.starts_with('<')).map(|t| rpc::title_from(&t));
        }
        let at = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts_ms);
        let body = serde_json::value::to_raw_value(&v)?;
        out.messages.push(Message { kind, body, end: out.end, at });
    }
    Ok(out)
}

/// What the person typed in a user entry (not tool results).
fn typed_text(v: &Value) -> Option<String> {
    let content = v.pointer("/message/content")?;
    match content {
        Value::String(s) => Some(s.trim().to_owned()),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|b| b.get("text")?.as_str())
            .map(|s| s.trim().to_owned()),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// `2026-11-08T02:39:24.174Z` → Unix ms. Claude Code always writes UTC with a `Z`.
fn parse_ts_ms(s: &str) -> Option<i64> {
    let (date, time) = s.strip_suffix('Z')?.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.splitn(3, ':').map(|x| x.parse::<i64>().ok());
    let (h, mi, sec) = (t.next()??, t.next()??, t.next()??);
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_claude_codes() {
        assert_eq!(claude_slug("/Users/me/.atelier/repo_1"), "-Users-me--atelier-repo-1");
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_ts_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_ts_ms("2026-11-08T02:39:24.174Z"), Some(1_794_105_564_174));
        assert_eq!(parse_ts_ms("nope"), None);
    }
}
