//! Agents starting sessions: the pure parts (workspace parameter, limits, the child's first message).
//! The `spawn` tool in [`crate::mcp::tools`] wires them to `session.create`.

use atelier_core::SessionId;
use atelier_core::domain::WorkspaceMode;
use atelier_proto::{SessionRow, SessionStatus};
use serde_json::Value;

use crate::config::SpawnLimits;

/// Where a spawned session works, as the caller asked for it (`self` already resolved to the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// A new worktree at a snapshot of that session's working tree.
    Branch(SessionId),
    /// That session's worktree, read-only.
    Share(SessionId),
    /// A new worktree from the project's HEAD.
    Fresh,
    /// The project checkout.
    Project,
}

impl Wanted {
    /// The `session.create` pair: workspace mode and the session it comes from.
    pub fn mode_and_from(&self) -> (WorkspaceMode, Option<SessionId>) {
        match self {
            Wanted::Branch(f) => (WorkspaceMode::Worktree, Some(f.clone())),
            Wanted::Share(f) => (WorkspaceMode::Shared, Some(f.clone())),
            Wanted::Fresh => (WorkspaceMode::Worktree, None),
            Wanted::Project => (WorkspaceMode::Direct, None),
        }
    }
}

const WORKSPACE_FORMS: &str =
    r#"expected "fresh", "project", {"branch": "self" | <session id>} or {"share": "self" | <session id>}"#;

/// Parses the tool's `workspace` argument. Absent means branching from the caller.
pub fn parse_workspace(v: Option<&Value>, me: &SessionId) -> Result<Wanted, String> {
    let who = |s: &str| if s == "self" { me.clone() } else { SessionId::from(s) };
    match v {
        None | Some(Value::Null) => Ok(Wanted::Branch(me.clone())),
        Some(Value::String(s)) => match s.as_str() {
            "fresh" => Ok(Wanted::Fresh),
            "project" => Ok(Wanted::Project),
            other => Err(format!("workspace `{other}`: {WORKSPACE_FORMS}")),
        },
        Some(Value::Object(o)) if o.len() == 1 => match (o.get("branch"), o.get("share")) {
            (Some(Value::String(s)), _) if !s.is_empty() => Ok(Wanted::Branch(who(s))),
            (_, Some(Value::String(s))) if !s.is_empty() => Ok(Wanted::Share(who(s))),
            _ => Err(format!("workspace: {WORKSPACE_FORMS}")),
        },
        Some(_) => Err(format!("workspace: {WORKSPACE_FORMS}")),
    }
}

/// How many spawn levels `id` is below a session a person started. Cycles can't be logged, but stop anyway.
pub fn depth(rows: &[SessionRow], id: &SessionId) -> u32 {
    let mut depth = 0;
    let mut at = id;
    while let Some(parent) = rows.iter().find(|r| &r.id == at).and_then(|r| r.parent.as_ref()) {
        depth += 1;
        if depth > 64 {
            break;
        }
        at = parent;
    }
    depth
}

/// Running or waiting. A child that hasn't had its first turn yet counts too: it is `idle` only until the
/// supervisor logs its first message, and spawning in a burst must not slip past the limit in that gap.
fn is_live(r: &SessionRow) -> bool {
    matches!(r.status, SessionStatus::Running | SessionStatus::Waiting)
        || (r.status == SessionStatus::Idle && r.turns == 0 && !r.imported)
}

/// Whether `parent` may start another child. The refusal names the limit and how to raise it.
pub fn check_limits(rows: &[SessionRow], parent: &SessionId, limits: &SpawnLimits) -> Result<(), String> {
    let child_depth = depth(rows, parent) + 1;
    if child_depth > limits.max_depth {
        return Err(format!(
            "spawn refused: sessions may be at most {} levels deep and this would be level {child_depth}. \
             Do the work yourself or ask your parent; to raise the limit set ATELIER_SPAWN_DEPTH for the server.",
            limits.max_depth
        ));
    }
    let live = rows.iter().filter(|r| r.parent.as_ref() == Some(parent)).filter(|r| !r.archived && is_live(r)).count();
    if live >= limits.max_live_children as usize {
        return Err(format!(
            "spawn refused: you already have {live} children running or waiting (the limit is {}). \
             Wait for one to finish or archive one; to raise the limit set ATELIER_SPAWN_CHILDREN for the server.",
            limits.max_live_children
        ));
    }
    Ok(())
}

/// What the child is told about where it came from, ahead of the task.
pub struct Envelope<'a> {
    pub parent: &'a SessionId,
    pub parent_title: &'a str,
    /// One sentence on the workspace, e.g. "a new worktree branched from …".
    pub workspace: &'a str,
    /// Whether the `send` tool exists to report back with.
    pub can_send: bool,
}

pub fn envelope(e: &Envelope, task: &str) -> String {
    let report = if e.can_send {
        format!("Report back with `send` (to={}) when you're done, or just finish; ", e.parent)
    } else {
        "Report your results in your final message when you're done; ".to_owned()
    };
    format!(
        "You were started by Atelier session {} (\"{}\").\nYour workspace: {}\n{report}your parent can ask you \
         follow-ups.\n\nTask:\n{task}",
        e.parent,
        e.parent_title.split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'"),
        e.workspace,
    )
}

/// `spawn { review }`'s task. Mirrors `reviewPrompt` in `web/src/lib/review.ts`.
pub fn review_prompt(title: &str, base: Option<&str>, branch: Option<&str>, report_to: Option<&SessionId>) -> String {
    let (base, branch) = (base.unwrap_or("HEAD"), branch.unwrap_or("HEAD"));
    let report = match report_to {
        Some(id) => format!(" Send your findings to session {id} with `send`."),
        None => String::new(),
    };
    format!(
        "Review the changes in this worktree (base `{base}`, branch `{branch}`) made by session \"{title}\". \
         Read the diff with `git diff {base}`, run the tests, and report problems by severity with file:line. \
         Don't edit files.{report}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_core::ProjectId;
    use atelier_core::domain::{AgentKind, Workspace};
    use serde_json::json;

    fn row(id: &str, parent: Option<&str>, status: SessionStatus, archived: bool) -> SessionRow {
        SessionRow {
            id: id.into(),
            project: ProjectId::generate(),
            stream: String::new(),
            title: id.into(),
            agent: AgentKind::ClaudeCode,
            workspace: Workspace {
                mode: WorkspaceMode::Worktree,
                path: "/w".into(),
                branch: None,
                base: None,
                from: None,
                snapshot: None,
            },
            model: None,
            status,
            alive: false,
            pending: 0,
            turns: 0,
            cost_usd: 0.0,
            created_at: 0,
            updated_at: 0,
            archived,
            imported: false,
            parent: parent.map(Into::into),
        }
    }

    #[test]
    fn workspace_defaults_to_branching_from_the_caller() {
        let me = SessionId::from("me");
        assert_eq!(parse_workspace(None, &me), Ok(Wanted::Branch(me.clone())));
        assert_eq!(parse_workspace(Some(&json!({"branch": "self"})), &me), Ok(Wanted::Branch(me.clone())));
        assert_eq!(parse_workspace(Some(&json!({"share": "self"})), &me), Ok(Wanted::Share(me.clone())));
        assert_eq!(parse_workspace(Some(&json!({"share": "abc"})), &me), Ok(Wanted::Share("abc".into())));
        assert_eq!(parse_workspace(Some(&json!("fresh")), &me), Ok(Wanted::Fresh));
        assert_eq!(parse_workspace(Some(&json!("project")), &me), Ok(Wanted::Project));
    }

    #[test]
    fn workspace_rejects_what_it_cannot_read() {
        let me = SessionId::from("me");
        for bad in [json!("self"), json!({}), json!({"share": 3}), json!({"share": "a", "branch": "b"}), json!(7)] {
            assert!(parse_workspace(Some(&bad), &me).is_err(), "{bad}");
        }
    }

    #[test]
    fn modes() {
        let s = SessionId::from("x");
        assert_eq!(Wanted::Branch(s.clone()).mode_and_from(), (WorkspaceMode::Worktree, Some(s.clone())));
        assert_eq!(Wanted::Share(s.clone()).mode_and_from(), (WorkspaceMode::Shared, Some(s)));
        assert_eq!(Wanted::Fresh.mode_and_from(), (WorkspaceMode::Worktree, None));
        assert_eq!(Wanted::Project.mode_and_from(), (WorkspaceMode::Direct, None));
    }

    #[test]
    fn depth_counts_parents() {
        use SessionStatus::Idle;
        let rows = vec![
            row("a", None, Idle, false),
            row("b", Some("a"), Idle, false),
            row("c", Some("b"), Idle, false),
            row("d", Some("c"), Idle, false),
        ];
        assert_eq!(depth(&rows, &"a".into()), 0);
        assert_eq!(depth(&rows, &"d".into()), 3);
        let limits = SpawnLimits::default();
        assert!(check_limits(&rows, &"c".into(), &limits).is_ok()); // a child at level 3
        let err = check_limits(&rows, &"d".into(), &limits).unwrap_err();
        assert!(err.contains("3 levels") && err.contains("ATELIER_SPAWN_DEPTH"), "{err}");
    }

    #[test]
    fn only_live_unarchived_children_count() {
        use SessionStatus::*;
        let limits = SpawnLimits::default();
        let mut rows = vec![row("p", None, Idle, false)];
        for (i, st) in [Running, Waiting, Running].into_iter().enumerate() {
            rows.push(row(&format!("c{i}"), Some("p"), st, false));
        }
        let mut done = row("done", Some("p"), Idle, false);
        done.turns = 1;
        rows.push(done);
        rows.push(row("gone", Some("p"), Running, true));
        rows.push(row("other", Some("q"), Running, false));
        assert!(check_limits(&rows, &"p".into(), &limits).is_ok()); // 3 live
        // A child still starting (idle, no turn yet) is live too.
        let starting = row("starting", Some("p"), Idle, false);
        let mut with_starting = rows.clone();
        with_starting.push(starting);
        assert!(check_limits(&with_starting, &"p".into(), &limits).is_err());
        rows.push(row("c3", Some("p"), Running, false));
        let err = check_limits(&rows, &"p".into(), &limits).unwrap_err();
        assert!(err.contains("4 children") && err.contains("ATELIER_SPAWN_CHILDREN"), "{err}");
    }

    #[test]
    fn envelope_says_who_where_and_how_to_report() {
        let p = SessionId::from("p1");
        let e = Envelope { parent: &p, parent_title: "Build \"it\"", workspace: "a worktree", can_send: false };
        let t = envelope(&e, "do x");
        assert!(t.contains("session p1 (\"Build 'it'\")") && t.contains("final message") && t.ends_with("do x"));
        let t = envelope(&Envelope { can_send: true, ..e }, "do x");
        assert!(t.contains("`send` (to=p1)"));
    }
}
