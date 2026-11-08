//! The event store: an append-only SQLite log plus incrementally maintained
//! projections, and a feed of commits for live subscribers.
//!
//! - **One writer thread** owns the write connection. Appends from anywhere
//!   are queued to it and group-committed: everything queued at the moment a
//!   transaction starts goes into that transaction.
//! - **Projections** are folded in the same transaction as the events they
//!   see, so a projection is never behind (or ahead of) the log. Each one is
//!   versioned; on a version change it is dropped and refolded from seq 1.
//! - **Readers** use a small pool of read-only connections (WAL lets them run
//!   concurrently with the writer).
//! - After each commit, a [`Commit`] (the new events and the projection rows
//!   they changed) is broadcast to subscribers.

mod projection;
mod read;
mod writer;

pub use projection::{Change, Projection};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use atelier_core::{Event, NewEvent, Seq};
use rusqlite::Connection;
use tokio::sync::{broadcast, oneshot};

/// Broadcast to subscribers after every commit.
#[derive(Debug)]
pub struct Commit {
    pub events: Vec<Arc<Event>>,
    pub changes: Vec<Change>,
}

impl Commit {
    pub fn last_seq(&self) -> Option<Seq> {
        self.events.last().map(|e| e.seq)
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    writer: std::sync::mpsc::Sender<writer::Cmd>,
    readers: Mutex<Vec<Connection>>,
    feed: broadcast::Sender<Arc<Commit>>,
    head: Arc<AtomicI64>,
}

/// How many commits a slow subscriber may fall behind before it is told it
/// lagged (and must re-read from the database).
const FEED_CAPACITY: usize = 1024;

impl Store {
    /// Open (or create) the store at `path`, bringing every projection up to
    /// date with the log before returning.
    pub fn open(path: impl AsRef<Path>, projections: Vec<Box<dyn Projection>>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = open_conn(&path, false)?;
        schema(&conn)?;
        let head = Arc::new(AtomicI64::new(read::head(&conn)?));
        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        let writer = writer::spawn(conn, projections, feed.clone(), head.clone())?;
        Ok(Self { inner: Arc::new(Inner { path, writer, readers: Mutex::new(Vec::new()), feed, head }) })
    }

    /// Append events atomically. Returns them with their seq and timestamp.
    pub async fn append(&self, events: Vec<NewEvent>) -> Result<Vec<Arc<Event>>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let (reply, rx) = oneshot::channel();
        self.inner
            .writer
            .send(writer::Cmd::Append { events, reply })
            .map_err(|_| anyhow::anyhow!("store writer has stopped"))?;
        rx.await.context("store writer dropped the request")?
    }

    pub async fn append_one(&self, event: NewEvent) -> Result<Arc<Event>> {
        Ok(self.append(vec![event]).await?.pop().expect("one in, one out"))
    }

    /// Highest committed seq (0 if empty).
    pub fn head(&self) -> Seq {
        self.inner.head.load(Ordering::Acquire)
    }

    /// Live feed of commits. Subscribe *before* reading history, then skip
    /// events at or below the last seq you read, to never miss one.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Commit>> {
        self.inner.feed.subscribe()
    }

    /// Run a read-only query on a pooled connection, off the async runtime.
    pub async fn read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = match inner.readers.lock().unwrap().pop() {
                Some(c) => c,
                None => open_conn(&inner.path, true)?,
            };
            let out = f(&conn);
            inner.readers.lock().unwrap().push(conn);
            out
        })
        .await?
    }

    /// Events matching `filter` with seq ≥ `from`, oldest first, at most `limit`.
    pub async fn events(
        &self,
        filter: atelier_core::stream::StreamFilter,
        from: Seq,
        limit: usize,
    ) -> Result<Vec<Arc<Event>>> {
        self.read(move |c| read::events(c, &filter, from, limit)).await
    }
}

fn open_conn(path: &Path, read_only: bool) -> Result<Connection> {
    use rusqlite::OpenFlags;
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags).with_context(|| format!("opening {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -16000;",
    )?;
    Ok(conn)
}

fn schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
            seq    INTEGER PRIMARY KEY,
            stream TEXT    NOT NULL,
            ts     INTEGER NOT NULL,
            kind   TEXT    NOT NULL,
            actor  TEXT    NOT NULL,
            cause  INTEGER,
            body   TEXT    NOT NULL
         ) STRICT;
         CREATE INDEX IF NOT EXISTS events_by_stream ON events(stream, seq);
         CREATE TABLE IF NOT EXISTS projections (
            name    TEXT    PRIMARY KEY,
            version INTEGER NOT NULL,
            seq     INTEGER NOT NULL
         ) STRICT;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
