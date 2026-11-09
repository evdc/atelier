//! Agent-written session titles.
//!
//! After a session's first turn ends, a one-shot `claude -p` (Haiku, nothing
//! persisted) proposes a title from the first prompt and the start of the
//! first reply. It runs beside the session, never inside it, and its title is
//! dropped if the person renamed the session in the meantime.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde_json::Value;
use tokio::process::Command;

use crate::views;

const MODEL: &str = "haiku";
const TIMEOUT: Duration = Duration::from_secs(90);
/// How much of each side goes to the titler; a title needs no more.
const PROMPT_CHARS: usize = 800;
const REPLY_CHARS: usize = 1200;
const MAX_WORDS: usize = 6;
const MAX_CHARS: usize = 80;

/// What the titler sees of a session: its first message and its first reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub prompt: String,
    pub reply: String,
}

/// The first turn's exchange, if the session should be titled now: its first
/// turn has just ended and nobody has renamed it.
pub fn exchange(c: &Connection, session: &str, stream: &str) -> Result<Option<Exchange>> {
    let Some(row) = views::sessions::get(c, session)? else { return Ok(None) };
    if row.turns != 1 || renamed(c, stream)? {
        return Ok(None);
    }
    let mut prompt = None;
    let mut reply = String::new();
    let mut q = c.prepare(
        "SELECT kind, body FROM events
         WHERE stream = ?1 AND kind IN ('user.message', 'cc.assistant', 'turn.ended')
         ORDER BY seq",
    )?;
    for row in q.query_map([stream], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (kind, body) = row?;
        match kind.as_str() {
            "user.message" if prompt.is_none() => prompt = text_field(&body),
            "cc.assistant" => reply.push_str(&assistant_text(&body)),
            _ => break, // turn.ended: the first turn is over
        }
    }
    Ok(prompt.map(|prompt| Exchange { prompt, reply }))
}

/// Whether the person (or an earlier title) has already named this session.
pub fn renamed(c: &Connection, stream: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE stream = ?1 AND kind = 'session.renamed')",
        [stream],
        |r| r.get(0),
    )?)
}

/// The instruction for the titler. Pure, so the wording and the truncation are testable.
pub fn prompt(ex: &Exchange) -> String {
    format!(
        "Write a title for this coding session: at most {MAX_WORDS} words, plain text, no quotes and no \
         trailing punctuation. Reply with the title only.\n\n\
         First message:\n{}\n\nStart of the first reply:\n{}",
        truncate(&ex.prompt, PROMPT_CHARS),
        truncate(&ex.reply, REPLY_CHARS),
    )
}

/// The title in `claude -p --output-format json` output, or `None` if the run
/// failed or gave nothing usable.
pub fn parse(output: &str) -> Option<String> {
    let v: Value = serde_json::from_str(output).ok()?;
    if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let first_line = v.get("result")?.as_str()?.lines().map(str::trim).find(|l| !l.is_empty())?;
    let unquoted = first_line.trim_matches(|c: char| c.is_whitespace() || "\"'`*#_.:“”‘’".contains(c));
    let words: Vec<&str> = unquoted.split_whitespace().take(MAX_WORDS).collect();
    let title: String = words.join(" ").chars().take(MAX_CHARS).collect();
    (!title.is_empty()).then_some(title)
}

/// Run the one-shot titler and return its title, if it gave a usable one.
pub async fn ask(bin: &Path, ex: &Exchange) -> Result<Option<String>> {
    let mut cmd = Command::new(bin);
    cmd.args(["-p", "--model", MODEL, "--no-session-persistence", "--output-format", "json"])
        .arg(prompt(ex))
        // Outside any worktree, so the titler reads no project context.
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| anyhow!("no title within {TIMEOUT:?}"))?
        .context("running the titler")?;
    if !out.status.success() {
        anyhow::bail!("the titler exited with {}", out.status);
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

/// The `text` of a `user.message` body.
fn text_field(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let text = v.get("text")?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The text blocks of a verbatim `cc.assistant` body (tool use and thinking are skipped).
fn assistant_text(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return String::new() };
    let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else { return String::new() };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn prompt_carries_both_sides_and_truncates_long_ones() {
        // '#' appears nowhere in the instruction text, so counting it counts only the reply.
        let ex = Exchange { prompt: "Fix the login bug".into(), reply: "#".repeat(5000) };
        let p = prompt(&ex);
        assert!(p.contains("Fix the login bug"), "{p}");
        assert!(p.contains("at most 6 words"), "{p}");
        assert_eq!(p.chars().filter(|&c| c == '#').count(), REPLY_CHARS, "reply not truncated");
    }

    #[test]
    fn parse_takes_the_result_and_cleans_it() {
        let ok = |result: &str| json!({"type": "result", "is_error": false, "result": result}).to_string();
        assert_eq!(parse(&ok("Fix login rate limiting")).as_deref(), Some("Fix login rate limiting"));
        assert_eq!(parse(&ok("  \"Fix login rate limiting.\"  \n")).as_deref(), Some("Fix login rate limiting"));
        assert_eq!(parse(&ok("\n\nAdd CSV export\nBecause people asked")).as_deref(), Some("Add CSV export"));
        assert_eq!(
            parse(&ok("one two three four five six seven eight")).as_deref(),
            Some("one two three four five six")
        );
        assert_eq!(parse(&ok("   ")), None);
    }

    #[test]
    fn parse_rejects_failed_or_malformed_runs() {
        assert_eq!(parse(&json!({"is_error": true, "result": "Fix it"}).to_string()), None);
        assert_eq!(parse(&json!({"result": 3}).to_string()), None);
        assert_eq!(parse(&json!({"subtype": "error_max_turns"}).to_string()), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn exchange_text_comes_from_the_verbatim_bodies() {
        assert_eq!(text_field(r#"{"text":"  Say hi\nthen list  "}"#).as_deref(), Some("Say hi\nthen list"));
        assert_eq!(text_field(r#"{"text":"   "}"#), None);
        let body = json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "tool_use", "name": "Bash"},
            {"type": "text", "text": "Hi there"},
        ]}});
        assert_eq!(assistant_text(&body.to_string()), "Hi there");
    }
}
