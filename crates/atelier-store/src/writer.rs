use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use atelier_core::{Event, NewEvent, Seq};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::{broadcast, oneshot};

use crate::{Change, Commit, Projection, read};

pub(crate) enum Cmd {
    Append { events: Vec<NewEvent>, reply: oneshot::Sender<Result<Vec<Arc<Event>>>> },
}

/// Most commands folded into one transaction.
const MAX_GROUP: usize = 256;
/// Events per transaction when rebuilding a projection.
const REBUILD_BATCH: usize = 5000;

pub(crate) fn spawn(
    mut conn: Connection,
    mut projections: Vec<Box<dyn Projection>>,
    feed: broadcast::Sender<Arc<Commit>>,
    head: Arc<AtomicI64>,
) -> Result<Sender<Cmd>> {
    catch_up(&mut conn, &mut projections)?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("store-writer".into()).spawn(move || run(conn, projections, rx, feed, head))?;
    Ok(tx)
}

fn run(
    mut conn: Connection,
    mut projections: Vec<Box<dyn Projection>>,
    rx: Receiver<Cmd>,
    feed: broadcast::Sender<Arc<Commit>>,
    head: Arc<AtomicI64>,
) {
    while let Ok(first) = rx.recv() {
        // Group commit: take whatever else is already queued.
        let mut cmds = vec![first];
        while cmds.len() < MAX_GROUP {
            match rx.try_recv() {
                Ok(c) => cmds.push(c),
                Err(_) => break,
            }
        }
        commit_group(&mut conn, &mut projections, cmds, &feed, &head);
    }
    tracing::debug!("store writer stopped");
}

fn commit_group(
    conn: &mut Connection,
    projections: &mut [Box<dyn Projection>],
    cmds: Vec<Cmd>,
    feed: &broadcast::Sender<Arc<Commit>>,
    head: &AtomicI64,
) {
    let now = unix_ms();
    let mut tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => {
            let msg = format!("begin transaction: {e}");
            for Cmd::Append { reply, .. } in cmds {
                let _ = reply.send(Err(anyhow!(msg.clone())));
            }
            return;
        }
    };

    let mut replies = Vec::with_capacity(cmds.len());
    let mut committed: Vec<Arc<Event>> = Vec::new();
    let mut changes: Vec<Change> = Vec::new();

    for Cmd::Append { events, reply } in cmds {
        // Each command is atomic on its own; one failing doesn't sink the group.
        let result = (|| -> Result<(Vec<Arc<Event>>, Vec<Change>)> {
            let sp = tx.savepoint()?;
            let mut out = Vec::new();
            let mut evs = Vec::with_capacity(events.len());
            for ne in events {
                let ev = insert(&sp, ne, now)?;
                for p in projections.iter_mut() {
                    p.apply(&sp, &ev, &mut out)
                        .map_err(|e| anyhow!("projection {} failed on {} @{}: {e:#}", p.name(), ev.kind, ev.seq))?;
                }
                evs.push(Arc::new(ev));
            }
            sp.commit()?;
            Ok((evs, out))
        })();
        match result {
            Ok((evs, out)) => {
                committed.extend(evs.iter().cloned());
                changes.extend(out);
                replies.push((reply, Ok(evs)));
            }
            Err(e) => {
                tracing::error!("append rejected: {e:#}");
                replies.push((reply, Err(e)));
            }
        }
    }

    let last = committed.last().map(|e| e.seq);
    let finish = (|| -> Result<()> {
        if let Some(seq) = last {
            set_projection_seqs(&tx, projections, seq)?;
        }
        tx.commit()?;
        Ok(())
    })();

    match finish {
        Ok(()) => {
            if let Some(seq) = last {
                head.store(seq, Ordering::Release);
            }
            for (reply, res) in replies {
                let _ = reply.send(res);
            }
            if !committed.is_empty() {
                // No receivers is fine.
                let _ = feed.send(Arc::new(Commit { events: committed, changes }));
            }
        }
        Err(e) => {
            let msg = format!("commit failed: {e:#}");
            tracing::error!("{msg}");
            for (reply, _) in replies {
                let _ = reply.send(Err(anyhow!(msg.clone())));
            }
        }
    }
}

fn insert(c: &Connection, ne: NewEvent, ts: i64) -> Result<Event> {
    c.prepare_cached(
        "INSERT INTO events (stream, ts, kind, actor, cause, body, project) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![ne.stream, ts, ne.kind, ne.actor, ne.cause, ne.body.get(), crate::project_of(&ne.stream)])?;
    Ok(Event {
        seq: c.last_insert_rowid(),
        stream: ne.stream,
        ts,
        kind: ne.kind,
        actor: ne.actor,
        cause: ne.cause,
        body: ne.body,
    })
}

fn set_projection_seqs(c: &Connection, projections: &[Box<dyn Projection>], seq: Seq) -> Result<()> {
    let mut q = c.prepare_cached("UPDATE projections SET seq = ?2 WHERE name = ?1")?;
    for p in projections {
        q.execute(params![p.name(), seq])?;
    }
    Ok(())
}

/// Bring every projection up to the head of the log, rebuilding any whose
/// version changed.
fn catch_up(conn: &mut Connection, projections: &mut [Box<dyn Projection>]) -> Result<()> {
    for p in projections.iter_mut() {
        let meta: Option<(u32, Seq)> = conn
            .query_row("SELECT version, seq FROM projections WHERE name = ?1", [p.name()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let tx = conn.transaction()?;
        let mut from = match meta {
            Some((v, seq)) if v == p.version() => {
                p.migrate(&tx)?;
                seq + 1
            }
            other => {
                if let Some((v, _)) = other {
                    tracing::info!("projection {}: v{v} -> v{}, rebuilding", p.name(), p.version());
                }
                p.reset(&tx)?;
                p.migrate(&tx)?;
                tx.execute(
                    "INSERT INTO projections (name, version, seq) VALUES (?1, ?2, 0)
                     ON CONFLICT(name) DO UPDATE SET version = excluded.version, seq = 0",
                    params![p.name(), p.version()],
                )?;
                1
            }
        };
        tx.commit()?;

        let mut folded = 0usize;
        loop {
            let batch = read::all_events(conn, from, REBUILD_BATCH)?;
            let Some(last) = batch.last().map(|e| e.seq) else {
                break;
            };
            let tx = conn.transaction()?;
            let mut discard = Vec::new();
            for ev in &batch {
                p.apply(&tx, ev, &mut discard)?;
                discard.clear();
            }
            tx.execute("UPDATE projections SET seq = ?2 WHERE name = ?1", params![p.name(), last])?;
            tx.commit()?;
            folded += batch.len();
            from = last + 1;
        }
        if folded > 0 {
            tracing::info!("projection {}: folded {folded} events", p.name());
        }
    }
    Ok(())
}

fn unix_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
