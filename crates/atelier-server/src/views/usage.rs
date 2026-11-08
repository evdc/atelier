use std::collections::BTreeMap;

use anyhow::Result;
use atelier_core::Event;
use atelier_proto::{LimitWindow, UsageRow};
use atelier_store::{Change, Projection};
use rusqlite::{Connection, Row, params};
use serde::Deserialize;

use super::parse_raw;

pub const VIEW: &str = "usage";
/// The view's only row key.
pub const ROW: &str = "global";

/// The latest reading of each subscription window (`five_hour`, `seven_day`),
/// across every session. Claude Code reports these in `cc.rate_limit_event`.
///
/// Each event carries one window, so the projection keeps one row per window
/// rather than only the very latest event: otherwise a 5-hour reading would
/// hide the 7-day one until the next event of the other kind.
pub struct Usage;

#[derive(Deserialize)]
struct RateLimitEvent {
    rate_limit_info: Option<RateLimitInfo>,
}

/// The windows this projection shows.
const WINDOWS: [&str; 2] = ["five_hour", "seven_day"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitInfo {
    /// The one window this event is about. Only that window's reading is top-level.
    rate_limit_type: Option<String>,
    #[serde(flatten)]
    reading: Reading,
    /// Every window's reading at once. Real events name `five_hour` at the top
    /// and carry `seven_day` only here, so this is needed for the second meter.
    unified_windows: Option<BTreeMap<String, Reading>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reading {
    utilization: Option<f64>,
    /// Unix seconds.
    resets_at: Option<f64>,
}

impl Projection for Usage {
    fn name(&self) -> &'static str {
        "usage"
    }
    fn version(&self) -> u32 {
        1
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_windows (
                window      TEXT PRIMARY KEY,
                utilization REAL,
                resets_at   INTEGER
             ) STRICT;",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS usage_windows;")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> Result<()> {
        if ev.kind != "cc.rate_limit_event" {
            return Ok(());
        }
        let Some(RateLimitEvent { rate_limit_info: Some(info) }) = parse_raw(ev) else {
            return Ok(());
        };
        // Top-level reading first, so the unified map (same event, fuller) wins if both name a window.
        let mut readings: Vec<(String, Reading)> = Vec::new();
        if let Some(w) = info.rate_limit_type {
            readings.push((w, info.reading));
        }
        readings.extend(info.unified_windows.unwrap_or_default());

        let mut changed = false;
        for (window, r) in readings {
            // Per-model and overage windows are not shown.
            if !WINDOWS.contains(&window.as_str()) {
                continue;
            }
            let resets_at = r.resets_at.map(|s| (s * 1000.0) as i64);
            db.execute(
                "INSERT INTO usage_windows (window, utilization, resets_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(window) DO UPDATE SET utilization = ?2, resets_at = ?3",
                params![window, r.utilization, resets_at],
            )?;
            changed = true;
        }
        if changed {
            out.push(Change::new(VIEW, ROW, None));
        }
        Ok(())
    }
}

/// The view's single row, or none if no window has been reported yet.
pub fn list(c: &Connection) -> Result<Vec<UsageRow>> {
    let mut q = c.prepare_cached("SELECT window, utilization, resets_at FROM usage_windows")?;
    let mut row = UsageRow { id: ROW.to_owned(), five_hour: None, seven_day: None };
    let mut any = false;
    for r in q.query_map([], window_row)? {
        let (window, w) = r?;
        any = true;
        match window.as_str() {
            "five_hour" => row.five_hour = Some(w),
            _ => row.seven_day = Some(w),
        }
    }
    Ok(if any { vec![row] } else { vec![] })
}

fn window_row(r: &Row) -> rusqlite::Result<(String, LimitWindow)> {
    Ok((r.get(0)?, LimitWindow { utilization: r.get(1)?, resets_at: r.get(2)? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_core::{Event, Seq};
    use serde_json::value::RawValue;

    fn ev(seq: Seq, kind: &str, body: &str) -> Event {
        Event {
            seq,
            stream: "p/x/s/y".into(),
            ts: seq,
            kind: kind.into(),
            actor: "agent:y".into(),
            cause: None,
            body: RawValue::from_string(body.into()).unwrap(),
        }
    }

    fn rate(seq: Seq, window: &str, util: Option<f64>) -> Event {
        let util = util.map_or("null".to_owned(), |u| u.to_string());
        let body = format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"allowed","rateLimitType":"{window}","utilization":{util},"resetsAt":1800000000}}}}"#
        );
        ev(seq, "cc.rate_limit_event", &body)
    }

    fn fold(events: &[Event]) -> Vec<UsageRow> {
        let db = Connection::open_in_memory().unwrap();
        let mut p = Usage;
        p.migrate(&db).unwrap();
        let mut out = Vec::new();
        for e in events {
            p.apply(&db, e, &mut out).unwrap();
        }
        list(&db).unwrap()
    }

    #[test]
    fn empty_until_a_window_is_reported() {
        assert!(fold(&[ev(1, "cc.assistant", "{}")]).is_empty());
    }

    #[test]
    fn latest_reading_per_window_wins() {
        let rows =
            fold(&[rate(1, "five_hour", Some(0.2)), rate(2, "seven_day", Some(0.5)), rate(3, "five_hour", Some(0.7))]);
        let r = &rows[0];
        assert_eq!(r.id, ROW);
        assert_eq!(r.five_hour.as_ref().unwrap().utilization, Some(0.7));
        assert_eq!(r.five_hour.as_ref().unwrap().resets_at, Some(1_800_000_000_000));
        assert_eq!(r.seven_day.as_ref().unwrap().utilization, Some(0.5));
    }

    #[test]
    fn one_event_can_fill_both_meters_from_unified_windows() {
        // The shape real sessions emit: `five_hour` at the top level, both windows below.
        let body = r#"{"type":"rate_limit_event","rate_limit_info":{"rateLimitType":"five_hour","utilization":0.3,"resetsAt":1794115800,
            "unifiedWindows":{"five_hour":{"resetsAt":1794115800,"status":"allowed","utilization":0.3},
                              "seven_day":{"resetsAt":1794481200,"status":"allowed","utilization":0.37}}}}"#;
        let rows = fold(&[ev(1, "cc.rate_limit_event", body)]);
        let r = &rows[0];
        assert_eq!(r.five_hour.as_ref().unwrap().utilization, Some(0.3));
        assert_eq!(r.seven_day.as_ref().unwrap().utilization, Some(0.37));
        assert_eq!(r.seven_day.as_ref().unwrap().resets_at, Some(1_794_481_200_000));
    }

    #[test]
    fn unknown_utilization_is_kept_as_none() {
        let rows = fold(&[rate(1, "seven_day", None)]);
        assert_eq!(rows[0].seven_day.as_ref().unwrap().utilization, None);
        assert!(rows[0].five_hour.is_none());
    }

    #[test]
    fn other_windows_and_malformed_bodies_are_ignored() {
        let rows = fold(&[
            rate(1, "seven_day_opus", Some(0.9)),
            ev(2, "cc.rate_limit_event", r#"{"rate_limit_info":"nope"}"#),
            ev(3, "cc.rate_limit_event", "{}"),
        ]);
        assert!(rows.is_empty());
    }
}
