//! Owns the agent processes.
//!
//! One process per *active* session. Each has a pump task that appends the
//! driver's output to the log (or relays it, if ephemeral). The supervisor is
//! the only writer of a session's agent-side events, and the single authority
//! on which permission requests are still answerable.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use atelier_core::domain::*;
use atelier_core::{Actor, NewEvent, SessionId};
use atelier_driver::{AgentCmd, AgentDriver, DriverEvent, Resume, StartSpec};
use atelier_proto::{SessionRow, SessionStatus};
use atelier_store::Store;
use serde_json::value::RawValue;
use tokio::sync::{broadcast, mpsc};

use crate::views;

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
}

/// How often the reaper looks for idle processes.
const REAP_EVERY: Duration = Duration::from_secs(30);
/// `process.exited.detail` for a process the reaper stopped.
pub const IDLE_DETAIL: &str = "idle";

struct Proc {
    generation: u64,
    cmds: mpsc::UnboundedSender<AgentCmd>,
    /// Last driver event or command. Runtime state for the reaper, not log state.
    last_active: Instant,
    /// Set by the reaper. The process stays listed until its exit is logged.
    reaping: Arc<AtomicBool>,
}

impl Supervisor {
    pub fn new(store: Store, driver: Arc<dyn AgentDriver>) -> Self {
        let (eph, _) = broadcast::channel(4096);
        Self {
            inner: Arc::new(Inner {
                store,
                driver,
                eph,
                actors: Mutex::default(),
                pending: Mutex::default(),
                generation: AtomicU64::new(0),
            }),
        }
    }

    pub fn ephemeral(&self) -> broadcast::Receiver<Arc<Ephemeral>> {
        self.inner.eph.subscribe()
    }

    /// Stop processes whose session has been idle for `idle`. They come back on the next message.
    pub fn spawn_reaper(&self, idle: Duration) {
        // Looking less often than the timeout would make short timeouts (tests) overshoot.
        let every = REAP_EVERY.min(idle).max(Duration::from_millis(100));
        let this = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                this.reap_idle(idle).await;
            }
        });
    }

    /// Stop processes that have been quiet for `idle` while their session is idle: no turn running, no
    /// permission request waiting. Quiet is re-checked under the lock, so a command that arrives while we
    /// look keeps its process.
    async fn reap_idle(&self, idle: Duration) {
        let quiet: Vec<SessionId> = {
            let actors = self.inner.actors.lock().unwrap();
            actors
                .iter()
                .filter(|(_, p)| !p.reaping.load(Ordering::Acquire) && p.last_active.elapsed() >= idle)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in quiet {
            let key = id.clone();
            let Ok(Some(row)) = self.inner.store.read(move |c| views::sessions::get(c, key.as_str())).await else {
                continue;
            };
            if !matches!(row.status, SessionStatus::Idle) || row.pending > 0 {
                continue;
            }
            let mut actors = self.inner.actors.lock().unwrap();
            let Some(p) = actors.get_mut(&id) else { continue };
            if p.reaping.load(Ordering::Acquire) || p.cmds.is_closed() || p.last_active.elapsed() < idle {
                continue;
            }
            tracing::info!(session = %id, "stopping idle agent process");
            p.reaping.store(true, Ordering::Release);
            let _ = p.cmds.send(AgentCmd::Shutdown);
        }
    }

    /// Sessions the log believes are alive can't be after a restart: the
    /// processes died with the previous server. Record that.
    pub async fn recover(&self) -> Result<()> {
        let stale = self.inner.store.read(views::sessions::alive).await?;
        let events: Vec<_> = stale
            .iter()
            .map(|s| {
                let body = ProcessExited {
                    reason: ExitReason::ServerRestart,
                    code: None,
                    stderr_tail: String::new(),
                    detail: None,
                };
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
        self.wait_stopped(&s.id).await?;
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

    /// The live process's command channel. Taking it counts as activity.
    fn running(&self, id: &SessionId) -> Option<mpsc::UnboundedSender<AgentCmd>> {
        let mut actors = self.inner.actors.lock().unwrap();
        let a = actors.get_mut(id)?;
        if a.cmds.is_closed() || a.reaping.load(Ordering::Acquire) {
            return None;
        }
        a.last_active = Instant::now();
        Some(a.cmds.clone())
    }

    /// A reaped process may still be exiting. Its exit must be logged before a replacement starts, or the
    /// replacement's events would be ordered after an exit that isn't its own.
    async fn wait_stopped(&self, id: &SessionId) -> Result<()> {
        for _ in 0..100 {
            let stopping = self.inner.actors.lock().unwrap().get(id).is_some_and(|p| p.reaping.load(Ordering::Acquire));
            if !stopping {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("the agent is still stopping; try again in a moment")
    }

    async fn ensure_running(&self, s: &SessionRow) -> Result<mpsc::UnboundedSender<AgentCmd>> {
        if let Some(cmds) = self.running(&s.id) {
            return Ok(cmds);
        }
        self.wait_stopped(&s.id).await?;
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
                let body = ProcessExited {
                    reason: ExitReason::SpawnFailed,
                    code: None,
                    stderr_tail: format!("{e:#}"),
                    detail: None,
                };
                self.append(s, Actor::Atelier, &body).await?;
                return Err(e);
            }
        };
        self.append(s, Actor::Atelier, &ProcessStarted { pid: handle.pid, native_id: handle.native_id.clone() })
            .await?;

        let generation = self.inner.generation.fetch_add(1, Ordering::Relaxed);
        let reaping = Arc::new(AtomicBool::new(false));
        self.inner.actors.lock().unwrap().insert(
            s.id.clone(),
            Proc { generation, cmds: handle.cmds.clone(), last_active: Instant::now(), reaping: reaping.clone() },
        );
        tokio::spawn(self.clone().pump(s.id.clone(), s.stream.clone(), generation, reaping, handle.events));
        Ok(handle.cmds)
    }

    /// Move one process's output into the log until it exits.
    async fn pump(
        self,
        id: SessionId,
        stream: String,
        generation: u64,
        reaping: Arc<AtomicBool>,
        mut events: mpsc::Receiver<DriverEvent>,
    ) {
        let actor = Actor::Agent(id.clone());
        while let Some(ev) = events.recv().await {
            self.touch(&id, generation);
            let new = match ev {
                DriverEvent::Raw { kind, body } => NewEvent::raw(&stream, kind, actor.clone(), body),
                DriverEvent::Ephemeral { kind, body } => {
                    let _ =
                        self.inner.eph.send(Arc::new(Ephemeral { stream: stream.clone(), kind, body: body.into() }));
                    continue;
                }
                DriverEvent::Bound { native_id } => NewEvent::new(&stream, actor.clone(), &AgentBound { native_id }),
                DriverEvent::TurnEnded(t) => NewEvent::new(&stream, actor.clone(), &t),
                DriverEvent::Permission(p) => {
                    self.inner.pending.lock().unwrap().insert((id.clone(), p.request_id.clone()));
                    NewEvent::new(&stream, actor.clone(), &p)
                }
                DriverEvent::Exited(mut x) => {
                    if reaping.load(Ordering::Acquire) {
                        x.detail = Some(IDLE_DETAIL.to_owned());
                    }
                    self.inner.pending.lock().unwrap().retain(|(s, _)| s != &id);
                    // Logged before the process is forgotten (see `wait_stopped`).
                    let new = NewEvent::new(&stream, Actor::Atelier, &x);
                    if let Err(e) = self.inner.store.append_one(new).await {
                        tracing::error!(session = %id, "dropping exit event: {e:#}");
                    }
                    break;
                }
            };
            if let Err(e) = self.inner.store.append_one(new).await {
                tracing::error!(session = %id, "dropping agent event: {e:#}");
            }
        }
        self.forget(&id, generation);
    }

    fn touch(&self, id: &SessionId, generation: u64) {
        if let Some(a) = self.inner.actors.lock().unwrap().get_mut(id).filter(|a| a.generation == generation) {
            a.last_active = Instant::now();
        }
    }

    fn forget(&self, id: &SessionId, generation: u64) {
        let mut actors = self.inner.actors.lock().unwrap();
        if actors.get(id).is_some_and(|a| a.generation == generation) {
            actors.remove(id);
        }
    }

    async fn append<B: atelier_core::EventBody>(&self, s: &SessionRow, actor: Actor, body: &B) -> Result<()> {
        self.inner.store.append_one(NewEvent::new(s.stream.clone(), actor, body)).await?;
        Ok(())
    }
}
