//! One WebSocket per client. See `atelier-proto` for the protocol.
//!
//! Back-pressure policy: every connection has a bounded outbox. If a client
//! can't keep up and the outbox fills, the connection is closed; the client
//! reconnects and resumes from its last seq. No unbounded buffering, no
//! per-client queues growing in the server.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::Result;
use atelier_core::Seq;
use atelier_core::stream::StreamFilter;
use atelier_proto::{ClientMsg, PROTOCOL, RpcId, ServerMsg, SubId, ViewName, WatchId};
use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures::{SinkExt, StreamExt};
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::{App, rpc, views};

const OUTBOX: usize = 1024;
const PAGE: usize = 500;

#[derive(Clone)]
struct Outbox {
    tx: mpsc::Sender<Utf8Bytes>,
    overflow: Arc<Notify>,
}

impl Outbox {
    /// Queue a frame. Returns false once the connection is closing.
    fn send(&self, msg: &ServerMsg) -> bool {
        let text = serde_json::to_string(msg).expect("server messages serialize");
        match self.tx.try_send(text.into()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflow.notify_one();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

/// `peer` is set for connections on the tailnet listener whose peer may be
/// trusted without a token (see [`crate::tailnet`]).
pub async fn connection(app: App, socket: WebSocket, peer: Option<IpAddr>) {
    let (mut sink, mut incoming) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Utf8Bytes>(OUTBOX);
    let out = Outbox { tx, overflow: Arc::new(Notify::new()) };

    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // The first frame must be a valid hello.
    match incoming.next().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(&t) {
            Ok(ClientMsg::Hello { protocol, client, token }) => {
                if protocol != PROTOCOL {
                    out.send(&ServerMsg::Fatal {
                        error: format!("protocol {protocol} unsupported; server speaks {PROTOCOL}"),
                    });
                    return finish(out, writer).await;
                }
                let ok = app.config.token_ok(token.as_deref())
                    || match peer {
                        Some(ip) => app.peers.trusted(ip).await,
                        None => false,
                    };
                if !ok {
                    let error = match (peer, token) {
                        (Some(_), None) => {
                            "unauthorized: this device isn't signed in to the server's Tailscale account; a token is needed"
                        }
                        _ => "unauthorized",
                    };
                    out.send(&ServerMsg::Fatal { error: error.into() });
                    return finish(out, writer).await;
                }
                tracing::info!("client connected: {client}");
                out.send(&ServerMsg::Welcome {
                    protocol: PROTOCOL,
                    server: concat!("atelierd/", env!("CARGO_PKG_VERSION")).into(),
                    head: app.store.head(),
                });
            }
            _ => {
                out.send(&ServerMsg::Fatal { error: "expected hello".into() });
                return finish(out, writer).await;
            }
        },
        _ => return finish(out, writer).await,
    }

    let mut subs: HashMap<SubId, JoinHandle<()>> = HashMap::new();
    let mut watches: HashMap<WatchId, JoinHandle<()>> = HashMap::new();
    let overflow = out.overflow.clone();
    let mut stopping = app.stopping.subscribe();

    loop {
        let frame = tokio::select! {
            f = incoming.next() => f,
            _ = overflow.notified() => {
                tracing::warn!("client too slow; closing so it can resume");
                break;
            }
            _ = stopping.wait_for(|s| *s) => break,
        };
        let text = match frame {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            Some(Ok(_)) => continue, // ping/pong handled by axum; binary unused
        };
        let msg = match serde_json::from_str::<ClientMsg>(&text) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("bad client frame: {e}");
                // An RPC we can't read (typically a newer client's method) must still be answered, or its caller
                // waits forever.
                if let Some(id) = rpc_id(&text) {
                    let error = format!(
                        "the server can't handle this request ({e}); it may be older than this app, so update or \
                         restart it"
                    );
                    out.send(&ServerMsg::Error { id, error });
                }
                continue;
            }
        };
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::Sub { id, filter, from } => {
                let task = tokio::spawn(run_sub(app.clone(), out.clone(), id, filter, from));
                if let Some(old) = subs.insert(id, task) {
                    old.abort();
                }
            }
            ClientMsg::Unsub { id } => {
                if let Some(t) = subs.remove(&id) {
                    t.abort();
                }
            }
            ClientMsg::Watch { id, view, scope } => {
                let task = tokio::spawn(run_watch(app.clone(), out.clone(), id, view, scope));
                if let Some(old) = watches.insert(id, task) {
                    old.abort();
                }
            }
            ClientMsg::Unwatch { id } => {
                if let Some(t) = watches.remove(&id) {
                    t.abort();
                }
            }
            ClientMsg::Rpc { id, call } => {
                let (app, out) = (app.clone(), out.clone());
                tokio::spawn(async move {
                    let reply = match rpc::handle(&app, atelier_core::Actor::User, None, call).await {
                        Ok(ok) => ServerMsg::Result { id, ok },
                        Err(e) => ServerMsg::Error { id, error: format!("{e:#}") },
                    };
                    out.send(&reply);
                });
            }
        }
    }

    for t in subs.into_values().chain(watches.into_values()) {
        t.abort();
    }
    finish(out, writer).await;
}

/// The id of a frame that is an RPC, even one whose call doesn't parse.
fn rpc_id(text: &str) -> Option<RpcId> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v["t"] != "rpc" {
        return None;
    }
    v["id"].as_u64()?.try_into().ok()
}

async fn finish(out: Outbox, writer: JoinHandle<()>) {
    drop(out);
    let _ = writer.await;
}

/// History from `from`, then live. Subscribing to the feed *before* reading
/// history guarantees no gap; seqs already sent are skipped.
async fn run_sub(app: App, out: Outbox, id: SubId, filter: StreamFilter, from: Seq) {
    let mut feed = app.store.subscribe();
    let mut eph = app.supervisor.ephemeral();
    let mut next = from.max(1);

    if catch_up(&app, &out, id, &filter, &mut next).await.is_err() {
        return;
    }
    if !out.send(&ServerMsg::Live { sub: id }) {
        return;
    }

    loop {
        tokio::select! {
            r = feed.recv() => match r {
                Ok(commit) => {
                    let events: Vec<_> = commit.events.iter()
                        .filter(|e| e.seq >= next && filter.matches(&e.stream))
                        .cloned()
                        .collect();
                    if let Some(last) = events.last() {
                        next = last.seq + 1;
                        if !out.send(&ServerMsg::Events { sub: id, events }) { return; }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if catch_up(&app, &out, id, &filter, &mut next).await.is_err() { return; }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            r = eph.recv() => match r {
                Ok(e) if filter.matches(&e.stream) => {
                    let msg = ServerMsg::Eph { sub: id, stream: e.stream.clone(), kind: e.kind.clone(), body: e.body.clone() };
                    if !out.send(&msg) { return; }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {} // ephemera may be dropped
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

async fn catch_up(app: &App, out: &Outbox, id: SubId, filter: &StreamFilter, next: &mut Seq) -> Result<()> {
    loop {
        let events = app.store.events(filter.clone(), *next, PAGE).await?;
        let Some(last) = events.last().map(|e| e.seq) else {
            return Ok(());
        };
        let full = events.len() == PAGE;
        *next = last + 1;
        if !out.send(&ServerMsg::Events { sub: id, events }) {
            anyhow::bail!("closing");
        }
        if !full {
            return Ok(());
        }
    }
}

/// Snapshot, then re-read and push rows as projections report them changed.
/// Re-reading the latest row makes duplicate or reordered changes harmless.
async fn run_watch(app: App, out: Outbox, id: WatchId, view: ViewName, scope: Option<String>) {
    let mut feed = app.store.subscribe();
    if !send_snapshot(&app, &out, id, view, scope.clone()).await {
        return;
    }
    loop {
        match feed.recv().await {
            Ok(commit) => {
                let keys: BTreeSet<String> = commit
                    .changes
                    .iter()
                    .filter(|c| c.view == view.as_str() && (scope.is_none() || c.scope == scope))
                    .map(|c| c.key.clone())
                    .collect();
                if keys.is_empty() {
                    continue;
                }
                let rows = app
                    .store
                    .read(move |c| {
                        keys.into_iter().map(|k| Ok((views::row(c, view, &k)?, k))).collect::<Result<Vec<_>>>()
                    })
                    .await;
                let Ok(rows) = rows else { continue };
                let (mut upsert, mut remove) = (Vec::new(), Vec::new());
                for (row, key) in rows {
                    match row {
                        Some(r) => upsert.push(r),
                        None => remove.push(key),
                    }
                }
                if !out.send(&ServerMsg::Patch { watch: id, upsert, remove }) {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if !send_snapshot(&app, &out, id, view, scope.clone()).await {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn send_snapshot(app: &App, out: &Outbox, id: WatchId, view: ViewName, scope: Option<String>) -> bool {
    match app.store.read(move |c| views::snapshot(c, view, scope.as_deref())).await {
        Ok(rows) => out.send(&ServerMsg::Snapshot { watch: id, rows }),
        Err(e) => {
            tracing::error!("snapshot of {view:?} failed: {e:#}");
            false
        }
    }
}
