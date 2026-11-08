use std::sync::Arc;

use anyhow::Result;
use atelier_core::stream::{StreamFilter, prefix_end};
use atelier_core::{Event, Seq};
use rusqlite::{Connection, Row, params};
use serde_json::value::RawValue;

pub(crate) fn head(c: &Connection) -> Result<Seq> {
    Ok(c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?)
}

pub(crate) fn events(c: &Connection, filter: &StreamFilter, from: Seq, limit: usize) -> Result<Vec<Arc<Event>>> {
    const COLS: &str = "seq, stream, ts, kind, actor, cause, body";
    let limit = limit as i64;
    let rows = match filter {
        StreamFilter::Stream(s) => {
            let mut q = c.prepare_cached(&format!(
                "SELECT {COLS} FROM events WHERE stream = ?1 AND seq >= ?2 ORDER BY seq LIMIT ?3"
            ))?;
            q.query_map(params![s, from, limit], row)?.collect::<rusqlite::Result<Vec<_>>>()?
        }
        StreamFilter::Prefix(p) => {
            let mut q = c.prepare_cached(&format!(
                "SELECT {COLS} FROM events WHERE stream >= ?1 AND stream < ?2 AND seq >= ?3
                 ORDER BY seq LIMIT ?4"
            ))?;
            q.query_map(params![p, prefix_end(p), from, limit], row)?.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    Ok(rows.into_iter().map(Arc::new).collect())
}

/// All events from `from`, regardless of stream (projection rebuilds).
pub(crate) fn all_events(c: &Connection, from: Seq, limit: usize) -> Result<Vec<Event>> {
    let mut q = c.prepare_cached(
        "SELECT seq, stream, ts, kind, actor, cause, body FROM events WHERE seq >= ?1 ORDER BY seq LIMIT ?2",
    )?;
    Ok(q.query_map(params![from, limit as i64], row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn row(r: &Row) -> rusqlite::Result<Event> {
    let body: String = r.get(6)?;
    Ok(Event {
        seq: r.get(0)?,
        stream: r.get(1)?,
        ts: r.get(2)?,
        kind: r.get(3)?,
        actor: r.get(4)?,
        cause: r.get(5)?,
        // Bodies were validated as JSON on the way in.
        body: RawValue::from_string(body)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e)))?,
    })
}
