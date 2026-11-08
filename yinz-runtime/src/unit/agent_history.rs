use super::*;
use serde_json::json;
fn record(session: &str, worker: &str, source: &str, text: &str) -> Value {
    json!({"sessionId":session,"agentId":worker,"isSidechain":true,"uuid":source,"type":"assistant","timestamp":"2026-11-07T17:44:28.234Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"private reasoning"},{"type":"tool_use","input":{"command":"private tool payload"}},{"type":"text","text":text}]}})
}
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("session.jsonl");
    std::fs::write(&parent, "{}").unwrap();
    let worker = tmp
        .path()
        .join("session/subagents/workflows/wf-one/agent-worker.jsonl");
    std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
    (tmp, parent, worker)
}
#[test]
fn worker_recovery_verifies_nested_workflow_records_and_keeps_only_assistant_text() {
    let (_tmp, parent, worker) = fixture();
    let lines = [
        json!({"sessionId":"session","agentId":"worker","isSidechain":true,"uuid":"prompt","type":"user","message":{"content":"task"}}),
        record("session", "worker", "one", "first reply"),
        record("other-session", "worker", "wrong", "other session"),
        record("session", "wrong-worker", "wrong2", "other worker"),
        record("session", "worker", "two", "second reply"),
    ];
    std::fs::write(
        &worker,
        lines
            .iter()
            .map(|r| r.to_string() + "\n")
            .collect::<String>()
            + "{partial",
    )
    .unwrap();
    let recovered = recover(&parent, "session", None);
    assert_eq!(recovered.files, 1);
    assert_eq!(recovered.messages.len(), 2);
    assert_eq!(recovered.messages[0].text, "first reply");
    assert_eq!(recovered.messages[0].worker_id.as_deref(), Some("worker"));
    assert_eq!(recovered.messages[0].turn_id, "prompt");
    assert_eq!(recovered.messages[0].created_at_ms, Some(1794073468234));
    assert_eq!(
        serde_json::to_value(&recovered.messages).unwrap(),
        serde_json::to_value(recover(&parent, "session", None).messages).unwrap()
    );
    assert!(recover(&parent, "wrong-session", None).messages.is_empty());
    assert!(recover(&parent, "session", Some("other"))
        .messages
        .is_empty());
}
#[test]
fn worker_recovery_marks_bounded_tails_and_tolerates_missing_transcripts() {
    use std::io::Write;
    let (_tmp, parent, worker) = fixture();
    let mut file = std::fs::File::create(&worker).unwrap();
    file.set_len(FILE_BYTES + 100).unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    writeln!(
        file,
        "\n{}",
        record("session", "worker", "one", "retained tail")
    )
    .unwrap();
    let recovered = recover(&parent, "session", None);
    assert_eq!(recovered.messages.len(), 1);
    assert!(recovered.messages[0].incomplete);
    assert!(!recovered.diagnostics.is_empty());
    std::fs::remove_file(worker).unwrap();
    assert!(recover(&parent, "session", None).messages.is_empty());
}
#[test]
fn claude_native_stop_and_observer_use_the_same_prompt_identity() {
    let (_tmp, parent, _worker) = fixture();
    std::fs::write(&parent,json!({"sessionId":"session","isSidechain":false,"type":"user","uuid":"prompt","message":{"content":"task"}}).to_string()+"\n").unwrap();
    let mut native = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "session".into(),
        transcript_path: Some(parent.display().to_string()),
        ..Default::default()
    };
    enrich(&mut native);
    assert_eq!(native.turn_id.as_deref(), Some("prompt"));
    let mut observer = HookEvent {
        prompt_id: Some("prompt".into()),
        ..native.clone()
    };
    observer.turn_id = None;
    enrich(&mut observer);
    assert_eq!(native.turn_id, observer.turn_id);
}

#[test]
fn native_stop_follows_reply_ancestry_past_a_queued_prompt() {
    let (_tmp, parent, _worker) = fixture();
    let rows = [
        json!({"sessionId":"session","type":"user","uuid":"user-one","promptId":"prompt-one","message":{"content":"first"}}),
        json!({"sessionId":"session","type":"assistant","uuid":"reply-one","parentUuid":"user-one","message":{"content":[{"type":"text","text":"answer"}]}}),
        json!({"sessionId":"session","type":"user","uuid":"user-two","promptId":"prompt-two","message":{"content":"queued"}}),
    ];
    std::fs::write(
        &parent,
        rows.iter()
            .map(|r| r.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let mut stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "session".into(),
        transcript_path: Some(parent.display().to_string()),
        last_assistant_message: Some("answer".into()),
        ..Default::default()
    };
    enrich(&mut stop);
    assert_eq!(stop.turn_id.as_deref(), Some("prompt-one"));
    assert_eq!(stop.prompt_id.as_deref(), Some("prompt-one"));
    stop.turn_id = None;
    stop.prompt_id = None;
    let (parsed, raw) =
        crate::agent::hooks::parse_hook_event("claude", serde_json::to_string(&stop).unwrap())
            .unwrap();
    assert_eq!(parsed.prompt_id.as_deref(), Some("prompt-one"));
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap()["prompt_id"],
        "prompt-one"
    );
}
