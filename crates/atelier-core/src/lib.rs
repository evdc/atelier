//! Atelier's event model.
//!
//! Everything that happens is an immutable [`Event`] in an append-only log;
//! all state is a projection of that log. This crate is pure (no IO): it
//! defines the envelope, ids, stream names and the typed bodies of the
//! events Atelier itself writes (see [`domain`]).

pub mod domain;
pub mod event;
pub mod ids;
pub mod stream;

pub use event::{Actor, Event, EventBody, NewEvent, Seq};
pub use ids::{ProjectId, SessionId};
