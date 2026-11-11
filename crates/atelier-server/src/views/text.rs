//! The readable text of an event: what a person would say a session *said*.
//!
//! Shared by everything that shows or searches a transcript as text (the MCP `read` tool; session search).
//! Only conversation counts: user messages, the agent's prose, and one compact line per tool call. Tool
//! results, thinking and sub-agent chatter are the bulk of the log and noise for "which session was it".

use atelier_core::Event;
use atelier_core::domain::{SessionCreated, SessionRenamed, UserMessage};
use serde::Deserialize;

/// Longest text of one document, in bytes (cut at a char boundary).
pub const MAX_DOC_BYTES: usize = 32 * 1024;
/// Longest tool-use line, in chars.
const MAX_TOOL_CHARS: usize = 200;
/// Tool input fields that say what a call was about, in order of preference.
const TOOL_TARGETS: [&str; 6] = ["file_path", "path", "command", "pattern", "url", "description"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Title,
    User,
    Agent,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Title => "title",
            Role::User => "user",
            Role::Agent => "agent",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub role: Role,
    pub text: String,
}

#[derive(Deserialize)]
struct Assistant {
    #[serde(default)]
    parent_tool_use_id: Option<String>,
    message: Option<AssistantMessage>,
}

#[derive(Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<Block>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Block {
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

/// The documents of one event, in order: at most one for the prose and one per tool call. Empty for every
/// kind that isn't conversation. Tolerates missing fields (the log keeps whatever the vendor sent).
pub fn docs(ev: &Event) -> Vec<Doc> {
    let one = |role, text: String| {
        let text = cap(text.trim(), MAX_DOC_BYTES);
        if text.is_empty() { Vec::new() } else { vec![Doc { role, text }] }
    };
    if let Some(Ok(m)) = ev.parse::<UserMessage>() {
        one(Role::User, m.text)
    } else if let Some(Ok(c)) = ev.parse::<SessionCreated>() {
        one(Role::Title, c.title)
    } else if let Some(Ok(r)) = ev.parse::<SessionRenamed>() {
        one(Role::Title, r.title)
    } else if ev.kind == "cc.assistant" {
        assistant(ev)
    } else {
        Vec::new()
    }
}

fn assistant(ev: &Event) -> Vec<Doc> {
    let Ok(a) = serde_json::from_str::<Assistant>(ev.body.get()) else { return Vec::new() };
    if a.parent_tool_use_id.is_some() {
        return Vec::new();
    }
    let blocks = a.message.map(|m| m.content).unwrap_or_default();
    let prose: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } if !text.trim().is_empty() => Some(text.trim()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    if !prose.is_empty() {
        out.push(Doc { role: Role::Agent, text: cap(&prose.join("\n\n"), MAX_DOC_BYTES) });
    }
    for b in &blocks {
        if let Block::ToolUse { name, input } = b {
            let line = tool_line(name, input);
            if !line.is_empty() {
                out.push(Doc { role: Role::Tool, text: line });
            }
        }
    }
    out
}

/// `Read src/main.rs`: the tool and the first of its usual targets, on one line.
fn tool_line(name: &str, input: &serde_json::Value) -> String {
    let target = TOOL_TARGETS.iter().find_map(|k| input.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()));
    let line = match target {
        Some(t) => format!("{name} {}", t.split_whitespace().collect::<Vec<_>>().join(" ")),
        None => name.to_owned(),
    };
    line.trim().chars().take(MAX_TOOL_CHARS).collect()
}

/// `s` cut to at most `max` bytes, on a char boundary.
pub fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json, value::RawValue};

    pub fn ev(kind: &str, body: Value) -> Event {
        Event {
            seq: 1,
            stream: "p/p1/s/s1".into(),
            ts: 0,
            kind: kind.into(),
            actor: "user".into(),
            cause: None,
            body: RawValue::from_string(body.to_string()).unwrap(),
        }
    }

    fn texts(ev: &Event) -> Vec<(Role, String)> {
        docs(ev).into_iter().map(|d| (d.role, d.text)).collect()
    }

    #[test]
    fn user_and_titles() {
        assert_eq!(texts(&ev("user.message", json!({"text": " fix the WAL "}))), [(Role::User, "fix the WAL".into())]);
        let renamed = ev("session.renamed", json!({"title": "WAL checkpoint"}));
        assert_eq!(texts(&renamed), [(Role::Title, "WAL checkpoint".into())]);
        assert!(texts(&ev("user.message", json!({"text": "  "}))).is_empty());
    }

    #[test]
    fn assistant_prose_is_joined_and_tools_get_one_line_each() {
        let e = ev(
            "cc.assistant",
            json!({"message": {"content": [
                {"type": "thinking", "thinking": "secret"},
                {"type": "text", "text": "First."},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "src/a.rs", "path": "ignored"}},
                {"type": "text", "text": "Second."},
                {"type": "tool_use", "name": "Bash", "input": {"command": "cargo\n  test"}},
                {"type": "tool_use", "name": "Mystery", "input": {"x": 1}},
            ]}}),
        );
        assert_eq!(
            texts(&e),
            [
                (Role::Agent, "First.\n\nSecond.".into()),
                (Role::Tool, "Read src/a.rs".into()),
                (Role::Tool, "Bash cargo test".into()),
                (Role::Tool, "Mystery".into()),
            ]
        );
    }

    #[test]
    fn noise_is_not_text() {
        let sub = ev(
            "cc.assistant",
            json!({"parent_tool_use_id": "t1", "message": {"content": [{"type": "text", "text": "sub-agent"}]}}),
        );
        assert!(docs(&sub).is_empty());
        let thinking = ev("cc.assistant", json!({"message": {"content": [{"type": "thinking", "thinking": "hm"}]}}));
        assert!(docs(&thinking).is_empty());
        let result = ev("cc.user", json!({"message": {"content": [{"type": "tool_result", "content": "big"}]}}));
        assert!(docs(&result).is_empty());
        assert!(docs(&ev("cc.assistant", json!({"unexpected": true}))).is_empty());
        assert!(docs(&ev("cc.system", json!({}))).is_empty());
    }

    #[test]
    fn long_text_is_capped_on_a_char_boundary() {
        let text = "é".repeat(MAX_DOC_BYTES);
        let d = docs(&ev("user.message", json!({ "text": text })));
        assert!(d[0].text.len() <= MAX_DOC_BYTES && d[0].text.chars().all(|c| c == 'é'));
        let long_cmd = "x".repeat(500);
        let t = docs(&ev(
            "cc.assistant",
            json!({"message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": long_cmd}}]}}),
        ));
        assert_eq!(t[0].text.chars().count(), MAX_TOOL_CHARS);
    }
}
