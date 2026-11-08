//! Claude Code, driven headless over `stream-json` on stdin/stdout.
//!
//! Protocol notes (verified against CC 2.1.x, see `spikes/claude-stream-json`):
//! - One long-lived process per session; each stdin `{"type":"user"}` line
//!   starts a turn, which ends with a `{"type":"result"}` line.
//! - `--permission-prompt-tool stdio` routes permission prompts (and
//!   AskUserQuestion) to us as `control_request{subtype:"can_use_tool"}`; we
//!   answer with `control_response`. Without it, prompts are auto-denied.
//! - `control_request{subtype:"interrupt"}` ends the current turn.
//! - `--resume <id> --fork-session [--resume-session-at <msg uuid>]` forks.
//!   (`--resume-session-at` is undocumented; keep its use here.)

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_core::domain::{AgentKind, ExitReason, PermissionDecision, PermissionRequested, ProcessExited, TurnEnded};
use serde_json::{Value, json, value::RawValue};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::{AgentCmd, AgentDriver, AgentHandle, DriverEvent, Resume, StartSpec};

/// Event kinds are namespaced `cc.<type>`.
pub const NS: &str = "cc";

/// How long a stopping process gets before it is killed.
const GRACE: Duration = Duration::from_secs(3);
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone)]
pub struct ClaudeCode {
    /// The `claude` executable.
    pub bin: PathBuf,
    /// `--permission-mode`. `auto` lets CC decide and ask only when it wants a person.
    pub permission_mode: String,
    pub extra_args: Vec<String>,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self { bin: "claude".into(), permission_mode: "auto".into(), extra_args: Vec::new() }
    }
}

impl ClaudeCode {
    fn command(&self, spec: &StartSpec) -> (Command, Option<String>) {
        let mut cmd = Command::new(&self.bin);
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            &self.permission_mode,
        ]);
        let native_id = match &spec.resume {
            Resume::New => {
                cmd.args(["--session-id", spec.session.as_str()]);
                Some(spec.session.to_string())
            }
            Resume::Continue { native_id } => {
                cmd.args(["--resume", native_id]);
                Some(native_id.clone())
            }
            Resume::Fork { native_id, at } => {
                cmd.args(["--resume", native_id, "--fork-session"]);
                if let Some(at) = at {
                    cmd.args(["--resume-session-at", at]);
                }
                None // learned from the init message
            }
        };
        if let Some(model) = &spec.model {
            cmd.args(["--model", model]);
        }
        cmd.args(&self.extra_args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        (cmd, native_id)
    }
}

impl AgentDriver for ClaudeCode {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn start(&self, spec: StartSpec) -> Result<AgentHandle> {
        let (mut cmd, native_id) = self.command(&spec);
        let mut child = cmd.spawn().with_context(|| format!("spawning {}", self.bin.display()))?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let (ev_tx, ev_rx) = mpsc::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (line_tx, line_rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let stopping = Arc::new(AtomicBool::new(false));
        // Inputs of pending permission requests: CC wants `updatedInput` on allow.
        let pending: Pending = Arc::default();
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();

        tokio::spawn(write_stdin(stdin, cmd_rx, line_rx, pending.clone(), stopping.clone(), kill_tx));

        let tail_w = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "claude.stderr", "{line}");
                let mut t = tail_w.lock().unwrap();
                if t.len() == STDERR_TAIL_LINES {
                    t.pop_front();
                }
                t.push_back(line);
            }
        });

        let reader = {
            let ev_tx = ev_tx.clone();
            let known = native_id.clone();
            tokio::spawn(read_stdout(stdout, ev_tx, line_tx, pending, known))
        };

        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx => {
                    tokio::select! {
                        s = child.wait() => s,
                        _ = tokio::time::sleep(GRACE) => {
                            let _ = child.start_kill();
                            child.wait().await
                        }
                    }
                }
            };
            // Deliver everything it printed before reporting the exit.
            let _ = reader.await;
            let reason = if stopping.load(Ordering::Acquire) { ExitReason::Stopped } else { ExitReason::Exited };
            let stderr_tail = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
            let code = status.ok().and_then(|s| s.code());
            let _ = ev_tx.send(DriverEvent::Exited(ProcessExited { reason, code, stderr_tail })).await;
        });

        Ok(AgentHandle { pid, native_id, cmds: cmd_tx, events: ev_rx })
    }
}

type Pending = Arc<Mutex<HashMap<String, Value>>>;

async fn write_stdin(
    mut stdin: ChildStdin,
    mut cmds: mpsc::UnboundedReceiver<AgentCmd>,
    mut lines: mpsc::UnboundedReceiver<String>,
    pending: Pending,
    stopping: Arc<AtomicBool>,
    kill: oneshot::Sender<()>,
) {
    loop {
        let line = tokio::select! {
            Some(line) = lines.recv() => line,
            cmd = cmds.recv() => match cmd {
                Some(AgentCmd::Send(text)) => json!({
                    "type": "user",
                    "message": { "role": "user", "content": text },
                })
                .to_string(),
                Some(AgentCmd::Interrupt) => json!({
                    "type": "control_request",
                    "request_id": uuid::Uuid::now_v7().to_string(),
                    "request": { "subtype": "interrupt" },
                })
                .to_string(),
                Some(AgentCmd::Answer { request_id, decision }) => {
                    let original = pending.lock().unwrap().remove(&request_id);
                    let response = match decision {
                        PermissionDecision::Allow { updated_input } => json!({
                            "behavior": "allow",
                            "updatedInput": updated_input.or(original).unwrap_or(json!({})),
                        }),
                        PermissionDecision::Deny { message } => json!({
                            "behavior": "deny",
                            "message": message,
                        }),
                    };
                    json!({
                        "type": "control_response",
                        "response": { "subtype": "success", "request_id": request_id, "response": response },
                    })
                    .to_string()
                }
                Some(AgentCmd::Shutdown) | None => break,
            },
        };
        if write_line(&mut stdin, &line).await.is_err() {
            break; // process is gone; the exit is reported by the wait task
        }
    }
    // Closing stdin asks CC to finish; the wait task kills it after GRACE.
    stopping.store(true, Ordering::Release);
    drop(stdin);
    let _ = kill.send(());
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    events: mpsc::Sender<DriverEvent>,
    replies: mpsc::UnboundedSender<String>,
    pending: Pending,
    mut known_id: Option<String>,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("claude stdout: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("claude: non-JSON line: {}", truncate(&line, 200));
                continue;
            }
        };
        for ev in classify(msg, &mut known_id, &pending, &replies) {
            if events.send(ev).await.is_err() {
                return; // nobody is listening any more
            }
        }
    }
}

/// Map one stdout message to driver events. Pure apart from bookkeeping, so
/// it is unit-testable without a process.
fn classify(
    mut msg: Value,
    known_id: &mut Option<String>,
    pending: &Pending,
    replies: &mpsc::UnboundedSender<String>,
) -> Vec<DriverEvent> {
    let ty = msg.get("type").and_then(Value::as_str).unwrap_or("unknown").to_owned();
    let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut out = Vec::with_capacity(1);

    match ty.as_str() {
        "stream_event" => {
            let event = msg.get_mut("event").map(Value::take).unwrap_or(Value::Null);
            out.push(DriverEvent::Ephemeral { kind: format!("{NS}.stream_event"), body: raw(&event) });
        }
        "system" if matches!(subtype.as_str(), "status" | "thinking_tokens") => {
            out.push(DriverEvent::Ephemeral { kind: format!("{NS}.system"), body: raw(&msg) });
        }
        "system" if subtype == "init" => {
            if let Some(id) = msg.get("session_id").and_then(Value::as_str)
                && known_id.as_deref() != Some(id)
            {
                *known_id = Some(id.to_owned());
                out.push(DriverEvent::Bound { native_id: id.to_owned() });
            }
            out.push(DriverEvent::Raw { kind: format!("{NS}.system"), body: raw(&msg) });
        }
        "assistant" => {
            strip_signatures(&mut msg);
            out.push(DriverEvent::Raw { kind: format!("{NS}.assistant"), body: raw(&msg) });
        }
        "result" => {
            let is_error = msg.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype != "success";
            out.push(DriverEvent::TurnEnded(TurnEnded {
                is_error,
                cost_usd: msg.get("total_cost_usd").and_then(Value::as_f64),
                duration_ms: msg.get("duration_ms").and_then(Value::as_u64),
                raw: msg,
            }));
        }
        "control_request" => {
            let request_id = msg.get("request_id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let request = msg.get_mut("request").map(Value::take).unwrap_or(Value::Null);
            match request.get("subtype").and_then(Value::as_str) {
                Some("can_use_tool") => {
                    let tool = request.get("tool_name").and_then(Value::as_str).unwrap_or("?").to_owned();
                    let input = request.get("input").cloned().unwrap_or(Value::Null);
                    pending.lock().unwrap().insert(request_id.clone(), input.clone());
                    out.push(DriverEvent::Permission(PermissionRequested { request_id, tool, input, raw: request }));
                }
                other => {
                    // Unhandled host callbacks must still be answered, or CC waits forever.
                    tracing::warn!("claude: unhandled control_request {other:?}");
                    let _ = replies.send(
                        json!({
                            "type": "control_response",
                            "response": { "subtype": "error", "request_id": request_id,
                                          "error": "not supported by Atelier" },
                        })
                        .to_string(),
                    );
                }
            }
        }
        "control_response" | "keep_alive" => {} // acks of our own requests
        _ => out.push(DriverEvent::Raw { kind: format!("{NS}.{ty}"), body: raw(&msg) }),
    }
    out
}

/// Thinking-block signatures are large opaque blobs only the API needs
/// (CC keeps its own transcript for resuming). Don't store or ship them.
fn strip_signatures(msg: &mut Value) {
    if let Some(blocks) = msg.pointer_mut("/message/content").and_then(Value::as_array_mut) {
        for b in blocks {
            if let Some(o) = b.as_object_mut() {
                o.remove("signature");
                if o.get("type").and_then(Value::as_str) == Some("redacted_thinking") {
                    o.remove("data");
                }
            }
        }
    }
}

fn raw(v: &Value) -> Box<RawValue> {
    serde_json::value::to_raw_value(v).expect("Value always serializes")
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests;
