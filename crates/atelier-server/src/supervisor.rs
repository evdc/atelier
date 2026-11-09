//! Owns the agent processes.
//!
//! One process per *active* session. Each has a pump task that appends the
//! driver's output to the log (or relays it, if ephemeral). The supervisor is
//! the only writer of a session's agent-side events, and the single authority
//! on which permission requests are still answerable.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use atelier_core::domain::*;
use atelier_core::{Actor, NewEvent, SessionId};
use atelier_driver::{AgentCmd, AgentDriver, DriverEvent, Resume, StartSpec};
use atelier_proto::SessionRow;
use atelier_store::Store;
use serde_json::value::RawValue;
use tokio::sync::{broadcast, mpsc};

use crate::{autotitle, views};

/// A live-only event (token deltas, status pings). Never persisted.
#[derive(Debug)]
pub struct Ephemeral {
    pub stream: String,
    pub kind: String,
    pub body: Arc<RawValue>,
}

#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    driver: Arc<dyn AgentDriver>,
    eph: broadcast::Sender<Arc<Ephemeral>>,
    actors: Mutex<HashMap<SessionId, Proc>>,
    /// (session, request id) of requests that can still be answered.
    pending: Mutex<HashSet<(SessionId, String)>>,
    generation: AtomicU64,
    /// The `claude` binary for one-shot titles; `None` turns them off.
    title_bin: Option<PathBuf>,
}

struct Proc {
    generation: u64,
    cmds: mpsc::UnboundedSender<AgentCmd>,
}

impl Supervisor {
    pub fn new(store: Store, driver: Arc<dyn AgentDriver>, title_bin: Option<PathBuf>) -> Self {
        let (eph, _) = broadcast::channel(4096);
        Self {
            inner: Arc::new(Inner {
                store,
                driver,
                eph,
                actors: Mutex::default(),
                pending: Mutex::default(),
                generation: AtomicU64::new(0),
                title_bin,
            }),
        }
    }

    pub fn ephemeral(&self) -> broadcast::Receiver<Arc<Ephemeral>> {
        self.inner.eph.subscribe()
    }

    /// Sessions the log believes are alive can't be after a restart: the
    /// processes died with the previous server. Record that.
    pub async fn recover(&self) -> Result<()> {
        let stale = self.inner.store.read(views::sessions::alive).await?;
        let events: Vec<_> = stale
            .iter()
            .map(|s| {
                let body = ProcessExited { reason: ExitReason::ServerRestart, code: None, stderr_tail: String::new() };
                NewEvent::new(s.stream.clone(), Actor::Atelier, &body)
            })
            .collect();
        if !events.is_empty() {
            tracing::info!("recovered {} sessions orphaned by a restart", events.len());
            self.inner.store.append(events).await?;
        }
        Ok(())
    }

    /// Append the user's message and deliver it, starting the agent if needed.
    pub async fn send(&self, s: &SessionRow, text: String) -> Result<()> {
        if s.archived {
            bail!("session is archived");
        }
        self.append(s, Actor::User, &UserMessage { text: text.clone() }).await?;
        let cmds = self.ensure_running(s).await?;
        cmds.send(AgentCmd::Send(text)).map_err(|_| anyhow!("agent exited before receiving the message"))
    }

    pub async fn interrupt(&self, s: &SessionRow) -> Result<()> {
        let cmds = self.running(&s.id).context("session is not running")?;
        self.append(s, Actor::User, &UserInterrupt {}).await?;
        let _ = cmds.send(AgentCmd::Interrupt);
        Ok(())
    }

    pub async fn answer(&self, s: &SessionRow, request_id: String, decision: PermissionDecision) -> Result<()> {
        // Claim it first so two devices answering at once can't both win.
        if !self.inner.pending.lock().unwrap().remove(&(s.id.clone(), request_id.clone())) {
            bail!("that request was already answered or has expired");
        }
        let cmds = self.running(&s.id).context("session is not running")?;
        let body = PermissionAnswered { request_id: request_id.clone(), decision: decision.clone() };
        self.append(s, Actor::User, &body).await?;
        let _ = cmds.send(AgentCmd::Answer { request_id, decision });
        Ok(())
    }

    pub fn stop(&self, id: &SessionId) {
        if let Some(cmds) = self.running(id) {
            let _ = cmds.send(AgentCmd::Shutdown);
        }
    }

    /// Stop everything and wait (bounded) for the exits to be recorded.
    pub async fn shutdown(&self) {
        let ids: Vec<_> = self.inner.actors.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.stop(id);
        }
        for _ in 0..40 {
            if self.inner.actors.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn running(&self, id: &SessionId) -> Option<mpsc::UnboundedSender<AgentCmd>> {
        let actors = self.inner.actors.lock().unwrap();
        actors.get(id).filter(|a| !a.cmds.is_closed()).map(|a| a.cmds.clone())
    }

    async fn ensure_running(&self, s: &SessionRow) -> Result<mpsc::UnboundedSender<AgentCmd>> {
        if let Some(cmds) = self.running(&s.id) {
            return Ok(cmds);
        }
        let native = {
            let id = s.id.clone();
            self.inner.store.read(move |c| views::sessions::native_id(c, id.as_str())).await?
        };
        let resume = match native {
            Some(native_id) => Resume::Continue { native_id },
            None => Resume::New,
        };
        let spec =
            StartSpec { session: s.id.clone(), cwd: PathBuf::from(&s.workspace.path), resume, model: s.model.clone() };

        let handle = match self.inner.driver.start(spec) {
            Ok(h) => h,
            Err(e) => {
                let body = ProcessExited { reason: ExitReason::SpawnFailed, code: None, stderr_tail: format!("{e:#}") };
                self.append(s, Actor::Atelier, &body).await?;
                return Err(e);
            }
        };
        self.append(s, Actor::Atelier, &ProcessStarted { pid: handle.pid, native_id: handle.native_id.clone() })
            .await?;

        let generation = self.inner.generation.fetch_add(1, Ordering::Relaxed);
        self.inner.actors.lock().unwrap().insert(s.id.clone(), Proc { generation, cmds: handle.cmds.clone() });
        tokio::spawn(self.clone().pump(s.id.clone(), s.stream.clone(), generation, handle.events));
        Ok(handle.cmds)
    }

    /// Move one process's output into the log until it exits.
    async fn pump(self, id: SessionId, stream: String, generation: u64, mut events: mpsc::Receiver<DriverEvent>) {
        let actor = Actor::Agent(id.clone());
        while let Some(ev) = events.recv().await {
            let mut turn_ok = false;
            let new = match ev {
                DriverEvent::Raw { kind, body } => NewEvent::raw(&stream, kind, actor.clone(), body),
                DriverEvent::Ephemeral { kind, body } => {
                    let _ =
                        self.inner.eph.send(Arc::new(Ephemeral { stream: stream.clone(), kind, body: body.into() }));
                    continue;
                }
                DriverEvent::Bound { native_id } => NewEvent::new(&stream, actor.clone(), &AgentBound { native_id }),
                DriverEvent::TurnEnded(t) => {
                    turn_ok = !t.is_error;
                    NewEvent::new(&stream, actor.clone(), &t)
                }
                DriverEvent::Permission(p) => {
                    self.inner.pending.lock().unwrap().insert((id.clone(), p.request_id.clone()));
                    NewEvent::new(&stream, actor.clone(), &p)
                }
                DriverEvent::Exited(x) => {
                    {
                        let mut actors = self.inner.actors.lock().unwrap();
                        if actors.get(&id).is_some_and(|a| a.generation == generation) {
                            actors.remove(&id);
                        }
                    }
                    self.inner.pending.lock().unwrap().retain(|(s, _)| s != &id);
                    NewEvent::new(&stream, Actor::Atelier, &x)
                }
            };
            if let Err(e) = self.inner.store.append_one(new).await {
                tracing::error!(session = %id, "dropping agent event: {e:#}");
            }
            if turn_ok {
                self.spawn_auto_title(&id, &stream);
            }
        }
    }

    /// Titles the session in the background, so the pump never waits on the titler.
    fn spawn_auto_title(&self, id: &SessionId, stream: &str) {
        let Some(bin) = self.inner.title_bin.clone() else { return };
        let this = self.clone();
        let (id, stream) = (id.clone(), stream.to_owned());
        tokio::spawn(async move {
            if let Err(e) = this.auto_title(&id, &stream, &bin).await {
                tracing::warn!(session = %id, "auto-title: {e:#}");
            }
        });
    }

    async fn auto_title(&self, id: &SessionId, stream: &str, bin: &Path) -> Result<()> {
        let (sid, st) = (id.to_string(), stream.to_owned());
        let Some(ex) = self.inner.store.read(move |c| autotitle::exchange(c, &sid, &st)).await? else {
            return Ok(());
        };
        let Some(title) = autotitle::ask(bin, &ex).await? else { return Ok(()) };
        // The person may have renamed the session while the titler was thinking.
        let st = stream.to_owned();
        if self.inner.store.read(move |c| autotitle::renamed(c, &st)).await? {
            return Ok(());
        }
        self.inner.store.append_one(NewEvent::new(stream, Actor::Atelier, &SessionRenamed { title })).await?;
        Ok(())
    }

    async fn append<B: atelier_core::EventBody>(&self, s: &SessionRow, actor: Actor, body: &B) -> Result<()> {
        self.inner.store.append_one(NewEvent::new(s.stream.clone(), actor, body)).await?;
        Ok(())
    }
}
