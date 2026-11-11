//! Messages between sessions (DESIGN §11.3): the pure parts. `session.send` from an agent wires them up in
//! [`crate::rpc`], and the `send` tool in [`crate::mcp::tools`] resolves its target with them.

use atelier_core::SessionId;
use atelier_core::domain::MessageMode;
use atelier_proto::SessionRow;

/// Longest message text, in bytes.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// A message may be at most this many hops from the person's last word.
pub const MAX_HOPS: u32 = 6;
/// Shortest id prefix `send` accepts.
pub const MIN_PREFIX: usize = 4;

const HOP_LIMIT: &str = "hop limit reached: this conversation needs the person. Tell them in your reply.";

/// The hops of a message sent by a session whose current turn began with an input of `last_input_hops`.
pub fn next_hops(last_input_hops: u32) -> Result<u32, String> {
    let hops = last_input_hops + 1;
    if hops > MAX_HOPS { Err(HOP_LIMIT.into()) } else { Ok(hops) }
}

/// Refusals that depend only on the text and the two ids.
pub fn check_text(from: &SessionId, to: &SessionId, text: &str) -> Result<(), String> {
    if from == to {
        return Err("you can't send a message to yourself".into());
    }
    if text.trim().is_empty() {
        return Err("message is empty".into());
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!("message is {} bytes; the limit is {MAX_TEXT_BYTES}", text.len()));
    }
    Ok(())
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'")
}

/// What the target agent reads as the turn's input. The text is the sender's and may contain anything, so it
/// can't be told apart from the envelope by the model except through the framing around it.
pub fn envelope(from: &SessionId, title: &str, project: &str, mode: MessageMode, text: &str) -> String {
    let mode = match mode {
        MessageMode::Tell => "tell",
        MessageMode::Ask => "ask",
    };
    format!(
        "<atelier-message from=\"{from}\" title=\"{}\" project=\"{}\" mode=\"{mode}\">\n{}\n</atelier-message>\n\
         (From another session, not the person. To reply, call send with to=\"{from}\". \
         Your reply text in this turn is not delivered.)",
        one_line(title),
        one_line(project),
        text.trim(),
    )
}

/// Finds the session `q` names: an exact id, or a prefix of at least [`MIN_PREFIX`] chars that only one id has.
pub fn resolve<'a>(rows: &'a [SessionRow], q: &str) -> Result<&'a SessionRow, String> {
    let q = q.trim();
    if let Some(r) = rows.iter().find(|r| r.id.as_str() == q) {
        return Ok(r);
    }
    if q.chars().count() < MIN_PREFIX {
        return Err(format!("`{q}` is too short: give at least {MIN_PREFIX} characters of the session id"));
    }
    let hits: Vec<&SessionRow> = rows.iter().filter(|r| r.id.as_str().starts_with(q)).collect();
    match hits.as_slice() {
        [] => Err(format!("no session with id `{q}` (use `sessions` to list them)")),
        [r] => Ok(r),
        many => {
            let list: Vec<String> =
                many.iter().take(10).map(|r| format!("{} · {}", r.id, one_line(&r.title))).collect();
            Err(format!("`{q}` matches {} sessions, use more of the id:\n{}", many.len(), list.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, title: &str) -> SessionRow {
        SessionRow {
            id: id.into(),
            project: "p".into(),
            stream: format!("p/p/s/{id}"),
            title: title.into(),
            agent: atelier_core::domain::AgentKind::ClaudeCode,
            workspace: atelier_core::domain::Workspace {
                mode: atelier_core::domain::WorkspaceMode::Direct,
                path: "/w".into(),
                branch: None,
                base: None,
                from: None,
                snapshot: None,
            },
            model: None,
            status: atelier_proto::SessionStatus::Idle,
            alive: false,
            pending: 0,
            turns: 0,
            cost_usd: 0.0,
            created_at: 0,
            updated_at: 0,
            archived: false,
            imported: false,
            parent: None,
        }
    }

    #[test]
    fn hops_count_up_and_stop_after_the_limit() {
        assert_eq!(next_hops(0), Ok(1));
        assert_eq!(next_hops(MAX_HOPS - 1), Ok(MAX_HOPS));
        let e = next_hops(MAX_HOPS).unwrap_err();
        assert!(e.starts_with("hop limit reached: this conversation needs the person."), "{e}");
    }

    #[test]
    fn text_refusals() {
        let (a, b): (SessionId, SessionId) = ("a".into(), "b".into());
        assert!(check_text(&a, &b, "hi").is_ok());
        assert!(check_text(&a, &a, "hi").unwrap_err().contains("yourself"));
        assert!(check_text(&a, &b, " \n").unwrap_err().contains("empty"));
        assert!(check_text(&a, &b, &"x".repeat(MAX_TEXT_BYTES)).is_ok());
        assert!(check_text(&a, &b, &"x".repeat(MAX_TEXT_BYTES + 1)).unwrap_err().contains("limit"));
    }

    #[test]
    fn envelope_frames_the_text_and_says_how_to_reply() {
        let e = envelope(&"7f3a".into(), "Fix \"WAL\"\n checkpoint", "atelier", MessageMode::Tell, " hello\nworld ");
        assert_eq!(
            e,
            "<atelier-message from=\"7f3a\" title=\"Fix 'WAL' checkpoint\" project=\"atelier\" mode=\"tell\">\n\
             hello\nworld\n</atelier-message>\n\
             (From another session, not the person. To reply, call send with to=\"7f3a\". \
             Your reply text in this turn is not delivered.)"
        );
    }

    #[test]
    fn prefixes_resolve_when_unique() {
        let rows = [row("7f3a1111", "One"), row("7f3b2222", "Two"), row("7f3a9999", "Three")];
        assert_eq!(resolve(&rows, "7f3b").unwrap().id.as_str(), "7f3b2222");
        assert_eq!(resolve(&rows, "7f3a9999").unwrap().id.as_str(), "7f3a9999");
        let amb = resolve(&rows, "7f3a").unwrap_err();
        assert!(amb.contains("7f3a1111 · One") && amb.contains("7f3a9999 · Three"), "{amb}");
        assert!(resolve(&rows, "7f3").unwrap_err().contains("too short"));
        assert!(resolve(&rows, "ffff").unwrap_err().contains("no session"));
        // An exact id wins even when it is also a prefix of another.
        let rows = [row("abcd", "Short"), row("abcdef", "Long")];
        assert_eq!(resolve(&rows, "abcd").unwrap().title, "Short");
    }
}
