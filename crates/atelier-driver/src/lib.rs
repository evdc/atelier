//! Agent drivers: the boundary between Atelier and a coding agent.
//!
//! A driver starts an agent process for a session and turns it into two
//! channels: commands in ([`AgentCmd`]), events out ([`DriverEvent`]). The
//! server persists the events and never needs to know which vendor produced
//! them — vendor output is stored verbatim under the vendor's namespace and
//! interpreted by projections/clients.
//!
//! Channels rather than an async trait keep drivers object-safe, make
//! back-pressure explicit and let a driver be a process, a remote service or
//! (later) Atelier's own agent loop.

pub mod claude;

use std::path::PathBuf;

use atelier_core::SessionId;
use atelier_core::domain::{AgentKind, PermissionDecision, PermissionRequested, ProcessExited, TurnEnded};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

pub use claude::ClaudeCode;

pub trait AgentDriver: Send + Sync + 'static {
    fn kind(&self) -> AgentKind;

    /// Spawn the agent. Must be called from within a Tokio runtime.
    fn start(&self, spec: StartSpec) -> anyhow::Result<AgentHandle>;
}

#[derive(Debug, Clone)]
pub struct StartSpec {
    pub session: SessionId,
    /// Working directory (the session's workspace).
    pub cwd: PathBuf,
    pub resume: Resume,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Resume {
    /// A brand-new conversation.
    New,
    /// Continue the agent's own session `native_id`.
    Continue { native_id: String },
    /// Branch from `native_id` into a new agent session, optionally
    /// truncated after message `at` (vendor message id).
    Fork { native_id: String, at: Option<String> },
}

pub struct AgentHandle {
    pub pid: Option<u32>,
    /// The agent's session id, if known before it starts talking.
    pub native_id: Option<String>,
    pub cmds: mpsc::UnboundedSender<AgentCmd>,
    /// Ends with exactly one [`DriverEvent::Exited`], then closes.
    pub events: mpsc::Receiver<DriverEvent>,
}

#[derive(Debug, Clone)]
pub enum AgentCmd {
    /// Start a turn with a user message.
    Send(String),
    /// Stop the current turn; the session continues.
    Interrupt,
    Answer {
        request_id: String,
        decision: PermissionDecision,
    },
    /// Stop the process (gracefully, then forcefully).
    Shutdown,
}

#[derive(Debug)]
pub enum DriverEvent {
    /// Persist verbatim. `kind` is namespaced by vendor (e.g. `cc.assistant`).
    Raw {
        kind: String,
        body: Box<RawValue>,
    },
    /// Relay live, don't persist (token deltas, status pings).
    Ephemeral {
        kind: String,
        body: Box<RawValue>,
    },
    /// The agent's own session id became known or changed (e.g. a fork).
    Bound {
        native_id: String,
    },
    TurnEnded(TurnEnded),
    Permission(PermissionRequested),
    Exited(ProcessExited),
}
