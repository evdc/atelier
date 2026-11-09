use super::*;
use atelier_core::stream::StreamFilter;
use atelier_core::{Actor, EventBody};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Ping {
    n: i64,
}
impl EventBody for Ping {
    const KIND: &'static str = "test.ping";
}

/// Sums `n` per stream.
struct Sums {
    version: u32,
}
impl Projection for Sums {
    fn name(&self) -> &'static str {
        "sums"
    }
    fn version(&self) -> u32 {
        self.version
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("CREATE TABLE IF NOT EXISTS sums (stream TEXT PRIMARY KEY, total INTEGER NOT NULL)")
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS sums")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> anyhow::Result<()> {
        if let Some(Ok(Ping { n })) = ev.parse::<Ping>() {
            if n < 0 {
                anyhow::bail!("negative");
            }
            db.execute(
                "INSERT INTO sums VALUES (?1, ?2) ON CONFLICT(stream) DO UPDATE SET total = total + ?2",
                params![ev.stream, n],
            )?;
            out.push(Change::new("sums", ev.stream.clone(), None));
        }
        Ok(())
    }
}

fn ping(stream: &str, n: i64) -> NewEvent {
    NewEvent::new(stream, Actor::User, &Ping { n })
}

async fn total(store: &Store, stream: &'static str) -> i64 {
    store
        .read(move |c| Ok(c.query_row("SELECT total FROM sums WHERE stream = ?1", [stream], |r| r.get(0))?))
        .await
        .unwrap()
}

#[tokio::test]
async fn append_read_project_and_feed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("a.db"), vec![Box::new(Sums { version: 1 })]).unwrap();
    let mut feed = store.subscribe();

    let evs = store.append(vec![ping("p/x/s/1", 2), ping("p/x/s/2", 3)]).await.unwrap();
    assert_eq!(evs.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
    store.append_one(ping("p/y/s/1", 5)).await.unwrap();
    assert_eq!(store.head(), 3);

    let commit = feed.recv().await.unwrap();
    assert_eq!(commit.events.len(), 2);
    assert_eq!(commit.changes.len(), 2);

    let px = store.events(StreamFilter::Prefix("p/x/".into()), 1, 100).await.unwrap();
    assert_eq!(px.len(), 2);
    let one = store.events(StreamFilter::Stream("p/x/s/2".into()), 1, 100).await.unwrap();
    assert_eq!(one[0].seq, 2);
    assert_eq!(total(&store, "p/y/s/1").await, 5);
}

#[tokio::test]
async fn failing_projection_rejects_only_that_append() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("a.db"), vec![Box::new(Sums { version: 1 })]).unwrap();
    let (a, b, c) =
        tokio::join!(store.append_one(ping("s", 1)), store.append_one(ping("s", -1)), store.append_one(ping("s", 10)),);
    assert!(a.is_ok() && b.is_err() && c.is_ok());
    assert_eq!(total(&store, "s").await, 11);
    assert_eq!(store.events(StreamFilter::Stream("s".into()), 1, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn projection_catches_up_and_rebuilds_on_version_bump() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    {
        let store = Store::open(&path, vec![Box::new(Sums { version: 1 })]).unwrap();
        store.append(vec![ping("s", 1), ping("s", 2)]).await.unwrap();
    }
    // Same version: picks up where it left off, no double counting.
    {
        let store = Store::open(&path, vec![Box::new(Sums { version: 1 })]).unwrap();
        assert_eq!(total(&store, "s").await, 3);
        store.append_one(ping("s", 4)).await.unwrap();
    }
    // New version: rebuilt from the log.
    let store = Store::open(&path, vec![Box::new(Sums { version: 2 })]).unwrap();
    assert_eq!(total(&store, "s").await, 7);
}

/// Prefix catch-up must return exactly what `StreamFilter::matches` selects,
/// in seq order, however it is paged. Streams are picked to trip a naive
/// prefix match: `p/xy/` vs `p/x/`, project-level and empty-project streams.
#[tokio::test]
async fn prefix_catch_up_matches_stream_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("a.db"), vec![]).unwrap();
    let streams = ["p/x/s/1", "p/x/s/2", "p/xy/s/1", "p/x", "p/x/", "p/", "p//s/1", "global"];
    for round in 0..40 {
        store.append(streams.iter().map(|s| ping(s, round)).collect()).await.unwrap();
    }

    let all = store.events(StreamFilter::Prefix(String::new()), 1, 10_000).await.unwrap();
    assert_eq!(all.len(), streams.len() * 40);
    for prefix in ["", "p/", "p/x/", "p/xy/", "p/x", "p/x/s/", "p/x/s/1", "p//", "p/z/", "global"] {
        let filter = StreamFilter::Prefix(prefix.into());
        for from in [1, 50, 200] {
            let want: Vec<Seq> =
                all.iter().filter(|e| e.seq >= from && filter.matches(&e.stream)).map(|e| e.seq).collect();
            let mut got = Vec::new();
            let mut next = from;
            loop {
                let page = store.events(filter.clone(), next, 7).await.unwrap();
                let Some(last) = page.last().map(|e| e.seq) else { break };
                got.extend(page.iter().map(|e| e.seq));
                next = last + 1;
            }
            assert_eq!(got, want, "prefix {prefix:?} from {from}");
        }
    }
}

/// A log written before the `project` column existed gets it backfilled on open.
#[tokio::test]
async fn legacy_database_gets_project_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE events (
                seq INTEGER PRIMARY KEY, stream TEXT NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL,
                actor TEXT NOT NULL, cause INTEGER, body TEXT NOT NULL
             ) STRICT;
             CREATE INDEX events_by_stream ON events(stream, seq);",
        )
        .unwrap();
        for s in ["p/a/s/1", "p/ab/s/1", "p/a/s/2", "global"] {
            c.execute(
                "INSERT INTO events (stream, ts, kind, actor, body) VALUES (?1, 0, 'test.ping', 'user', '{\"n\":1}')",
                params![s],
            )
            .unwrap();
        }
    }

    let store = Store::open(&path, vec![]).unwrap();
    let px = store.events(StreamFilter::Prefix("p/a/".into()), 1, 10).await.unwrap();
    assert_eq!(px.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 3]);
    let unset = store
        .read(|c| {
            Ok(c.query_row("SELECT COUNT(*) FROM events WHERE stream GLOB 'p/*/*' AND project IS NULL", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .await
        .unwrap();
    assert_eq!(unset, 0);

    store.append_one(ping("p/a/s/3", 1)).await.unwrap();
    assert_eq!(store.events(StreamFilter::Prefix("p/a/".into()), 1, 10).await.unwrap().len(), 3);
    drop(store);

    // Reopening is a no-op: nothing is re-backfilled or duplicated.
    let store = Store::open(&path, vec![]).unwrap();
    assert_eq!(store.events(StreamFilter::Prefix("p/a/".into()), 1, 10).await.unwrap().len(), 3);
}
