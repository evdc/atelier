//! End to end over a real WebSocket, with a scripted stand-in for `claude`
//! (`atelier-driver/tests/fixtures/fake-claude.sh`).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
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
        auto_title: false,
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
async fn session_rename_over_websocket() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    let (app, addr) = boot(tmp.path()).await;
    let mut c = Client::connect(addr, "secret").await;
    let (_, sid, stream) = session_in_project(&mut c, &repo, "Fix the login bug").await;

    // The new title is trimmed and arrives as a row patch.
    c.rpc("session.rename", json!({"session": sid, "title": "  Login rate limits  "})).await;
    c.until(|m| session_row_matches(m, |r| r["id"] == sid && r["title"] == "Login rate limits")).await;

    let why = rpc_err(&mut c, "session.rename", json!({"session": sid, "title": "   "})).await;
    assert!(why.contains("empty"), "{why}");
    let why = rpc_err(&mut c, "session.rename", json!({"session": sid, "title": "x".repeat(201)})).await;
    assert!(why.contains("longer"), "{why}");

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
async fn auto_title_names_session_after_first_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);

    // The fake answers the one-shot titler with a fixed title, so this needs no network.
    let (app, addr) = boot_with(tmp.path(), true).await;
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
        auto_title: false,
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

async fn boot(tmp: &Path) -> (App, std::net::SocketAddr) {
    boot_with(tmp, false).await
}

async fn boot_with(tmp: &Path, auto_title: bool) -> (App, std::net::SocketAddr) {
    let config = Config {
        home: tmp.join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: FAKE.into(),
        auto_title,
        token: Some("secret".into()),
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::start(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app.router()).into_future());
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

    let (app, addr) = boot(tmp.path()).await;
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
    assert!(!cpath.exists());
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
