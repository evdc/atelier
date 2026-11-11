//! End to end over a real WebSocket, with a scripted stand-in for `claude`
//! (`atelier-driver/tests/fixtures/fake-claude.sh`).

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atelier_server::tailnet::PeerAuth;
use atelier_server::{App, Config};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../atelier-driver/tests/fixtures/fake-claude.sh");

struct Client {
    ws: Ws,
    next_id: u32,
    seen: Vec<Value>,
}

impl Client {
    async fn connect(addr: std::net::SocketAddr, token: &str) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let mut c = Client { ws, next_id: 1, seen: Vec::new() };
        c.send(json!({"t": "hello", "protocol": 1, "client": "e2e", "token": token})).await;
        c.until(|m| m["t"] == "welcome").await;
        c
    }

    async fn send(&mut self, v: Value) {
        self.ws.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"t": "rpc", "id": id, "call": {"method": method, "params": params}})).await;
        let reply = self.until(|m| m["id"] == id && (m["t"] == "result" || m["t"] == "error")).await;
        assert_eq!(reply["t"], "result", "{method} failed: {reply}");
        reply["ok"].clone()
    }

    /// Like `until`, but first looks at frames received since `from` (an index into `seen`).
    async fn until_after(&mut self, from: usize, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(m) = self.seen[from..].iter().find(|m| pred(m)) {
            return m.clone();
        }
        self.until(pred).await
    }

    /// Read frames until one matches (keeping all of them in `seen`).
    async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let frame = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .unwrap_or_else(|_| panic!("timed out; seen: {:#?}", self.seen))
                .unwrap()
                .unwrap();
            let Message::Text(t) = frame else { continue };
            let v: Value = serde_json::from_str(&t).unwrap();
            self.seen.push(v.clone());
            if pred(&v) {
                return v;
            }
        }
    }

    fn event_kinds(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter(|m| m["t"] == "events")
            .flat_map(|m| m["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_owned()))
            .collect()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success();
    assert!(ok, "git {args:?}");
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git_out_path(dir: &Path, arg: &str) -> PathBuf {
    let out = git_out(dir, &["rev-parse", arg]);
    let p = PathBuf::from(&out);
    if p.is_absolute() { p } else { dir.join(p) }
}

fn session_row_matches(m: &Value, pred: impl Fn(&Value) -> bool) -> bool {
    m["t"] == "patch" && m["upsert"].as_array().is_some_and(|rows| rows.iter().any(&pred))
}

#[tokio::test]
async fn session_lifecycle_over_websocket() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    let config = Config {
        home: tmp.path().join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: FAKE.into(),
        claude_home: tmp.path().join("claude"),
        idle_timeout: Duration::from_secs(600),
        auto_title: false,
        spawn: Default::default(),
        token: Some("secret".into()),
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::start(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app.router()).into_future());

    // A wrong token is refused.
    {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
        ws.send(Message::Text(json!({"t":"hello","protocol":1,"client":"x","token":"nope"}).to_string().into()))
            .await
            .unwrap();
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!() };
        assert!(t.contains("unauthorized"));
    }

    let mut c = Client::connect(addr, "secret").await;

    // Projects view: empty snapshot, then the added project arrives as a patch.
    c.send(json!({"t": "watch", "id": 1, "view": "projects"})).await;
    let snap = c.until(|m| m["t"] == "snapshot").await;
    assert_eq!(snap["rows"], json!([]));
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    let patch = c.until(|m| m["t"] == "patch" && m["watch"] == 1).await;
    assert_eq!(patch["upsert"][0]["is_git"], true);

    // Adding the same path again returns the existing project.
    let again = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await;
    assert_eq!((again["id"].clone(), again["existing"].clone()), (project.clone(), json!(true)));

    // A session in a fresh worktree; the fake agent stops at a permission request.
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;
    let created = c.rpc("session.create", json!({"project": project, "prompt": "Say hi\nthen list files"})).await;
    let (sid, stream) = (created["id"].clone(), created["stream"].as_str().unwrap().to_owned());

    let row = c.until(|m| session_row_matches(m, |r| r["status"] == "waiting")).await;
    let row = &row["upsert"][0];
    assert_eq!(row["title"], "Say hi");
    assert_eq!(row["pending"], 1);
    assert_eq!(row["workspace"]["mode"], "worktree");
    assert!(row["workspace"]["branch"].as_str().unwrap().starts_with("atelier/say-hi-"));
    assert!(Path::new(row["workspace"]["path"].as_str().unwrap()).join(".git").exists());

    // Subscribe to the session from the start: history, then live.
    c.send(json!({"t": "sub", "id": 3, "filter": {"stream": stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 3).await;
    let req = c
        .seen
        .iter()
        .filter(|m| m["t"] == "events")
        .flat_map(|m| m["events"].as_array().unwrap().clone())
        .find(|e| e["kind"] == "permission.requested")
        .expect("permission request in history");
    let request_id = req["body"]["request_id"].clone();

    c.rpc("session.answer", json!({"session": sid, "request_id": request_id, "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["status"] == "idle" && r["turns"] == 1)).await;

    // Answering twice is refused.
    c.send(json!({"t": "rpc", "id": 99, "call": {"method": "session.answer",
        "params": {"session": sid, "request_id": request_id, "decision": {"behavior": "allow"}}}}))
        .await;
    let dup = c.until(|m| m["id"] == 99).await;
    assert_eq!(dup["t"], "error");

    c.rpc("session.stop", json!({"session": sid})).await;
    c.until(|m| session_row_matches(m, |r| r["alive"] == false)).await;
    // The row can arrive before the event that caused it, so wait for the event itself.
    if !c.event_kinds().iter().any(|k| k == "process.exited") {
        c.until(|m| {
            m["t"] == "events" && m["events"].as_array().unwrap().iter().any(|e| e["kind"] == "process.exited")
        })
        .await;
    }

    let kinds = c.event_kinds();
    for k in [
        "session.created",
        "user.message",
        "process.started",
        "cc.system",
        "cc.assistant",
        "permission.requested",
        "permission.answered",
        "turn.ended",
        "process.exited",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing {k} in {kinds:?}");
    }
    assert!(!kinds.iter().any(|k| k == "cc.stream_event"), "deltas must not be persisted");

    // A restarted server finds no orphans (the process exit was recorded).
    drop(c);
    app.supervisor.shutdown().await;
}

/// The `(kind, body)` pairs of one `events` frame.
fn kinds_in(frame: &Value) -> Vec<(String, Value)> {
    frame["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["kind"].as_str().unwrap().to_owned(), e["body"].clone()))
        .collect()
}

/// Create a project and a session in it, and watch that project's sessions.
/// Returns the project id, the session id and its stream.
async fn session_in_project(c: &mut Client, repo: &Path, prompt: &str) -> (Value, Value, String) {
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;
    let created = c.rpc("session.create", json!({"project": project, "prompt": prompt})).await;
    let (sid, stream) = (created["id"].clone(), created["stream"].as_str().unwrap().to_owned());
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "waiting")).await;
    (project, sid, stream)
}

/// The titles `session.renamed` events carry in a session's stream, in order.
async fn renamed_titles(c: &mut Client, stream: &str) -> Vec<(String, String)> {
    c.send(json!({"t": "sub", "id": 9, "filter": {"stream": stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 9).await;
    c.seen
        .iter()
        .filter(|m| m["t"] == "events")
        .flat_map(|m| m["events"].as_array().unwrap().clone())
        .filter(|e| e["kind"] == "session.renamed")
        .map(|e| (e["actor"].as_str().unwrap().to_owned(), e["body"]["title"].as_str().unwrap().to_owned()))
        .collect()
}

#[tokio::test]
async fn idle_process_is_reaped_and_resumed_on_next_message() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    // A one-second timeout: the reaper runs every second.
    let (app, addr) = boot(tmp.path(), Duration::from_secs(1)).await;
    let mut c = Client::connect(addr, "secret").await;
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;

    let created = c.rpc("session.create", json!({"project": project, "prompt": "first"})).await;
    let (sid, stream) = (created["id"].clone(), created["stream"].as_str().unwrap().to_owned());
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "waiting")).await;
    c.send(json!({"t": "sub", "id": 3, "filter": {"stream": stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 3).await;

    // Waiting on a permission request is not idle, however long it waits.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    c.rpc("session.changes", json!({"session": sid})).await;
    assert!(!c.event_kinds().iter().any(|k| k == "process.exited"), "reaped while waiting");

    let request_id = c
        .seen
        .iter()
        .filter(|m| m["t"] == "events")
        .flat_map(kinds_in)
        .rfind(|(k, _)| k == "permission.requested")
        .expect("permission request")
        .1["request_id"]
        .clone();
    c.rpc("session.answer", json!({"session": sid, "request_id": request_id, "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "idle" && r["turns"] == 1)).await;

    // Idle now: the reaper stops the process, and the log says why.
    let frame = c.until(|m| m["t"] == "events" && kinds_in(m).iter().any(|(k, _)| k == "process.exited")).await;
    let (_, exited) = kinds_in(&frame).into_iter().find(|(k, _)| k == "process.exited").unwrap();
    assert_eq!((exited["reason"].clone(), exited["detail"].clone()), (json!("stopped"), json!("idle")));
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["alive"] == false && r["status"] == "idle")).await;

    // The next message starts a new process that resumes the conversation.
    c.rpc("session.send", json!({"session": sid, "text": "second"})).await;
    let second =
        c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "waiting" && r["alive"] == true)).await;
    assert_eq!(second["upsert"][0]["turns"], 1);
    let request_id = c
        .seen
        .iter()
        .filter(|m| m["t"] == "events")
        .flat_map(kinds_in)
        .rfind(|(k, _)| k == "permission.requested")
        .unwrap()
        .1["request_id"]
        .clone();
    c.rpc("session.answer", json!({"session": sid, "request_id": request_id, "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "idle" && r["turns"] == 2)).await;

    let resumed: Vec<Value> = c
        .seen
        .iter()
        .filter(|m| m["t"] == "events")
        .flat_map(kinds_in)
        .filter(|(k, _)| k == "cc.system")
        .map(|(_, b)| b["resumed"].clone())
        .collect();
    assert_eq!(resumed, vec![json!(false), json!(true)]);

    c.rpc("session.stop", json!({"session": sid})).await;
    c.until(|m| session_row_matches(m, |r| r["alive"] == false)).await;
    drop(c);
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn session_rename_over_websocket() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let (_, sid, stream) = session_in_project(&mut c, &repo, "Fix the login bug").await;

    // The new title is trimmed and arrives as a row patch.
    c.rpc("session.rename", json!({"session": sid, "title": "  Login rate limits  "})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["title"] == "Login rate limits")).await;

    let why = rpc_err(&mut c, "session.rename", json!({"session": sid, "title": "   "})).await;
    assert!(why.contains("empty"), "{why}");
    let why = rpc_err(&mut c, "session.rename", json!({"session": sid, "title": "x".repeat(201)})).await;
    assert!(why.contains("longer"), "{why}");
    // A method this server doesn't know (a newer client's) is answered with an error, not ignored.
    let why = rpc_err(&mut c, "no.such.method", json!({})).await;
    assert!(why.contains("older than this app"), "{why}");

    // The turn ends (the fake agent's request id is "r1"); the name holds.
    c.rpc("session.answer", json!({"session": sid, "request_id": "r1", "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "idle")).await;
    let last = c.seen.iter().rev().find(|m| session_row_matches(m, |r| r["id"] == sid)).unwrap().clone();
    assert_eq!(last["upsert"][0]["title"], "Login rate limits");

    // The rename is in the log, as the person's.
    assert_eq!(renamed_titles(&mut c, &stream).await, vec![("user".into(), "Login rate limits".into())]);
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn archived_session_can_be_unarchived_but_not_resumed_without_its_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let (_, sid, _) = session_in_project(&mut c, &repo, "Tidy the README").await;
    c.rpc("session.answer", json!({"session": sid, "request_id": "r1", "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["status"] == "idle")).await;

    // Clean worktree: archiving removes it. Patches that arrive while an rpc is awaited are
    // already in `seen`, so each wait starts from the frames received after its own call.
    let mark = c.seen.len();
    let archived = c.rpc("session.archive", json!({"session": sid})).await;
    assert_eq!(archived["removed"], true, "{archived}");
    c.until_after(mark, |m| session_row_matches(m, |r| r["id"] == sid && r["archived"] == true)).await;

    let mark = c.seen.len();
    c.rpc("session.unarchive", json!({"session": sid})).await;
    c.until_after(mark, |m| session_row_matches(m, |r| r["id"] == sid && r["archived"] == false)).await;

    // The worktree is gone, so a new message is refused before anything is logged.
    let why = rpc_err(&mut c, "session.send", json!({"session": sid, "text": "hello again"})).await;
    assert!(why.contains("worktree was removed"), "{why}");
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn auto_title_names_session_after_first_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    // The fake answers the one-shot titler with a fixed title, so this needs no network.
    let (app, addr) = boot_with(tmp.path(), Duration::from_secs(600), true).await;
    let mut c = Client::connect(addr, "secret").await;
    let (_, sid, stream) = session_in_project(&mut c, &repo, "Fix the login bug").await;
    assert_eq!(
        c.seen.iter().rev().find(|m| session_row_matches(m, |r| r["id"] == sid)).unwrap()["upsert"][0]["title"],
        "Fix the login bug"
    );

    c.rpc("session.answer", json!({"session": sid, "request_id": "r1", "decision": {"behavior": "allow"}})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["title"] == "Login rate limits")).await;
    assert_eq!(renamed_titles(&mut c, &stream).await, vec![("atelier".into(), "Login rate limits".into())]);
    app.supervisor.shutdown().await;
}

/// Stands in for `tailscale whois`: trusts exactly the listed addresses.
struct FakeTailnet(Vec<IpAddr>);

impl PeerAuth for FakeTailnet {
    fn trusted(&self, ip: IpAddr) -> futures::future::BoxFuture<'_, bool> {
        Box::pin(async move { self.0.contains(&ip) })
    }
}

/// Send a hello (with `token`, if any, and `origin` on the upgrade) and return
/// `welcome`, or the `fatal` error text.
async fn hello(addr: SocketAddr, token: Option<&str>, origin: Option<&str>) -> String {
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let mut msg = json!({"t": "hello", "protocol": 1, "client": "e2e"});
    if let Some(t) = token {
        msg["token"] = json!(t);
    }
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();
    let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no reply") };
    let v: Value = serde_json::from_str(&t).unwrap();
    if v["t"] == "welcome" { "welcome".into() } else { v["error"].as_str().unwrap().to_owned() }
}

async fn serve(app: App, tailnet: bool) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    if tailnet {
        let svc = app.tailnet_router().into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(axum::serve(listener, svc).into_future());
    } else {
        tokio::spawn(axum::serve(listener, app.router()).into_future());
    }
    addr
}

#[tokio::test]
async fn tailnet_peers_and_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        home: tmp.path().join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: FAKE.into(),
        claude_home: tmp.path().join("claude"),
        idle_timeout: Duration::from_secs(600),
        auto_title: false,
        spawn: Default::default(),
        token: Some("secret".into()),
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::start(config).await.unwrap();

    let local: IpAddr = "127.0.0.1".parse().unwrap();
    let mut trusting = app.clone();
    trusting.peers = Arc::new(FakeTailnet(vec![local]));
    let mut stranger = app.clone();
    stranger.peers = Arc::new(FakeTailnet(vec![]));

    // Loopback: the token is required, whoever vouches for the peer.
    let loopback = serve(trusting.clone(), false).await;
    assert_eq!(hello(loopback, Some("secret"), None).await, "welcome");
    assert!(hello(loopback, None, None).await.starts_with("unauthorized"));
    assert!(hello(loopback, Some("nope"), None).await.starts_with("unauthorized"));

    // Tailnet, trusted peer: no token needed, but not from a foreign web page.
    let trusted = serve(trusting, true).await;
    assert_eq!(hello(trusted, None, None).await, "welcome");
    assert_eq!(hello(trusted, None, Some("tauri://localhost")).await, "welcome");
    assert!(hello(trusted, None, Some("https://evil.example")).await.starts_with("unauthorized"));
    assert_eq!(hello(trusted, Some("secret"), Some("https://evil.example")).await, "welcome");

    // Tailnet, someone else's device: only the token gets in.
    let untrusted = serve(stranger, true).await;
    let refused = hello(untrusted, None, None).await;
    assert!(refused.starts_with("unauthorized") && refused.contains("Tailscale"), "{refused}");
    assert_eq!(hello(untrusted, Some("secret"), None).await, "welcome");

    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn workspaces_from_other_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;

    // The owner builds: an uncommitted edit in its worktree.
    let owner = c.rpc("session.create", json!({"project": project, "prompt": "build"})).await["id"].clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == owner && r["status"] == "waiting")).await;
    let owner_ws = row["upsert"][0]["workspace"].clone();
    let opath = Path::new(owner_ws["path"].as_str().unwrap()).to_owned();
    std::fs::write(opath.join("a.txt"), "one\nowner edit\n").unwrap();

    // Share it read-only: same directory, started with the edit tools disallowed.
    let reader = c
        .rpc("session.create", json!({"project": project, "prompt": "review", "workspace": "shared", "from": owner}))
        .await["id"]
        .clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == reader && r["status"] == "waiting")).await;
    let rws = row["upsert"][0]["workspace"].clone();
    assert_eq!((rws["mode"].as_str(), rws["path"].as_str()), (Some("shared"), owner_ws["path"].as_str()), "{rws}");
    assert_eq!(rws["from"], owner);
    let started_with = std::fs::read_to_string(git_out_path(&opath, "--git-dir").join("atelier-fake-args")).unwrap();
    assert!(started_with.contains("--disallowedTools Edit,Write,NotebookEdit"), "{started_with}");
    // Sharing a reader resolves to the owner.
    let reader2 = c
        .rpc("session.create", json!({"project": project, "prompt": "again", "workspace": "shared", "from": reader}))
        .await["id"]
        .clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == reader2 && r["status"] == "waiting")).await;
    assert_eq!(row["upsert"][0]["workspace"]["from"], owner);

    // Branch from it: a fresh worktree at a snapshot that includes the owner's uncommitted edit.
    let child = c
        .rpc("session.create", json!({"project": project, "prompt": "alt", "workspace": "worktree", "from": owner}))
        .await["id"]
        .clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == child && r["status"] == "waiting")).await;
    let cws = row["upsert"][0]["workspace"].clone();
    let cpath = Path::new(cws["path"].as_str().unwrap()).to_owned();
    assert_ne!(cpath, opath);
    // The stored first message carries the snapshot note ahead of the prompt. The owner was waiting, not running,
    // so the running note is absent.
    let child_stream = format!("p/{}/s/{}", project.as_str().unwrap(), child.as_str().unwrap());
    c.send(json!({"t": "sub", "id": 9, "filter": {"stream": child_stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 9).await;
    let first = c
        .seen
        .iter()
        .filter(|m| m["t"] == "events" && m["sub"] == 9)
        .flat_map(kinds_in)
        .find(|(k, _)| k == "user.message")
        .expect("the branch's first message")
        .1["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(first.starts_with("This session's branch starts at a snapshot of its source's working tree"), "{first}");
    assert!(first.ends_with("\n\nalt"), "{first}");
    assert!(!first.contains("mid-edit"), "{first}");
    assert_eq!((cws["mode"].as_str(), cws["from"].clone()), (Some("worktree"), owner.clone()), "{cws}");
    assert!(cws["snapshot"].is_string(), "{cws}");
    assert_eq!(cws["base"], cws["snapshot"]);
    assert_eq!(std::fs::read_to_string(cpath.join("a.txt")).unwrap(), "one\nowner edit\n");
    let ch = c.rpc("session.changes", json!({"session": child})).await;
    assert_eq!(ch["files"], json!([]));
    // The owner is untouched by that.
    assert!(git_out(&opath, &["status", "--porcelain"]).contains("a.txt"));

    // Refusals: no `from` for a share, a directory has nothing to share, and the source must be live.
    let e = rpc_err(&mut c, "session.create", json!({"project": project, "prompt": "x", "workspace": "shared"})).await;
    assert!(e.contains("needs the session"), "{e}");
    let e = rpc_err(
        &mut c,
        "session.create",
        json!({"project": project, "prompt": "x", "workspace": "direct", "from": owner}),
    )
    .await;
    assert!(e.contains("no worktree of its own"), "{e}");

    // Archiving the owner keeps the worktree while a reader is live.
    let r = c.rpc("session.archive", json!({"session": owner})).await;
    assert_eq!(r["removed"], false);
    assert!(r["kept"].as_str().unwrap().contains("is reading this worktree"), "{r}");
    assert!(opath.exists());
    // Nothing else is handed the reader's directory while it's in use.
    let other = c.rpc("session.create", json!({"project": project, "prompt": "other"})).await["id"].clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == other && r["status"] == "waiting")).await;
    assert_ne!(row["upsert"][0]["workspace"]["path"], owner_ws["path"]);

    // Archiving a reader never touches the worktree; the last reader out releases the archived owner's.
    let r = c.rpc("session.archive", json!({"session": reader})).await;
    assert_eq!(r["removed"], false);
    assert!(r["kept"].is_null(), "{r}");
    assert!(opath.exists());
    // Clean first: the owner's uncommitted edit is what would otherwise keep the worktree.
    git(&opath, &["checkout", "-q", "--", "a.txt"]);
    let r = c.rpc("session.archive", json!({"session": reader2})).await;
    assert_eq!(r["removed"], true, "{r}");
    let owner_stream = format!("p/{}/s/{}", project.as_str().unwrap(), owner.as_str().unwrap());
    c.send(json!({"t": "sub", "id": 8, "filter": {"stream": owner_stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 8).await;
    assert!(c.event_kinds().iter().any(|k| k == "workspace.removed"));

    // A branch's snapshot ref goes with its worktree.
    let snap_ref = format!("refs/atelier/snap/{}", child.as_str().unwrap());
    assert!(!git_out(&repo, &["for-each-ref", &snap_ref]).is_empty());
    let r = c.rpc("session.archive", json!({"session": child})).await;
    assert_eq!(r["removed"], true, "{r}");
    assert!(git_out(&repo, &["for-each-ref", &snap_ref]).is_empty());
    app.supervisor.shutdown().await;
}

async fn boot(tmp: &Path, idle_timeout: Duration) -> (App, std::net::SocketAddr) {
    boot_with(tmp, idle_timeout, false).await
}

async fn boot_with(tmp: &Path, idle_timeout: Duration, auto_title: bool) -> (App, std::net::SocketAddr) {
    let config = Config {
        home: tmp.join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: FAKE.into(),
        claude_home: tmp.join("claude"),
        idle_timeout,
        auto_title,
        spawn: Default::default(),
        token: Some("secret".into()),
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::start(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app.router()).into_future());
    // Agents started from here on get Atelier's MCP tools, as under `atelierd`.
    app.supervisor.set_mcp_addr(addr);
    (app, addr)
}

async fn rpc_err(c: &mut Client, method: &str, params: Value) -> String {
    let id = 1000 + c.next_id;
    c.send(json!({"t": "rpc", "id": id, "call": {"method": method, "params": params}})).await;
    let reply = c.until(|m| m["id"] == id).await;
    assert_eq!(reply["t"], "error", "{method} should fail: {reply}");
    reply["error"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn worktree_changes_and_archive_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);

    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;

    // ── dirty case ──
    let dirty = c.rpc("session.create", json!({"project": project, "prompt": "dirty"})).await["id"].clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == dirty && r["status"] == "waiting")).await;
    let ws = row["upsert"][0]["workspace"].clone();
    let (path, branch) =
        (Path::new(ws["path"].as_str().unwrap()).to_owned(), ws["branch"].as_str().unwrap().to_owned());

    let none = c.rpc("session.changes", json!({"session": dirty})).await;
    assert_eq!(none["files"], json!([]));

    std::fs::write(path.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(path.join("new.txt"), "x\ny\n").unwrap();
    let ch = c.rpc("session.changes", json!({"session": dirty})).await;
    assert_eq!(
        ch["files"],
        json!([
            {"path": "a.txt", "status": "modified", "added": 2, "removed": 1},
            {"path": "new.txt", "status": "untracked", "added": 2, "removed": 0},
        ])
    );
    let patch = c.rpc("session.diff", json!({"session": dirty, "path": "a.txt"})).await["patch"].clone();
    assert!(patch.as_str().unwrap().contains("+TWO"), "{patch}");
    let patch = c.rpc("session.diff", json!({"session": dirty, "path": "new.txt"})).await["patch"].clone();
    assert!(patch.as_str().unwrap().contains("+x"), "{patch}");
    rpc_err(&mut c, "session.diff", json!({"session": dirty, "path": "../repo/a.txt"})).await;

    // Live agent: the project can't be removed.
    let why = rpc_err(&mut c, "project.remove", json!({"project": project})).await;
    assert!(why.contains("running"), "{why}");

    // Uncommitted work: archive keeps everything and says why.
    let r = c.rpc("session.archive", json!({"session": dirty})).await;
    assert_eq!(r["removed"], false);
    assert!(r["kept"].as_str().unwrap().contains("uncommitted"), "{r}");
    assert!(path.exists());

    // ── clean case ──
    let clean = c.rpc("session.create", json!({"project": project, "prompt": "clean"})).await["id"].clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == clean && r["status"] == "waiting")).await;
    let ws = &row["upsert"][0]["workspace"];
    let (cpath, cbranch) =
        (Path::new(ws["path"].as_str().unwrap()).to_owned(), ws["branch"].as_str().unwrap().to_owned());
    let r = c.rpc("session.archive", json!({"session": clean})).await;
    assert_eq!(r["removed"], true, "{r}");
    // The directory is pooled: kept (detached) for the next session, along with its ignored files.
    assert!(cpath.exists());
    assert!(git_out(&cpath, &["branch", "--show-current"]).is_empty());
    std::fs::write(repo.join(".git/info/exclude"), "warm.marker\n").unwrap();
    std::fs::write(cpath.join("warm.marker"), "x").unwrap();
    let branches = std::process::Command::new("git").arg("-C").arg(&repo).args(["branch", "--list"]).output().unwrap();
    let branches = String::from_utf8_lossy(&branches.stdout);
    assert!(!branches.contains(&cbranch) && branches.contains(&branch), "{branches}");

    // A committed, unmerged branch is kept too.
    git(&path, &["add", "."]);
    git(&path, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "work"]);
    let unmerged = c.rpc("session.create", json!({"project": project, "prompt": "unmerged"})).await["id"].clone();
    c.until(|m| session_row_matches(m, |r| r["id"] == unmerged && r["status"] == "waiting")).await;
    let row = c.seen.iter().rev().find(|m| session_row_matches(m, |r| r["id"] == unmerged)).unwrap().clone();
    let upath = Path::new(row["upsert"][0]["workspace"]["path"].as_str().unwrap()).to_owned();
    assert_eq!(upath, cpath, "the pooled worktree is reused");
    assert!(upath.join("warm.marker").exists());
    assert_eq!(
        git_out(&upath, &["branch", "--show-current"]),
        row["upsert"][0]["workspace"]["branch"].as_str().unwrap()
    );
    std::fs::write(upath.join("b.txt"), "b\n").unwrap();
    git(&upath, &["add", "."]);
    git(&upath, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "b"]);
    let r = c.rpc("session.archive", json!({"session": unmerged})).await;
    assert_eq!(r["removed"], false);
    assert!(r["kept"].as_str().unwrap().contains("aren't merged"), "{r}");
    let ch = c.rpc("session.changes", json!({"session": unmerged})).await;
    assert_eq!(ch["files"][0]["path"], "b.txt");

    // The removal is in the log.
    let clean_stream = format!("p/{}/s/{}", project.as_str().unwrap(), clean.as_str().unwrap());
    c.send(json!({"t": "sub", "id": 7, "filter": {"stream": clean_stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 7).await;
    assert!(c.event_kinds().iter().any(|k| k == "workspace.removed"));
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn import_runs_only_on_request_and_only_adds_what_is_new() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let cwd = repo.to_str().unwrap();

    let config = Config {
        home: tmp.path().join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: FAKE.into(),
        claude_home: tmp.path().join("claude"),
        idle_timeout: Duration::from_secs(600),
        auto_title: false,
        spawn: Default::default(),
        token: Some("secret".into()),
    };
    // The history already exists when the project is added, as with a real `~/.claude`.
    let slug: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = config.claude_home.join("projects").join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    let theirs = dir.join("native-1.jsonl");
    let line = |ty: &str, ts: &str, content: Value| {
        json!({"type": ty, "cwd": cwd, "timestamp": ts, "message": {"role": ty, "content": content}}).to_string()
    };
    let mut f = std::fs::File::create(&theirs).unwrap();
    writeln!(f, r#"{{"type":"queue-operation","operation":"enqueue"}}"#).unwrap();
    writeln!(f, "{}", line("user", "2026-01-01T00:00:00.000Z", json!("Fix the login bug"))).unwrap();
    writeln!(
        f,
        "{}",
        line(
            "assistant",
            "2026-01-01T00:00:05.000Z",
            json!([{"type": "thinking", "thinking": "", "signature": "SIG"}])
        )
    )
    .unwrap();
    // A session from some other directory is not ours.
    let other = dir.join("native-2.jsonl");
    std::fs::write(
        &other,
        line("user", "2026-01-01T00:00:00.000Z", json!("elsewhere")).replace(cwd, "/elsewhere") + "\n",
    )
    .unwrap();

    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::start(config).await.unwrap();
    let addr = serve(app, false).await;
    let mut c = Client::connect(addr, "secret").await;

    let project = c.rpc("project.add", json!({"path": cwd})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 1, "view": "sessions", "scope": project})).await;
    let snap = c.until(|m| m["t"] == "snapshot" && m["watch"] == 1).await;
    // Adding the project did not import anything.
    assert_eq!(snap["rows"], json!([]));

    let r = c.rpc("project.import", json!({"project": project})).await;
    assert_eq!((r["sessions"].clone(), r["messages"].clone()), (json!(1), json!(2)));
    // The patch may already have arrived while the rpc reply was awaited.
    let is_row =
        |m: &Value| session_row_matches(m, |r| r["imported"] == true && r["updated_at"] == 1_767_225_605_000i64);
    let row = match c.seen.iter().find(|m| is_row(m)) {
        Some(m) => m.clone(),
        None => c.until(is_row).await,
    };
    let row = &row["upsert"][0];
    assert_eq!(row["title"], "Fix the login bug");
    assert_eq!(row["created_at"], 1_767_225_600_000i64);
    assert_eq!(row["workspace"]["path"], cwd);
    let stream = row["stream"].as_str().unwrap().to_owned();

    // Nothing new: nothing imported.
    let r = c.rpc("project.import", json!({"project": project})).await;
    assert_eq!((r["sessions"].clone(), r["messages"].clone()), (json!(0), json!(0)));

    // The terminal session continues, mid-write at the end; only the finished line comes over.
    let mut f = std::fs::OpenOptions::new().append(true).open(&theirs).unwrap();
    writeln!(f, "{}", line("user", "2026-01-01T00:01:00.000Z", json!("and the signup?"))).unwrap();
    write!(f, r#"{{"type":"assistant","#).unwrap();
    let r = c.rpc("project.import", json!({"project": project})).await;
    assert_eq!((r["sessions"].clone(), r["messages"].clone()), (json!(0), json!(1)));
    let r = c.rpc("project.import", json!({"project": project})).await;
    assert_eq!(r["messages"], json!(0));

    c.send(json!({"t": "sub", "id": 2, "filter": {"stream": stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 2).await;
    let kinds = c.event_kinds();
    assert_eq!(kinds.iter().filter(|k| *k == "session.created").count(), 1);
    assert_eq!(kinds.iter().filter(|k| *k == "cc.user").count(), 2);
    assert_eq!(kinds.iter().filter(|k| *k == "cc.assistant").count(), 1);
    let blob = serde_json::to_string(&c.seen).unwrap();
    assert!(!blob.contains("SIG"), "thinking signatures are stripped");
}

/// One HTTP request over a plain socket: `(status, body)`.
async fn http(addr: SocketAddr, method: &str, path: &str, bearer: Option<&str>, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, out.split_once("\r\n\r\n").map(|x| x.1).unwrap_or_default().to_owned())
}

/// The first `cc.user` event in `stream`: what the fake agent printed after calling a tool.
async fn tool_result(c: &mut Client, stream: &str, sub: u64) -> Value {
    c.send(json!({"t": "sub", "id": sub, "filter": {"stream": stream}, "from": 1})).await;
    let frame = c
        .until(|m| {
            m["t"] == "events"
                && m["sub"] == sub
                && m["events"].as_array().unwrap().iter().any(|e| e["kind"] == "cc.user")
        })
        .await;
    let ev = frame["events"].as_array().unwrap().iter().find(|e| e["kind"] == "cc.user").unwrap().clone();
    ev["body"]["message"]["content"][0]["content"].clone()
}

#[tokio::test]
async fn agents_see_other_sessions_through_the_mcp_server() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;

    // A session that knows something.
    let (project, other, _) = session_in_project(&mut c, &repo, "WAL checkpoint notes").await;

    // An agent lists sessions and finds itself marked, next to the other one.
    let me = c.rpc("session.create", json!({"project": project, "prompt": "MCP:sessions:{}"})).await;
    let listing = tool_result(&mut c, me["stream"].as_str().unwrap(), 10).await;
    assert!(listing.get("error").is_none(), "{listing}");
    let text = listing["result"]["content"][0]["text"].as_str().unwrap();
    let mine = text.lines().find(|l| l.starts_with(me["id"].as_str().unwrap())).unwrap_or_else(|| panic!("{text}"));
    assert!(mine.ends_with("(you)"), "{text}");
    assert!(text.contains(&format!("{} · WAL checkpoint notes", other.as_str().unwrap())), "{text}");

    // Another reads that session and gets what was said in it.
    let prompt = format!("MCP:read:{{\"session\":\"{}\"}}", other.as_str().unwrap());
    let reader = c.rpc("session.create", json!({"project": project, "prompt": prompt})).await;
    let read = tool_result(&mut c, reader["stream"].as_str().unwrap(), 11).await;
    let text = read["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("user: WAL checkpoint notes") && text.contains("agent: hi"), "{text}");
    assert!(!text.contains("Bash"), "a permission request isn't conversation: {text}");

    // What the agent was started with: Atelier's server, its tools, and who it is.
    let ws = c.rpc("session.create", json!({"project": project, "prompt": "MCP:sessions:{}"})).await;
    tool_result(&mut c, ws["stream"].as_str().unwrap(), 12).await;
    let wpath = {
        let rows = app.store.read(|c| atelier_server::views::sessions::list(c, None)).await.unwrap();
        rows.into_iter().find(|r| r.id.as_str() == ws["id"].as_str().unwrap()).unwrap().workspace.path
    };
    let args = std::fs::read_to_string(git_out_path(Path::new(&wpath), "--git-dir").join("atelier-fake-args")).unwrap();
    assert!(args.contains("--allowedTools mcp__atelier__sessions,mcp__atelier__read"), "{args}");
    assert!(args.contains(&format!("--append-system-prompt You are Atelier session {}", ws["id"].as_str().unwrap())));
    let token = args.split("Bearer ").nth(1).unwrap().split('"').next().unwrap().to_owned();

    // The endpoint wants that token: nothing, a wrong one or GET gets no further.
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    assert_eq!(http(addr, "POST", "/mcp", None, ping).await.0, 401);
    assert_eq!(http(addr, "POST", "/mcp", Some("nope"), ping).await.0, 401);
    assert_eq!(http(addr, "GET", "/mcp", Some(&token), "").await.0, 405);
    let (status, body) = http(addr, "POST", "/mcp", Some(&token), ping).await;
    assert_eq!((status, body.contains("\"result\"")), (200, true), "{body}");
    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert_eq!(http(addr, "POST", "/mcp", Some(&token), note).await.0, 202);

    // The tailnet listener doesn't serve it at all, even for a valid token.
    let tailnet = serve(app.clone(), true).await;
    assert_eq!(http(tailnet, "POST", "/mcp", Some(&token), ping).await.0, 404);

    // The token dies with its process.
    c.rpc("session.stop", json!({"session": ws["id"]})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == ws["id"] && r["alive"] == false)).await;
    assert_eq!(http(addr, "POST", "/mcp", Some(&token), ping).await.0, 401);
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn calls_made_by_an_agent_are_attributed_to_it_and_limited() {
    use atelier_core::{Actor, SessionId};
    use atelier_proto::Call;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let (project, sid, stream) = session_in_project(&mut c, &repo, "hello").await;

    let agent = Actor::Agent(SessionId::from(sid.as_str().unwrap()));
    let rename = Call::SessionRename { session: SessionId::from(sid.as_str().unwrap()), title: "By itself".into() };
    atelier_server::rpc::handle(&app, agent.clone(), Some(7), rename).await.unwrap();
    let st = stream.clone();
    let evs = app.store.events(atelier_core::stream::StreamFilter::Stream(st), 1, 1000).await.unwrap();
    let renamed = evs.iter().find(|e| e.kind == "session.renamed").unwrap();
    assert_eq!((renamed.actor.as_str(), renamed.cause), (format!("agent:{}", sid.as_str().unwrap()).as_str(), Some(7)));

    // People only: removing projects and browsing the server's disk.
    let remove = Call::ProjectRemove { project: project.as_str().unwrap().into() };
    let err = atelier_server::rpc::handle(&app, agent.clone(), None, remove).await.unwrap_err();
    assert!(format!("{err}").contains("only a person"), "{err}");
    let err = atelier_server::rpc::handle(&app, agent, None, Call::FsSuggest {}).await.unwrap_err();
    assert!(format!("{err}").contains("only a person"), "{err}");
    app.supervisor.shutdown().await;
}

#[tokio::test]
async fn agents_spawn_child_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]);
    let (app, addr) = boot(tmp.path(), Duration::from_secs(600)).await;
    let mut c = Client::connect(addr, "secret").await;
    let project = c.rpc("project.add", json!({"path": repo.to_str().unwrap()})).await["id"].clone();
    c.send(json!({"t": "watch", "id": 2, "view": "sessions", "scope": project})).await;
    c.until(|m| m["t"] == "snapshot" && m["watch"] == 2).await;

    // The parent has an uncommitted file in its worktree, and holds an MCP token.
    let parent = c.rpc("session.create", json!({"project": project, "prompt": "build"})).await["id"].clone();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == parent && r["status"] == "waiting")).await;
    let ppath = Path::new(row["upsert"][0]["workspace"]["path"].as_str().unwrap()).to_owned();
    std::fs::write(ppath.join("wip.txt"), "uncommitted\n").unwrap();
    let args = std::fs::read_to_string(git_out_path(&ppath, "--git-dir").join("atelier-fake-args")).unwrap();
    assert!(!args.contains("mcp__atelier__spawn"), "spawn must go through the permission mode: {args}");
    let token = args.split("Bearer ").nth(1).unwrap().split('"').next().unwrap().to_owned();
    let mut n = 0;
    let mut tool = |name: &'static str, a: Value| {
        n += 1;
        let body = json!({"jsonrpc": "2.0", "id": n, "method": "tools/call", "params": {"name": name, "arguments": a}});
        let token = token.clone();
        async move {
            let (status, body) = http(addr, "POST", "/mcp", Some(&token), &body.to_string()).await;
            assert_eq!(status, 200, "{body}");
            let v: Value = serde_json::from_str(&body).unwrap();
            (v["result"]["isError"] == true, v["result"]["content"][0]["text"].as_str().unwrap_or_default().to_owned())
        }
    };

    // Default: a new worktree at a snapshot including the parent's uncommitted file.
    let (is_err, text) = tool("spawn", json!({"task": "try the other way"})).await;
    assert!(!is_err && text.starts_with("started "), "{text}");
    let child = text.split_whitespace().nth(1).unwrap().to_owned();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == child && r["status"] == "waiting")).await;
    let r = row["upsert"][0].clone();
    assert_eq!(r["parent"], parent);
    let cpath = Path::new(r["workspace"]["path"].as_str().unwrap()).to_owned();
    assert_ne!(cpath, ppath);
    assert_eq!(std::fs::read_to_string(cpath.join("wip.txt")).unwrap(), "uncommitted\n");
    assert_eq!(r["workspace"]["from"], parent);
    // The stream carries `parent` on `session.created`, and the first message the envelope.
    let stream = format!("p/{}/s/{child}", project.as_str().unwrap());
    c.send(json!({"t": "sub", "id": 9, "filter": {"stream": stream}, "from": 1})).await;
    c.until(|m| m["t"] == "live" && m["sub"] == 9).await;
    let evs: Vec<_> = c.seen.iter().filter(|m| m["t"] == "events" && m["sub"] == 9).flat_map(kinds_in).collect();
    let created = &evs.iter().find(|(k, _)| k == "session.created").unwrap().1;
    assert_eq!(created["parent"], parent);
    let first = evs.iter().find(|(k, _)| k == "user.message").unwrap().1["text"].as_str().unwrap().to_owned();
    assert!(first.contains(&format!("started by Atelier session {}", parent.as_str().unwrap())), "{first}");
    assert!(first.contains("a snapshot of session") && first.ends_with("try the other way"), "{first}");

    // Share: the parent's own directory, read-only.
    let (is_err, text) = tool("spawn", json!({"task": "look", "workspace": {"share": "self"}})).await;
    assert!(!is_err && text.contains("read-only"), "{text}");
    let shared = text.split_whitespace().nth(1).unwrap().to_owned();
    let row = c.until(|m| session_row_matches(m, |r| r["id"] == shared && r["status"] == "waiting")).await;
    assert_eq!(row["upsert"][0]["workspace"]["path"].as_str().unwrap(), ppath.to_str().unwrap());
    let started = std::fs::read_to_string(git_out_path(&ppath, "--git-dir").join("atelier-fake-args")).unwrap();
    assert!(started.contains("--disallowedTools Edit,Write,NotebookEdit"), "{started}");

    // `review` is the same, with the review task.
    let (is_err, text) = tool("spawn", json!({"review": "self"})).await;
    assert!(!is_err && text.contains("read-only"), "{text}");

    // Bad parameters are refused before anything starts; so is the 5th live child.
    let (is_err, text) = tool("spawn", json!({"task": "x", "workspace": "elsewhere"})).await;
    assert!(is_err || text.is_empty());
    let (is_err, text) = tool("spawn", json!({"task": "fresh one", "workspace": "fresh"})).await;
    assert!(!is_err, "{text}");
    let (is_err, text) = tool("spawn", json!({"task": "one too many", "workspace": "fresh"})).await;
    assert!(is_err && text.contains("4 children") && text.contains("ATELIER_SPAWN_CHILDREN"), "{text}");
    app.supervisor.shutdown().await;
}
