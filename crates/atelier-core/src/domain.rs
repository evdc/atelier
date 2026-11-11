//! Bodies of the events Atelier itself writes.
//!
//! Vendor output is stored as-is under its own namespace (e.g. `cc.assistant`
//! for Claude Code) and interpreted by projections; it is not modelled here.
//!
//! Rules for changing this file: the log is immutable, so a body type may
//! only gain *optional* fields (`#[serde(default)]`). Anything else is a new
//! kind.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{EventBody, ProjectId, Seq, SessionId};

macro_rules! kind {
    ($t:ty, $k:literal) => {
        impl EventBody for $t {
            const KIND: &'static str = $k;
        }
    };
}

// ─── projects (stream: global) ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectAdded {
    pub id: ProjectId,
    pub name: String,
    /// Absolute path on the server.
    pub path: String,
    pub is_git: bool,
}
kind!(ProjectAdded, "project.added");

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectRemoved {
    pub id: ProjectId,
}
kind!(ProjectRemoved, "project.removed");

// ─── sessions (stream: p/<project>/s/<session>) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceMode {
    /// A fresh git worktree on its own branch (the default).
    #[default]
    Worktree,
    /// The project directory itself.
    Direct,
    /// Another session's worktree, read-only.
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Workspace {
    pub mode: WorkspaceMode,
    /// Where the agent runs.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Commit the worktree was created from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The session this workspace came from: the owner for `Shared`, the snapshot source for a branched `Worktree`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<SessionId>,
    /// For a branched `Worktree`, the snapshot commit it started from (also its `base`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionCreated {
    pub id: SessionId,
    pub project: ProjectId,
    pub title: String,
    pub agent: AgentKind,
    pub workspace: Workspace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where the session came from, when Atelier didn't start it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// Unix ms the conversation really began, for imports (an event's `ts` is
    /// when it was appended, not when it happened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    /// The session that started this one (`spawn`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}
kind!(SessionCreated, "session.created");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Origin {
    /// Read from the agent's own history on disk.
    Imported {
        /// The agent's session id (what `claude --resume` takes).
        native_id: String,
        /// The history file it was read from.
        path: String,
    },
}

/// Part of an imported history file was copied into the session's stream.
/// `offset` is the byte position just past the last line imported, so a later
/// import continues from there.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportProgressed {
    pub path: String,
    #[ts(type = "number")]
    pub offset: u64,
    /// Unix ms of the newest message copied, so the session sorts by when it was last active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub at: Option<i64>,
}
kind!(ImportProgressed, "import.progressed");

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionRenamed {
    pub title: String,
}
kind!(SessionRenamed, "session.renamed");

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionArchived {}
kind!(SessionArchived, "session.archived");

/// The person put an archived session back in their list. It does not restore a worktree
/// that archiving removed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionUnarchived {}
kind!(SessionUnarchived, "session.unarchived");

/// A session's branch was deleted and it let go of its worktree (on archive, when nothing
/// would be lost). The directory is deleted too unless `pooled`, in which case it waits,
/// detached, for the next session.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceRemoved {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub pooled: bool,
}
kind!(WorkspaceRemoved, "workspace.removed");

/// A message from the person to the agent. Starts a turn.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UserMessage {
    pub text: String,
}
kind!(UserMessage, "user.message");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum MessageMode {
    /// Fire and forget: the target may reply with its own `send`.
    Tell,
    /// The sender is waiting for the target's answer (`ask`).
    Ask,
}

/// A message from another session, in the *target's* stream. The sender's side is already in its own
/// transcript as the tool call. Starts a turn, like `user.message`, but is not the person and not a reason
/// to notify them. `actor` is `agent:<from>`; `cause` is the sender's `cc.assistant` event that made the call.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentMessage {
    pub from: SessionId,
    /// What the sender wrote, without the envelope the target sees.
    pub text: String,
    pub mode: MessageMode,
    /// One more than the hops of the input that started the sender's turn; a person's message counts as 0.
    pub hops: u32,
    /// Events in the sender's stream the text points at.
    #[serde(default)]
    #[ts(type = "Array<number>")]
    pub refs: Vec<Seq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_id: Option<String>,
}
kind!(AgentMessage, "agent.message");

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UserInterrupt {}
kind!(UserInterrupt, "user.interrupt");

/// The agent process started. `native_id` is the agent's own session id
/// (what `claude --resume` takes), when known at spawn.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProcessStarted {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub native_id: Option<String>,
}
kind!(ProcessStarted, "process.started");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ExitReason {
    /// Atelier stopped it (idle, user stop, shutdown).
    Stopped,
    /// It exited on its own.
    Exited,
    /// The server went away while it was running; found on restart.
    ServerRestart,
    /// It failed to start.
    SpawnFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProcessExited {
    pub reason: ExitReason,
    #[serde(default)]
    pub code: Option<i32>,
    /// Last lines of stderr, for diagnosing crashes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_tail: String,
    /// Why, beyond `reason`: `"idle"` when the supervisor reaped an idle process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
kind!(ProcessExited, "process.exited");

/// The agent learned (or changed) its own session id, e.g. after a fork.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentBound {
    pub native_id: String,
}
kind!(AgentBound, "agent.bound");

/// A turn finished (successfully, with an error, or interrupted).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TurnEnded {
    pub is_error: bool,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub duration_ms: Option<u64>,
    /// The vendor's own end-of-turn record, verbatim.
    #[ts(type = "unknown")]
    pub raw: Value,
}
kind!(TurnEnded, "turn.ended");

/// The agent wants a person: a tool permission, or a question for the user.
///
/// Pending permission requests are void once their process exits
/// (`process.exited`); there is no separate cancellation event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionRequested {
    /// Opaque id used to answer it.
    pub request_id: String,
    pub tool: String,
    #[ts(type = "unknown")]
    pub input: Value,
    /// The vendor's request, verbatim (carries e.g. permission suggestions).
    #[ts(type = "unknown")]
    pub raw: Value,
}
kind!(PermissionRequested, "permission.requested");

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "behavior", rename_all = "kebab-case")]
pub enum PermissionDecision {
    Allow {
        /// Replaces the tool input (e.g. AskUserQuestion answers).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        updated_input: Option<Value>,
        /// Permission rules to apply with this allow, verbatim from the request's
        /// `permission_suggestions` ("always allow"). Forwarded as `updatedPermissions`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        updated_permissions: Option<Value>,
    },
    Deny {
        /// Shown to the agent as the reason the tool was refused.
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionAnswered {
    pub request_id: String,
    pub decision: PermissionDecision,
}
kind!(PermissionAnswered, "permission.answered");
