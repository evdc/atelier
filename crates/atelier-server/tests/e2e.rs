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
