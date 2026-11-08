//! Server-side views: projections of the log into SQLite tables, and the
//! queries that read them.
//!
//! Adding a view:
//! 1. a [`Projection`] that folds events into a table and reports changed
//!    rows as [`atelier_store::Change`]s under the view's name;
//! 2. a row type in `atelier-proto` and a [`ViewName`] variant;
//! 3. `snapshot`/`row` arms below. Watches then work with no other changes.

pub mod projects;
pub mod sessions;
pub mod usage;

use anyhow::Result;
use atelier_proto::ViewName;
use atelier_store::Projection;
use rusqlite::Connection;
use serde_json::Value;

pub fn projections() -> Vec<Box<dyn Projection>> {
    // Order matters only for cross-table reads inside `apply`; none yet.
    vec![Box::new(projects::Projects), Box::new(sessions::Sessions), Box::new(usage::Usage)]
}

/// All rows of a view (narrowed to `scope`, if given), in display order.
pub fn snapshot(c: &Connection, view: ViewName, scope: Option<&str>) -> Result<Vec<Value>> {
    Ok(match view {
        ViewName::Projects => to_values(projects::list(c)?),
        ViewName::Sessions => to_values(sessions::list(c, scope)?),
        ViewName::Usage => to_values(usage::list(c)?),
    })
}

/// One row by key, or `None` if it no longer exists (→ the client removes it).
pub fn row(c: &Connection, view: ViewName, key: &str) -> Result<Option<Value>> {
    Ok(match view {
        ViewName::Projects => projects::get(c, key)?.map(to_value),
        ViewName::Sessions => sessions::get(c, key)?.map(to_value),
        ViewName::Usage => usage::list(c)?.into_iter().find(|r| r.id == key).map(to_value),
    })
}

fn to_value<T: serde::Serialize>(t: T) -> Value {
    serde_json::to_value(t).expect("rows serialize")
}

fn to_values<T: serde::Serialize>(ts: Vec<T>) -> Vec<Value> {
    ts.into_iter().map(to_value).collect()
}

/// Log-and-skip helper for projections: a malformed body is a bug or an old
/// schema, and must not wedge the log.
pub(crate) fn parse<B: atelier_core::EventBody>(ev: &atelier_core::Event) -> Option<B> {
    match ev.parse::<B>()? {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!("skipping malformed {} @{}: {e}", ev.kind, ev.seq);
            None
        }
    }
}

/// Like [`parse`], for vendor bodies that have no `EventBody` type: the
/// projection reads only the fields it needs and ignores the rest.
pub(crate) fn parse_raw<T: serde::de::DeserializeOwned>(ev: &atelier_core::Event) -> Option<T> {
    match serde_json::from_str(ev.body.get()) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("skipping malformed {} @{}: {e}", ev.kind, ev.seq);
            None
        }
    }
}
