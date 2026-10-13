use super::*;
use crate::agent::hooks::{hook_state, waiting_detail};
use serde_json::json;

#[test]
fn translates_pre_invocation_turn_start() {
    let payload = json!({
        "hook_event_name": "PreInvocation",
        "conversationId": "conv-1234",
        "workspacePaths": ["/workspace/my-repo"],
        "transcriptPath": "/tmp/transcript.jsonl",
        "invocationNum": 0,
        "stepIdx": 5,
    });
    let event = to_hook_event(&payload);
    assert_eq!(event.hook_event_name, "UserPromptSubmit");
    assert_eq!(event.session_id, "conv-1234");
    assert_eq!(event.cwd.as_deref(), Some("/workspace/my-repo"));
    assert_eq!(
        event.transcript_path.as_deref(),
        Some("/tmp/transcript.jsonl")
    );
    assert_eq!(event.prompt_id.as_deref(), Some("conv-1234:5"));
    assert_eq!(event.turn_id.as_deref(), Some("5"));
}

#[test]
fn translates_pre_invocation_continuation() {
    let payload = json!({
        "hook_event_name": "PreInvocation",
        "conversationId": "conv-1234",
        "workspacePaths": ["/workspace/my-repo"],
        "invocationNum": 1,
        "stepIdx": 7,
    });
    let event = to_hook_event(&payload);
    assert_eq!(event.hook_event_name, "PreInvocation");
    assert_eq!(event.session_id, "conv-1234");
}

#[test]
fn translates_pre_tool_use_and_waiting_tools() {
    let bash_payload = json!({
        "hook_event_name": "PreToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "run_command",
            "args": { "CommandLine": "cargo test" }
        }
    });
    let event = to_hook_event(&bash_payload);
    assert_eq!(event.hook_event_name, "PreToolUse");
    assert_eq!(event.tool_name.as_deref(), Some("run_command"));
    assert_eq!(
        event.tool_input.as_ref().unwrap()["CommandLine"],
        "cargo test"
    );
    assert_eq!(hook_state(&event), Some(crate::agent::AgentState::Busy));

    let question_payload = json!({
        "hook_event_name": "PreToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "ask_question",
            "args": {
                "questions": [
                    { "question": "Proceed with refactor?" }
                ]
            }
        }
    });
    let event = to_hook_event(&question_payload);
    assert_eq!(event.hook_event_name, "PreToolUse");
    assert_eq!(event.tool_name.as_deref(), Some("ask_question"));
    assert_eq!(hook_state(&event), Some(crate::agent::AgentState::Waiting));
    assert_eq!(
        waiting_detail(&event).as_deref(),
        Some("Proceed with refactor?")
    );
}

#[test]
fn translates_post_tool_use() {
    let payload = json!({
        "hook_event_name": "PostToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "run_command",
            "args": { "CommandLine": "cargo check" }
        },
        "error": "exit status 1"
    });
    let event = to_hook_event(&payload);
    assert_eq!(event.hook_event_name, "PostToolUse");
    assert_eq!(event.message.as_deref(), Some("exit status 1"));
}

#[test]
fn translates_stop_success_and_failure() {
    let ok_payload = json!({
        "hook_event_name": "Stop",
        "conversationId": "conv-1234",
        "terminationReason": "model_stop",
        "error": ""
    });
    let event = to_hook_event(&ok_payload);
    assert_eq!(event.hook_event_name, "Stop");
    assert_eq!(event.agy_turn_complete, None);
    assert_eq!(
        crate::agent::hooks::hook_state_for("agy", &event),
        Some(crate::agent::AgentState::Busy)
    );

    let confirmed_payload = json!({
        "hook_event_name": "Stop",
        "conversationId": "conv-1234",
        "terminationReason": "model_stop",
        "error": "",
        "_yinz_agy_turn_complete": true
    });
    let confirmed_event = to_hook_event(&confirmed_payload);
    assert_eq!(confirmed_event.hook_event_name, "Stop");
    assert_eq!(confirmed_event.agy_turn_complete, Some(true));
    assert_eq!(
        crate::agent::hooks::hook_state_for("agy", &confirmed_event),
        Some(crate::agent::AgentState::Idle)
    );

    let err_payload = json!({
        "hook_event_name": "Stop",
        "conversationId": "conv-1234",
        "terminationReason": "error",
        "error": "context deadline exceeded"
    });
    let event = to_hook_event(&err_payload);
    assert_eq!(event.hook_event_name, "StopFailure");
    assert_eq!(event.message.as_deref(), Some("context deadline exceeded"));
    assert_eq!(
        crate::agent::hooks::hook_state_for("agy", &event),
        Some(crate::agent::AgentState::Idle)
    );
}
