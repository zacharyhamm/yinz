use super::*;
use crate::agent::{hook_state, AgentState};

fn event(json: &str) -> HookEvent {
    to_hook_event(&serde_json::from_str(json).expect("the test's own event parses"))
}

#[test]
fn a_session_lifecycle_reports_the_states_the_reader_knows() {
    let started =
        event(r#"{"type":"session.created","properties":{"sessionID":"ses_1","cwd":"/work"}}"#);
    assert_eq!(started.hook_event_name, "SessionStart");
    assert_eq!(started.source.as_deref(), Some("startup"));
    assert_eq!(started.session_id, "ses_1");
    assert_eq!(started.cwd.as_deref(), Some("/work"));
    assert_eq!(hook_state(&started), Some(AgentState::Idle));

    let idle = event(
        r#"{"type":"session.idle","properties":{"sessionID":"ses_1",
                "lastAssistantMessage":"Ran the tests; they pass.","turnID":"msg_9"}}"#,
    );
    assert_eq!(idle.hook_event_name, "Stop");
    assert_eq!(
        idle.last_assistant_message.as_deref(),
        Some("Ran the tests; they pass.")
    );
    assert_eq!(idle.turn_id.as_deref(), Some("msg_9"));
    assert_eq!(hook_state(&idle), Some(AgentState::Idle));

    assert_eq!(
        hook_state(&event(
            r#"{"type":"session.error","properties":{"sessionID":"ses_1"}}"#
        )),
        Some(AgentState::Idle),
        "an errored turn ended"
    );
    assert_eq!(
        hook_state(&event(
            r#"{"type":"session.compacted","properties":{"sessionID":"ses_1"}}"#
        )),
        Some(AgentState::Busy)
    );
    assert_eq!(
        to_hook_event(&serde_json::json!({"type":"session.deleted","properties":{}}))
            .hook_event_name,
        "SessionEnd"
    );
}

#[test]
fn a_prompt_names_the_turn_that_is_about_to_start() {
    let event = event(
        r#"{"type":"chat.message","properties":{"sessionID":"ses_1",
                "messageID":"msg_1","prompt":"fix the parser"}}"#,
    );
    assert_eq!(event.hook_event_name, "UserPromptSubmit");
    assert_eq!(event.prompt.as_deref(), Some("fix the parser"));
    assert_eq!(hook_state(&event), Some(AgentState::Busy));
}

#[test]
fn a_tool_is_named_and_its_arguments_spelled_the_way_the_readers_read_them() {
    let before = event(
        r#"{"type":"tool.execute.before","properties":{"sessionID":"ses_1",
                "callID":"call_1","tool":"bash","args":{"command":"cargo build --release"}}}"#,
    );
    assert_eq!(before.hook_event_name, "PreToolUse");
    assert_eq!(before.tool_name.as_deref(), Some("Bash"));
    assert_eq!(before.tool_use_id.as_deref(), Some("call_1"));
    assert_eq!(hook_state(&before), Some(AgentState::Busy));
    assert_eq!(
        crate::agent::hook_detail(&before).as_deref(),
        Some("Bash cargo")
    );

    let read = event(
        r#"{"type":"tool.execute.before","properties":{"sessionID":"ses_1",
                "callID":"call_2","tool":"read","args":{"filePath":"/work/src/main.rs"}}}"#,
    );
    assert_eq!(read.tool_name.as_deref(), Some("Read"));
    assert_eq!(
        read.tool_input
            .as_ref()
            .and_then(|input| input.get("file_path"))
            .and_then(Value::as_str),
        Some("/work/src/main.rs")
    );
    assert_eq!(
        crate::agent::hook_detail(&read).as_deref(),
        Some("Read main.rs")
    );

    let after = event(
        r#"{"type":"tool.execute.after","properties":{"sessionID":"ses_1",
                "callID":"call_1","tool":"bash","args":{"command":"cargo build"}}}"#,
    );
    assert_eq!(after.hook_event_name, "PostToolUse");
    assert_eq!(
        crate::agent::hook_detail(&after),
        None,
        "a finished tool clears the hint"
    );
}

#[test]
fn opencodes_question_tool_is_the_wait_ask_user_question_names() {
    let ask = event(
        r#"{"type":"tool.execute.before","properties":{"sessionID":"ses_1",
                "callID":"call_3","tool":"question","args":{"questions":[
                    {"header":"Library","question":"Which library should we use?"}]}}}"#,
    );
    assert_eq!(ask.tool_name.as_deref(), Some("AskUserQuestion"));
    assert_eq!(hook_state(&ask), Some(AgentState::Waiting));
    assert_eq!(
        crate::agent::waiting_detail(&ask).as_deref(),
        Some("Which library should we use?")
    );
}

#[test]
fn a_permission_prompt_waits_and_names_what_it_is_about() {
    let bash = event(
        r#"{"type":"permission.updated","properties":{"sessionID":"ses_1","callID":"call_4",
                "type":"bash","title":"rm -rf target","metadata":{}}}"#,
    );
    assert_eq!(bash.hook_event_name, "PermissionRequest");
    assert_eq!(bash.tool_use_id.as_deref(), Some("call_4"));
    assert_eq!(bash.tool_name.as_deref(), Some("Bash"));
    assert_eq!(hook_state(&bash), Some(AgentState::Waiting));
    assert_eq!(
        crate::agent::waiting_detail(&bash).as_deref(),
        Some("Bash rm")
    );

    let file = event(
        r#"{"type":"permission.updated","properties":{"sessionID":"ses_1",
                "type":"edit","title":"/work/src/main.rs"}}"#,
    );
    assert_eq!(file.tool_name.as_deref(), Some("Edit"));
    assert_eq!(
        file.tool_input
            .as_ref()
            .and_then(|input| input.get("file_path"))
            .and_then(Value::as_str),
        Some("/work/src/main.rs")
    );

    let replied = event(
        r#"{"type":"permission.replied","properties":{"sessionID":"ses_1",
                "permissionID":"per_1","response":"once"}}"#,
    );
    assert_eq!(replied.hook_event_name, "PostToolUse");
    assert_eq!(hook_state(&replied), Some(AgentState::Busy));
}

#[test]
fn current_opencode_permissions_use_call_identity_metadata_and_pattern_fallback() {
    for (permission, metadata, patterns, input, detail) in [
        (
            "bash",
            serde_json::json!({"command": "cargo test", "extra": true}),
            vec!["ignored *"],
            serde_json::json!({"command": "cargo test", "extra": true}),
            "Bash cargo",
        ),
        (
            "edit",
            serde_json::json!({"filePath": "/work/main.rs", "extra": true}),
            vec!["ignored.rs"],
            serde_json::json!({"file_path": "/work/main.rs", "extra": true}),
            "Edit main.rs",
        ),
        (
            "bash",
            serde_json::json!({}),
            vec!["git status", "git diff"],
            serde_json::json!({"command": "git status git diff"}),
            "Bash git",
        ),
        (
            "read",
            serde_json::json!({"unrelated": true}),
            vec!["/work/main.rs"],
            serde_json::json!({"file_path": "/work/main.rs"}),
            "Read main.rs",
        ),
    ] {
        let hook = to_hook_event(&serde_json::json!({
            "type": "permission.asked",
            "properties": {
                "sessionID": "ses_1", "permission": permission,
                "tool": {"messageID": "msg_1", "callID": "call_current"},
                "metadata": metadata, "patterns": patterns,
            },
        }));
        assert_eq!(hook.hook_event_name, "PermissionRequest");
        assert_eq!(hook.session_id, "ses_1");
        assert_eq!(hook.tool_use_id.as_deref(), Some("call_current"));
        assert_eq!(hook.tool_input, Some(input));
        assert_eq!(hook_state(&hook), Some(AgentState::Waiting));
        assert_eq!(crate::agent::waiting_detail(&hook).as_deref(), Some(detail));
    }
    let legacy = event(
        r#"{"type":"permission.updated","properties":{"type":"bash","callID":"legacy",
                "title":"ignored","metadata":{"command":"cargo test","extra":true}}}"#,
    );
    assert_eq!(legacy.tool_use_id.as_deref(), Some("legacy"));
    assert_eq!(
        legacy.tool_input,
        Some(serde_json::json!({"command":"cargo test","extra":true}))
    );
    assert_eq!(
        crate::agent::waiting_detail(&legacy).as_deref(),
        Some("Bash cargo")
    );
}

#[test]
fn a_completed_assistant_message_is_one_display_batch() {
    let message = event(
        r#"{"type":"assistant.message","properties":{"sessionID":"ses_1",
                "messageID":"msg_5","text":"Parser verified with Unicode"}}"#,
    );
    assert_eq!(message.hook_event_name, "MessageDisplay");
    assert_eq!(message.message_id.as_deref(), Some("msg_5"));
    assert_eq!(message.turn_id.as_deref(), Some("msg_5"));
    assert_eq!(message.index, Some(0));
    assert_eq!(message.final_batch, Some(true));
    assert_eq!(
        message.delta.as_deref(),
        Some("Parser verified with Unicode")
    );
    assert_eq!(
        hook_state(&message),
        None,
        "a display leaves the record alone"
    );
}

/// A type this module has not been taught is reported as itself and changes no
/// state, rather than being dropped: the broadcast is where a plugin reads what a
/// record has no room for.
#[test]
fn an_unknown_event_reports_nothing_and_still_names_itself() {
    let unknown = to_hook_event(&serde_json::json!({
        "type": "session.status",
        "properties": { "sessionID": "ses_1", "status": { "type": "busy" } },
    }));
    assert_eq!(unknown.hook_event_name, "session.status");
    assert_eq!(hook_state(&unknown), None);
}

#[test]
fn the_forwarded_events_are_the_ones_the_installer_wires() {
    // The plugin's `switch` has one arm per entry, and this is what keeps the two in
    // step when a later opencode grows a signal worth reporting.
    for wired in OPENCODE_HOOK_EVENTS {
        assert!(!wired.is_empty(), "{wired}");
    }
    assert!(OPENCODE_HOOK_EVENTS.contains(&"session.idle"));
    assert!(OPENCODE_HOOK_EVENTS.contains(&"tool.execute.before"));
    assert!(OPENCODE_HOOK_EVENTS.contains(&"permission.asked"));
}
