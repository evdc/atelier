use super::*;
use atelier_core::SessionId;

fn run(msg: Value) -> (Vec<DriverEvent>, Option<String>, Vec<String>) {
    let pending = Pending::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut known = Some("a".to_owned());
    let evs = classify(msg, &mut known, &pending, &tx);
    let mut replies = Vec::new();
    while let Ok(r) = rx.try_recv() {
        replies.push(r);
    }
    (evs, known, replies)
}

#[test]
fn deltas_are_ephemeral() {
    let (evs, ..) = run(json!({"type": "stream_event", "event": {"type": "content_block_delta"}}));
    assert!(matches!(&evs[..], [DriverEvent::Ephemeral { kind, body }]
        if kind == "cc.stream_event" && body.get().contains("content_block_delta")));
}

#[test]
fn init_binds_new_native_id_once() {
    let (evs, known, _) = run(json!({"type": "system", "subtype": "init", "session_id": "b"}));
    assert!(matches!(&evs[0], DriverEvent::Bound { native_id } if native_id == "b"));
    assert!(matches!(&evs[1], DriverEvent::Raw { kind, .. } if kind == "cc.system"));
    assert_eq!(known.as_deref(), Some("b"));

    let (evs, ..) = run(json!({"type": "system", "subtype": "init", "session_id": "a"}));
    assert_eq!(evs.len(), 1, "same id: no rebind");
}

#[test]
fn assistant_signatures_are_stripped() {
    let (evs, ..) = run(json!({"type": "assistant", "message": {"content": [
        {"type": "thinking", "thinking": "", "signature": "SIG"}, {"type": "text", "text": "x"}]}}));
    let DriverEvent::Raw { body, .. } = &evs[0] else { panic!() };
    assert!(!body.get().contains("SIG"));
}

#[test]
fn result_becomes_turn_ended() {
    let (evs, ..) = run(json!({"type": "result", "subtype": "error_during_execution", "is_error": false,
                               "total_cost_usd": 0.5}));
    let DriverEvent::TurnEnded(t) = &evs[0] else { panic!() };
    assert!(t.is_error);
    assert_eq!(t.cost_usd, Some(0.5));
}

#[test]
fn unknown_control_requests_are_refused_not_ignored() {
    let (evs, _, replies) =
        run(json!({"type": "control_request", "request_id": "x", "request": {"subtype": "hook_callback"}}));
    assert!(evs.is_empty());
    assert!(replies[0].contains("\"error\"") && replies[0].contains("\"x\""));
}

#[test]
fn unknown_types_are_kept() {
    let (evs, ..) = run(json!({"type": "brand_new_thing", "x": 1}));
    assert!(matches!(&evs[0], DriverEvent::Raw { kind, .. } if kind == "cc.brand_new_thing"));
}

#[tokio::test]
async fn drives_a_process_end_to_end() {
    let driver = ClaudeCode {
        bin: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake-claude.sh").into(),
        ..Default::default()
    };
    let mut h = driver
        .start(StartSpec {
            session: SessionId::generate(),
            cwd: std::env::temp_dir(),
            resume: Resume::New,
            model: None,
            read_only: false,
            mcp: None,
            system_append: None,
        })
        .unwrap();
    h.cmds.send(AgentCmd::Send { text: "hello".into(), images: vec![] }).unwrap();

    let mut kinds = Vec::new();
    while let Some(ev) = h.events.recv().await {
        match ev {
            DriverEvent::Permission(p) => {
                assert_eq!((p.tool.as_str(), p.input["command"].as_str()), ("Bash", Some("ls")));
                kinds.push("permission".to_owned());
                let decision = PermissionDecision::Allow { updated_input: None, updated_permissions: None };
                h.cmds.send(AgentCmd::Answer { request_id: p.request_id, decision }).unwrap();
            }
            DriverEvent::TurnEnded(t) => {
                // Allow without an explicit input echoes the original input back.
                assert_eq!(t.raw["echo"]["response"]["response"]["updatedInput"]["command"], "ls");
                kinds.push("turn".to_owned());
                h.cmds.send(AgentCmd::Shutdown).unwrap();
            }
            DriverEvent::Exited(x) => {
                assert_eq!(x.reason, ExitReason::Stopped);
                kinds.push("exited".to_owned());
            }
            DriverEvent::Raw { kind, .. } | DriverEvent::Ephemeral { kind, .. } => kinds.push(kind),
            DriverEvent::Bound { native_id } => kinds.push(format!("bound:{native_id}")),
        }
    }
    assert_eq!(
        kinds,
        ["bound:fake-native-1", "cc.system", "cc.stream_event", "cc.assistant", "permission", "turn", "exited"]
    );
}

#[test]
fn allow_forwards_updated_permissions_verbatim() {
    let suggestion = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test"}],
                            "behavior": "allow", "destination": "localSettings"});
    let line = permission_response(
        "r1",
        PermissionDecision::Allow { updated_input: None, updated_permissions: Some(json!([suggestion.clone()])) },
        Some(json!({"command": "npm test"})),
    );
    let v: Value = serde_json::from_str(&line).unwrap();
    let response = &v["response"]["response"];
    assert_eq!(response["behavior"], "allow");
    assert_eq!(response["updatedInput"]["command"], "npm test", "original input echoed when not replaced");
    assert_eq!(response["updatedPermissions"], json!([suggestion]));
}

#[test]
fn allow_without_suggestion_omits_updated_permissions() {
    let line =
        permission_response("r1", PermissionDecision::Allow { updated_input: None, updated_permissions: None }, None);
    let v: Value = serde_json::from_str(&line).unwrap();
    assert!(v["response"]["response"].get("updatedPermissions").is_none());
}

#[test]
fn deny_carries_the_reason_as_message() {
    let line = permission_response("r1", PermissionDecision::Deny { message: "use cargo test".into() }, None);
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["response"]["response"]["behavior"], "deny");
    assert_eq!(v["response"]["response"]["message"], "use cargo test");
}

fn args_of(spec: &StartSpec) -> Vec<String> {
    let (cmd, _) = ClaudeCode::default().command(spec);
    cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect()
}

fn spec() -> StartSpec {
    StartSpec {
        session: SessionId::generate(),
        cwd: std::env::temp_dir(),
        resume: Resume::New,
        model: None,
        read_only: false,
        mcp: None,
        system_append: None,
    }
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str())
}

#[test]
fn no_mcp_means_no_mcp_flags() {
    let args = args_of(&spec());
    for flag in ["--mcp-config", "--allowedTools", "--append-system-prompt"] {
        assert!(!args.iter().any(|a| a == flag), "{flag}");
    }
}

#[test]
fn mcp_config_carries_url_token_and_allowed_tools() {
    let args = args_of(&StartSpec {
        mcp: Some(crate::McpEndpoint { url: "http://127.0.0.1:1/mcp".into(), token: "tok".into() }),
        system_append: Some("You are s1.".into()),
        ..spec()
    });
    let cfg: Value = serde_json::from_str(after(&args, "--mcp-config").unwrap()).unwrap();
    let server = &cfg["mcpServers"]["atelier"];
    assert_eq!(server["type"], "http");
    assert_eq!(server["url"], "http://127.0.0.1:1/mcp");
    assert_eq!(server["headers"]["Authorization"], "Bearer tok");
    assert_eq!(after(&args, "--allowedTools"), Some("mcp__atelier__sessions,mcp__atelier__read,mcp__atelier__send"));
    assert_eq!(after(&args, "--append-system-prompt"), Some("You are s1."));
}

#[test]
fn read_only_note_and_system_append_share_one_flag() {
    let args = args_of(&StartSpec { read_only: true, system_append: Some("You are s1.".into()), ..spec() });
    assert_eq!(args.iter().filter(|a| *a == "--append-system-prompt").count(), 1);
    let text = after(&args, "--append-system-prompt").unwrap();
    assert!(text.starts_with("You are in another session's worktree") && text.ends_with("\n\nYou are s1."), "{text}");
}

#[test]
fn user_content_is_a_string_until_there_are_pictures() {
    use atelier_core::domain::Image;
    assert_eq!(user_content("hi".into(), &[]), json!("hi"));
    let img = Image { media_type: "image/png".into(), data: "AAAA".into() };
    assert_eq!(
        user_content("look".into(), std::slice::from_ref(&img)),
        json!([
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } },
            { "type": "text", "text": "look" },
        ])
    );
    // The API refuses empty text blocks; a picture alone is a whole message.
    assert_eq!(user_content(" ".into(), &[img]).as_array().unwrap().len(), 1);
}
