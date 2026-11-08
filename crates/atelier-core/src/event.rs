use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use ts_rs::TS;

/// Position in the global log. Monotonic, gap-free in practice, starts at 1.
/// It is also the sync cursor: a client that has seen `n` resumes from `n + 1`.
pub type Seq = i64;

/// A persisted event.
///
/// `body` is kept as raw JSON: the server relays it to clients without
/// re-encoding, and only parses it where a projection needs to.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Event {
    #[ts(type = "number")]
    pub seq: Seq,
    /// Partition key. See [`crate::stream`].
    pub stream: String,
    /// Unix milliseconds, assigned by the store on append.
    #[ts(type = "number")]
    pub ts: i64,
    /// Dotted kind, e.g. `session.created` or `cc.assistant`.
    pub kind: String,
    /// Who caused it: `user`, `atelier`, or `agent:<session id>`.
    pub actor: String,
    /// The event that caused this one, if any.
    #[ts(type = "number | null")]
    pub cause: Option<Seq>,
    #[ts(type = "unknown")]
    pub body: Box<RawValue>,
}

impl Event {
    /// Parse the body as `B` if this event is of `B`'s kind.
    ///
    /// `None` for other kinds. `Some(Err)` means the body doesn't match its
    /// own kind's schema (a bug, or an old schema): callers should log and
    /// skip rather than fail, since the log is immutable.
    pub fn parse<B: EventBody>(&self) -> Option<Result<B, serde_json::Error>> {
        (self.kind == B::KIND).then(|| serde_json::from_str(self.body.get()))
    }

    /// Parse the body as arbitrary JSON (for vendor events).
    pub fn json(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_str(self.body.get())
    }

    pub fn is<B: EventBody>(&self) -> bool {
        self.kind == B::KIND
    }
}

/// An event before it is appended (no seq or timestamp yet).
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub stream: String,
    pub kind: String,
    pub actor: String,
    pub cause: Option<Seq>,
    pub body: Box<RawValue>,
}

impl NewEvent {
    /// A typed Atelier event.
    pub fn new<B: EventBody>(stream: impl Into<String>, actor: Actor, body: &B) -> Self {
        let json = serde_json::to_string(body).expect("event bodies always serialize");
        Self::raw(stream, B::KIND, actor, RawValue::from_string(json).expect("valid json"))
    }

    /// An event whose body we don't own the schema of (e.g. vendor output).
    pub fn raw(stream: impl Into<String>, kind: impl Into<String>, actor: Actor, body: Box<RawValue>) -> Self {
        Self { stream: stream.into(), kind: kind.into(), actor: actor.to_string(), cause: None, body }
    }

    pub fn caused_by(mut self, seq: Seq) -> Self {
        self.cause = Some(seq);
        self
    }
}

/// Implemented by every typed event body. `KIND` is its `kind` string.
pub trait EventBody: Serialize + DeserializeOwned {
    const KIND: &'static str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User,
    Atelier,
    Agent(crate::SessionId),
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Actor::User => f.write_str("user"),
            Actor::Atelier => f.write_str("atelier"),
            Actor::Agent(id) => write!(f, "agent:{id}"),
        }
    }
}
