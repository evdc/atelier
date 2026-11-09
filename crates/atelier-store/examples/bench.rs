//! Store benchmark. Run by hand, in release mode:
//!
//! ```text
//! cargo run --release -p atelier-store --example bench
//! ```
//!
//! Writes its databases to a temp dir. Catch-up pages are 500 events, matching
//! the subscription loop in `atelier-server`, so the numbers are what a client
//! catching up actually pays.

use std::time::{Duration, Instant};

use anyhow::Result;
use atelier_core::stream::StreamFilter;
use atelier_core::{Actor, Event, EventBody, NewEvent};
use atelier_store::{Change, Projection, Store};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Catch-up page size used by subscriptions in `atelier-server`.
const PAGE: usize = 500;

const PROJECTS: usize = 10;
const SESSIONS: usize = 10;
/// 10 projects x 10 sessions x 10k events = 1M events; one project is 100k.
const PER_SESSION: usize = 10_000;

#[derive(Serialize, Deserialize)]
struct Tick {
    n: i64,
}
impl EventBody for Tick {
    const KIND: &'static str = "bench.tick";
}

fn tick(stream: &str, n: i64) -> NewEvent {
    NewEvent::new(stream, Actor::User, &Tick { n })
}

/// A cheap stand-in for a real projection: one upsert per event.
struct PerStream {
    version: u32,
}

impl Projection for PerStream {
    fn name(&self) -> &'static str {
        "bench_per_stream"
    }
    fn version(&self) -> u32 {
        self.version
    }
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS bench_per_stream (stream TEXT PRIMARY KEY, n INTEGER NOT NULL) STRICT",
        )
    }
    fn reset(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute_batch("DROP TABLE IF EXISTS bench_per_stream")
    }
    fn apply(&mut self, db: &Connection, ev: &Event, _out: &mut Vec<Change>) -> anyhow::Result<()> {
        db.execute(
            "INSERT INTO bench_per_stream (stream, n) VALUES (?1, 1)
             ON CONFLICT(stream) DO UPDATE SET n = n + 1",
            params![ev.stream],
        )?;
        Ok(())
    }
}

fn open(path: &std::path::Path, version: u32) -> Result<(Store, Duration)> {
    let t = Instant::now();
    let store = Store::open(path, vec![Box::new(PerStream { version })])?;
    Ok((store, t.elapsed()))
}

fn report(label: &str, events: usize, elapsed: Duration) {
    let secs = elapsed.as_secs_f64();
    println!("{label}: {events} events in {:.1} ms ({:.0} events/s)", secs * 1e3, events as f64 / secs);
}

async fn append_throughput(path: &std::path::Path) -> Result<()> {
    let (store, _) = open(path, 1)?;

    const SINGLE: usize = 20_000;
    let t = Instant::now();
    for i in 0..SINGLE {
        store.append_one(tick("p/P0/s/S0", i as i64)).await?;
    }
    report("append, 1 appender, 1 event per call", SINGLE, t.elapsed());

    const CONCURRENT: usize = 128;
    const PER_APPENDER: usize = 200;
    let t = Instant::now();
    let mut handles = Vec::with_capacity(CONCURRENT);
    for c in 0..CONCURRENT {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let stream = format!("p/P{}/s/S{c}", c % PROJECTS);
            for i in 0..PER_APPENDER {
                store.append_one(tick(&stream, i as i64)).await?;
            }
            anyhow::Ok(())
        }));
    }
    for h in handles {
        h.await??;
    }
    report("append, 128 concurrent appenders, 1 event per call", CONCURRENT * PER_APPENDER, t.elapsed());
    Ok(())
}

/// Fill the log with 1M events, interleaved across streams the way concurrent
/// sessions would be, so per-stream seqs are spread through the log.
async fn load(path: &std::path::Path) -> Result<()> {
    let (store, _) = open(path, 1)?;
    let streams: Vec<String> =
        (0..PROJECTS).flat_map(|p| (0..SESSIONS).map(move |s| format!("p/P{p}/s/S{s}"))).collect();

    const ROUNDS_PER_APPEND: usize = 10;
    let total = streams.len() * PER_SESSION;
    let t = Instant::now();
    for round in (0..PER_SESSION).step_by(ROUNDS_PER_APPEND) {
        let mut events = Vec::with_capacity(streams.len() * ROUNDS_PER_APPEND);
        for r in round..round + ROUNDS_PER_APPEND {
            for s in &streams {
                events.push(tick(s, r as i64));
            }
        }
        store.append(events).await?;
    }
    report("load (1M events, projection at v1)", total, t.elapsed());
    Ok(())
}

/// Catch up from seq 1 in pages of `PAGE`. Stops early once `budget` is spent.
struct CatchUp {
    events: usize,
    total: Duration,
    first_page: Duration,
    worst_page: Duration,
    complete: bool,
}

async fn catch_up(store: &Store, filter: StreamFilter, budget: Duration) -> Result<CatchUp> {
    let start = Instant::now();
    let mut next = 1;
    let mut out = CatchUp {
        events: 0,
        total: Duration::ZERO,
        first_page: Duration::ZERO,
        worst_page: Duration::ZERO,
        complete: false,
    };
    loop {
        let t = Instant::now();
        let page = store.events(filter.clone(), next, PAGE).await?;
        let took = t.elapsed();
        if out.events == 0 {
            out.first_page = took;
        }
        out.worst_page = out.worst_page.max(took);
        let Some(last) = page.last().map(|e| e.seq) else {
            out.complete = true;
            break;
        };
        out.events += page.len();
        next = last + 1;
        if page.len() < PAGE {
            out.complete = true;
            break;
        }
        if start.elapsed() > budget {
            break;
        }
    }
    out.total = start.elapsed();
    Ok(out)
}

fn print_catch_up(label: &str, c: &CatchUp) {
    let done = if c.complete { "" } else { " (stopped at time budget, incomplete)" };
    println!(
        "{label}: {} events in {:.1} ms, first page {:.2} ms, worst page {:.2} ms{done}",
        c.events,
        c.total.as_secs_f64() * 1e3,
        c.first_page.as_secs_f64() * 1e3,
        c.worst_page.as_secs_f64() * 1e3,
    );
}

async fn catch_up_benchmarks(path: &std::path::Path) -> Result<()> {
    let (store, _) = open(path, 2)?;
    let session = StreamFilter::Stream("p/P0/s/S0".into());
    let project = StreamFilter::Prefix("p/P0/".into());
    let everything = StreamFilter::Prefix("p/".into());
    let generous = Duration::from_secs(120);

    let c = catch_up(&store, session, generous).await?;
    print_catch_up("catch-up exact stream p/P0/s/S0 (10k of 1M)", &c);
    let c = catch_up(&store, project, generous).await?;
    print_catch_up("catch-up prefix p/P0/ (100k of 1M)", &c);
    let c = catch_up(&store, everything, Duration::from_secs(60)).await?;
    print_catch_up("catch-up prefix p/ (1M of 1M)", &c);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    println!("databases in {}", dir.path().display());

    append_throughput(&dir.path().join("throughput.db")).await?;

    let path = dir.path().join("catchup.db");
    load(&path).await?;
    // Reopening at a new projection version rebuilds it from the log.
    let (store, rebuild) = open(&path, 2)?;
    report("projection rebuild (1M events, version bump)", 1_000_000, rebuild);
    drop(store);

    catch_up_benchmarks(&path).await?;
    Ok(())
}
