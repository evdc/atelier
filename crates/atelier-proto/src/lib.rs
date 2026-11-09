//! The client ⇄ server protocol, spoken as JSON text frames over one
//! WebSocket per client. TypeScript types are generated from this file
//! (`make gen`); `web/src/lib/conn.ts` is the client side.
//!
//! Three primitives, all resumable:
//!
//! - **sub** — raw events of a stream (or stream prefix) from a seq, then
//!   live. A reconnecting client re-subscribes from `last_seen + 1`.
//!   Ephemeral events (token deltas) are delivered live only.
//! - **watch** — a server-side view (a projection table): a full snapshot,
//!   then row upserts/removals as they change. Cheap for lists.
//! - **rpc** — commands. They never return state; state arrives through
//!   subs and watches, so every client converges on the same log.

pub mod local;

use std::sync::Arc;

use atelier_core::domain::{AgentKind, PermissionDecision, Workspace, WorkspaceMode};
use atelier_core::stream::StreamFilter;
use atelier_core::{Event, ProjectId, Seq, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use ts_rs::TS;

/// Bump on any incompatible change to this file.
pub const PROTOCOL: u32 = 1;

pub type SubId = u32;
pub type WatchId = u32;
pub type RpcId = u32;

// ─── client → server ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Must be the first frame.
    Hello {
        protocol: u32,
        /// Free-form client description, for logs.
        client: String,
        #[serde(default)]
        #[ts(optional)]
        token: Option<String>,
    },
    Sub {
        id: SubId,
        filter: StreamFilter,
        /// First seq wanted (1 = from the beginning).
        #[ts(type = "number")]
        from: Seq,
    },
    Unsub {
        id: SubId,
    },
    Watch {
        id: WatchId,
        view: ViewName,
        /// Narrow to one partition (e.g. a project id for `sessions`).
        #[serde(default)]
        #[ts(optional)]
        scope: Option<String>,
    },
    Unwatch {
        id: WatchId,
    },
    Rpc {
        id: RpcId,
        call: Call,
    },
}

/// Commands. Each is one method; the server replies with `result` or `error`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "method", content = "params")]
pub enum Call {
    /// Add a directory on the server as a project.
    #[serde(rename = "project.add")]
    ProjectAdd {
        path: String,
        #[serde(default)]
        #[ts(optional)]
        name: Option<String>,
    },
    #[serde(rename = "project.remove")]
    ProjectRemove { project: ProjectId },

    /// Create a session and send its first message. Result: `{ id }`.
    #[serde(rename = "session.create")]
    SessionCreate {
        project: ProjectId,
        prompt: String,
        #[serde(default)]
        workspace: WorkspaceMode,
        #[serde(default)]
        #[ts(optional)]
        title: Option<String>,
        #[serde(default)]
        #[ts(optional)]
        model: Option<String>,
    },
    /// Send a message; starts (or resumes) the agent process if needed.
    #[serde(rename = "session.send")]
    SessionSend { session: SessionId, text: String },
    #[serde(rename = "session.interrupt")]
    SessionInterrupt { session: SessionId },
    #[serde(rename = "session.answer")]
    SessionAnswer { session: SessionId, request_id: String, decision: PermissionDecision },
    /// Stop the agent process. The session stays and can be resumed.
    #[serde(rename = "session.stop")]
    SessionStop { session: SessionId },
    /// Archive a session and, when it's safe, remove its worktree and branch.
    /// Result: [`Archived`].
    #[serde(rename = "session.archive")]
    SessionArchive { session: SessionId },
    /// What the session changed in its workspace. Result: [`Changes`].
    #[serde(rename = "session.changes")]
    SessionChanges { session: SessionId },
    /// The unified diff of one changed file. Result: `{ patch }`.
    #[serde(rename = "session.diff")]
    SessionDiff { session: SessionId, path: String },
    /// Directories inside `path` on the server (default: its user's home), for
    /// the add-project picker. Result: [`DirListing`].
    #[serde(rename = "fs.list")]
    FsList {
        #[serde(default)]
        #[ts(optional)]
        path: Option<String>,
        /// Include dot-directories.
        #[serde(default)]
        #[ts(optional)]
        hidden: Option<bool>,
    },
    /// Recently changed git repositories in the usual places under the
    /// server's home. Result: [`Suggestions`].
    #[serde(rename = "fs.suggest")]
    FsSuggest {},
}

/// Result of `fs.list`. Directories only, capped at 500 entries.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DirListing {
    pub path: String,
    /// `None` at the filesystem root.
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Has a `.git` entry (a repository, or a worktree).
    pub is_git: bool,
}

/// Result of `fs.suggest`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Suggestions {
    pub entries: Vec<Suggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Suggestion {
    pub path: String,
    pub name: String,
}

/// Result of `session.archive`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Archived {
    /// The worktree and branch were deleted.
    pub removed: bool,
    /// Why they were kept, when they were.
    #[serde(default)]
    #[ts(optional)]
    pub kept: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    /// Not tracked by git yet.
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileChange {
    pub path: String,
    pub status: ChangeStatus,
    pub added: u32,
    pub removed: u32,
}

/// Result of `session.changes`: a snapshot of the workspace against its base.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Changes {
    pub base: String,
    pub files: Vec<FileChange>,
}

// ─── server → client ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
        server: String,
        #[ts(type = "number")]
        head: Seq,
    },
    /// A batch of persisted events for a sub, in seq order.
    Events {
        sub: SubId,
        events: Vec<Arc<Event>>,
    },
    /// The sub has delivered all history; what follows is live.
    Live {
        sub: SubId,
    },
    /// An ephemeral event (not persisted, never replayed).
    Eph {
        sub: SubId,
        stream: String,
        kind: String,
        #[ts(type = "unknown")]
        body: Arc<RawValue>,
    },
    /// The full current contents of a watched view (also re-sent after a lag).
    Snapshot {
        watch: WatchId,
        #[ts(type = "unknown[]")]
        rows: Vec<Value>,
    },
    /// Rows that changed since the last snapshot/patch.
    Patch {
        watch: WatchId,
        #[ts(type = "unknown[]")]
        upsert: Vec<Value>,
        remove: Vec<String>,
    },
    Result {
        id: RpcId,
        #[ts(type = "unknown")]
        ok: Value,
    },
    Error {
        id: RpcId,
        error: String,
    },
    /// The connection is about to close (bad hello, protocol mismatch…).
    Fatal {
        error: String,
    },
}

// ─── views ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ViewName {
    /// Rows: [`ProjectRow`]. Unscoped.
    Projects,
    /// Rows: [`SessionRow`]. Scope: project id.
    Sessions,
    /// Rows: [`UsageRow`], at most one. Unscoped.
    Usage,
}

impl ViewName {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewName::Projects => "projects",
            ViewName::Sessions => "sessions",
            ViewName::Usage => "usage",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectRow {
    pub id: ProjectId,
    pub name: String,
    pub path: String,
    pub is_git: bool,
    #[ts(type = "number")]
    pub added_at: i64,
    /// Last activity in any of its sessions.
    #[ts(type = "number")]
    pub updated_at: i64,
    pub sessions: u32,
    /// Sessions currently running a turn.
    pub running: u32,
    /// Sessions blocked on a person.
    pub waiting: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// A turn is in progress.
    Running,
    /// Blocked on a permission request or question.
    Waiting,
    /// Ready for the next message.
    Idle,
    /// The last turn errored or the process died mid-turn.
    Failed,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Running => "running",
            SessionStatus::Waiting => "waiting",
            SessionStatus::Idle => "idle",
            SessionStatus::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "failed" => Self::Failed,
            _ => Self::Idle,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionRow {
    pub id: SessionId,
    pub project: ProjectId,
    /// The session's event stream (`p/<project>/s/<id>`).
    pub stream: String,
    pub title: String,
    pub agent: AgentKind,
    pub workspace: Workspace,
    pub model: Option<String>,
    pub status: SessionStatus,
    /// Whether an agent process is attached right now.
    pub alive: bool,
    /// Unanswered permission requests / questions.
    pub pending: u32,
    pub turns: u32,
    pub cost_usd: f64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub archived: bool,
}

/// Subscription usage, from the latest `cc.rate_limit_event` per window across
/// all sessions. The row is absent until a window has been reported.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageRow {
    /// Always `global`: the view has a single row.
    pub id: String,
    pub five_hour: Option<LimitWindow>,
    pub seven_day: Option<LimitWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LimitWindow {
    /// Fraction of the window's allowance used, when the vendor reports it.
    #[ts(type = "number | null")]
    pub utilization: Option<f64>,
    /// Unix milliseconds when the window resets, when reported.
    #[ts(type = "number | null")]
    pub resets_at: Option<i64>,
}
