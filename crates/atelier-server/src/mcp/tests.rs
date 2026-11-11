use std::sync::Arc;
use std::time::Duration;

use atelier_core::domain::{AgentKind, ProjectAdded, SessionCreated, UserMessage, Workspace, WorkspaceMode};
use atelier_core::{Actor, NewEvent, ProjectId, SessionId, stream};
use atelier_driver::ClaudeCode;
use serde_json::{Value, json};

use super::*;
use crate::Config;

async fn app() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        home: tmp.path().join("home"),
        listen: "127.0.0.1:0".parse().unwrap(),
        listen_tailscale: false,
        tailscale_bin: "false".into(),
        claude_bin: "false".into(),
        claude_home: tmp.path().join("claude"),
        idle_timeout: Duration::from_secs(600),
        auto_title: false,
        spawn: Default::default(),
        token: None,
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let app = App::with_driver(config, Arc::new(ClaudeCode::default())).await.unwrap();
    (app, tmp)
}

async fn session(app: &App, project: &ProjectId, title: &str) -> SessionId {
    let id = SessionId::generate();
    let created = SessionCreated {
        id: id.clone(),
        project: project.clone(),
        title: title.into(),
        agent: AgentKind::ClaudeCode,
        workspace: Workspace {
            mode: WorkspaceMode::Direct,
            path: "/w".into(),
            branch: None,
            base: None,
            from: None,
            snapshot: None,
        },
        model: None,
        origin: None,
        started_at: None,
        parent: None,
    };
    app.store.append_one(NewEvent::new(stream::session(project, &id), Actor::User, &created)).await.unwrap();
    id
}

async fn project(app: &App, name: &str) -> ProjectId {
    let id = ProjectId::generate();
    let added = ProjectAdded { id: id.clone(), name: name.into(), path: format!("/{name}"), is_git: false };
    app.store.append_one(NewEvent::new(stream::GLOBAL, Actor::User, &added)).await.unwrap();
    id
}

fn call(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn text(resp: &Value) -> &str {
    resp["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text in {resp}"))
}

#[tokio::test]
async fn initialize_negotiates_a_known_version() {
    let (app, _t) = app().await;
    let me = SessionId::generate();
    let r = handle(&app, &me, call(1, "initialize", json!({"protocolVersion": "2025-03-26"}))).await.unwrap();
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    let r = handle(&app, &me, call(2, "initialize", json!({"protocolVersion": "1999-01-01"}))).await.unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn notifications_get_no_reply_and_ping_works() {
    let (app, _t) = app().await;
    let me = SessionId::generate();
    assert!(handle(&app, &me, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.is_none());
    let r = handle(&app, &me, call(7, "ping", Value::Null)).await.unwrap();
    assert_eq!(r["result"], json!({}));
}

#[tokio::test]
async fn tools_list_describes_every_tool() {
    let (app, _t) = app().await;
    let r = handle(&app, &SessionId::generate(), call(1, "tools/list", Value::Null)).await.unwrap();
    let names: Vec<_> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["sessions", "read", "send", "spawn"]);
    for t in r["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn protocol_errors() {
    let (app, _t) = app().await;
    let me = SessionId::generate();
    let r = handle(&app, &me, call(1, "resources/list", Value::Null)).await.unwrap();
    assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
    let r = handle(&app, &me, json!([call(1, "ping", Value::Null)])).await.unwrap();
    assert_eq!(r["error"]["code"], INVALID_REQUEST);
    let r = handle(&app, &me, call(1, "tools/call", json!({"name": "nope"}))).await.unwrap();
    assert_eq!(r["error"]["code"], INVALID_PARAMS);
    // Bad arguments are a protocol error; a well-formed call that fails is a result the model can read.
    let r =
        handle(&app, &me, call(1, "tools/call", json!({"name": "read", "arguments": {"session": 5}}))).await.unwrap();
    assert_eq!(r["error"]["code"], INVALID_PARAMS);
    let r = handle(&app, &me, call(1, "tools/call", json!({"name": "read", "arguments": {}}))).await.unwrap();
    assert_eq!(r["error"]["code"], INVALID_PARAMS);
    let r = handle(&app, &me, call(1, "tools/call", json!({"name": "read", "arguments": {"session": "nope"}})))
        .await
        .unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(text(&r).contains("no such session"));
}

#[tokio::test]
async fn sessions_tool_scopes_to_your_project_and_hides_archived() {
    let (app, _t) = app().await;
    let (mine, other) = (project(&app, "mine").await, project(&app, "other").await);
    let me = session(&app, &mine, "Me").await;
    let sibling = session(&app, &mine, "Sibling").await;
    let _elsewhere = session(&app, &other, "Elsewhere").await;
    let archived = session(&app, &mine, "Gone").await;
    let s = stream::session(&mine, &archived);
    app.store.append_one(NewEvent::new(s, Actor::User, &atelier_core::domain::SessionArchived {})).await.unwrap();

    let list = |args: Value| {
        let (app, me) = (app.clone(), me.clone());
        async move {
            handle(&app, &me, call(1, "tools/call", json!({"name": "sessions", "arguments": args}))).await.unwrap()
        }
    };
    let r = list(json!({})).await;
    let out = text(&r);
    assert!(
        out.contains(&format!("{me} · Me")) && out.lines().any(|l| l.contains("Me") && l.ends_with("(you)")),
        "{out}"
    );
    assert!(out.contains(sibling.as_str()) && !out.contains("Elsewhere") && !out.contains("Gone"), "{out}");

    let r = list(json!({"include_archived": true})).await;
    assert!(text(&r).contains("Gone") && text(&r).contains("archived"));
    let r = list(json!({"project": other.as_str()})).await;
    assert!(text(&r).contains("Elsewhere") && !text(&r).contains("(you)"));
    let r = list(json!({"status": "running"})).await;
    assert_eq!(text(&r), "No sessions match.");
    let r = list(json!({"status": "sleepy"})).await;
    assert_eq!(r["error"]["code"], INVALID_PARAMS);
    let r = list(json!({"project": "nope"})).await;
    assert_eq!(r["result"]["isError"], true);
}

#[tokio::test]
async fn read_tool_returns_the_other_sessions_text() {
    let (app, _t) = app().await;
    let p = project(&app, "mine").await;
    let (me, other) = (session(&app, &p, "Me").await, session(&app, &p, "Other").await);
    let say = UserMessage { text: "why is the WAL growing?".into() };
    app.store.append_one(NewEvent::new(stream::session(&p, &other), Actor::User, &say)).await.unwrap();
    let r = handle(&app, &me, call(1, "tools/call", json!({"name": "read", "arguments": {"session": other.as_str()}})))
        .await
        .unwrap();
    let out = text(&r);
    assert!(
        out.starts_with(&format!("Session {other} · Other\n")) && out.contains(" user: why is the WAL growing?"),
        "{out}"
    );
}

#[test]
fn the_appended_prompt_is_short_and_says_who_you_are() {
    let id = SessionId::generate();
    let long = format!("A \"quoted\"\ntitle {}", "x".repeat(300));
    for title in ["Fix login", long.as_str(), ""] {
        let p = system_prompt(&id, title);
        assert!(p.chars().count() < 600, "{} chars", p.chars().count());
        assert!(p.contains(id.as_str()) && p.contains("`sessions`") && p.contains("`read`") && p.contains("atelier"));
        assert!(!p.contains('\n'));
    }
    assert!(system_prompt(&id, "Fix login").contains("(\"Fix login\")"));
}
