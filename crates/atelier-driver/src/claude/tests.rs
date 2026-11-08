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
        })
        .unwrap();
    h.cmds.send(AgentCmd::Send("hello".into())).unwrap();

    let mut kinds = Vec::new();
    while let Some(ev) = h.events.recv().await {
        match ev {
            DriverEvent::Permission(p) => {
                assert_eq!((p.tool.as_str(), p.input["command"].as_str()), ("Bash", Some("ls")));
                kinds.push("permission".to_owned());
                let decision = PermissionDecision::Allow { updated_input: None };
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
