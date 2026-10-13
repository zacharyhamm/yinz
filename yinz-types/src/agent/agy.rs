//! Reading Antigravity's (`agy`) hook JSON as the hook shape the rest of yinz reads.
//!
//! Antigravity (`agy`) lifecycle hooks are configured via `hooks.json` in
//! `~/.gemini/config/hooks.json` (or `<workspace>/.agents/hooks.json`).
//! The hook script forwards events to `yinz action agent-report --hook-json --kind agy`,
//! and [`to_hook_event`] turns one forwarded payload into a [`HookEvent`], so the record,
//! the broadcast, and the run history read `agy` identically to Claude Code, Codex, and OpenCode.

use super::HookEvent;
use serde_json::Value;

/// The Antigravity hook events the generated hook script handles.
pub const AGY_HOOK_EVENTS: &[&str] = &[
    "PreInvocation",
    "PreToolUse",
    "PostToolUse",
    "PostInvocation",
    "Stop",
];

/// Timeout in seconds for Antigravity hooks.
pub const AGY_HOOK_TIMEOUT_SECONDS: u64 = 5;

/// Translate one forwarded Antigravity event into the hook shape the session reads.
pub fn to_hook_event(event: &Value) -> HookEvent {
    let event_name = event
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let session_id = event
        .get("conversationId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_default();

    let cwd = event
        .get("workspacePaths")
        .and_then(Value::as_array)
        .and_then(|paths| paths.first())
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_owned);

    let transcript_path = event
        .get("transcriptPath")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);

    let tool_call = event.get("toolCall");
    let tool_name = tool_call
        .and_then(|tc| tc.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tool_input = tool_call.and_then(|tc| tc.get("args")).cloned();

    let error = event
        .get("error")
        .and_then(Value::as_str)
        .filter(|e| !e.is_empty())
        .map(str::to_owned);

    let step_idx = event
        .get("initialNumSteps")
        .or_else(|| event.get("stepIdx"))
        .and_then(Value::as_u64);
    let invocation_num = event.get("invocationNum").and_then(Value::as_u64);
    let agy_turn_complete = event
        .get("_yinz_agy_turn_complete")
        .and_then(Value::as_bool);

    let mut hook = HookEvent {
        session_id: session_id.clone(),
        cwd,
        transcript_path,
        tool_name: tool_name.clone(),
        tool_input,
        message: error.clone(),
        error: error.clone().map(Value::String),
        tool_use_id: matches!(event_name, "PreToolUse" | "PostToolUse")
            .then(|| event.get("stepIdx").and_then(Value::as_u64))
            .flatten()
            .map(|idx| format!("step:{idx}")),
        turn_id: step_idx.map(|idx| idx.to_string()),
        agy_turn_complete,
        ..Default::default()
    };

    match event_name {
        "PreInvocation" => {
            if invocation_num.unwrap_or(0) == 0 {
                hook.hook_event_name = "UserPromptSubmit".to_owned();
                hook.prompt_id = step_idx.map(|idx| format!("{session_id}:{idx}"));
            } else {
                hook.hook_event_name = "PreInvocation".to_owned();
            }
        },
        "PreToolUse" => {
            hook.hook_event_name = "PreToolUse".to_owned();
            if tool_name.as_deref() == Some("ask_question") {
                hook.permission_mode = Some("manual".to_owned());
            }
        },
        "PostToolUse" => {
            hook.hook_event_name = "PostToolUse".to_owned();
        },
        "PostInvocation" => {
            hook.hook_event_name = "PostInvocation".to_owned();
        },
        "Stop" => {
            let termination_reason = event.get("terminationReason").and_then(Value::as_str);
            let has_error = error.is_some()
                || matches!(termination_reason, Some("error" | "max_steps_exceeded"));
            if has_error {
                hook.hook_event_name = "StopFailure".to_owned();
            } else {
                // The raw Stop hook invocation is a pre-decision notification: later Stop
                // handlers may return `{"decision":"continue"}` to resume the execution loop.
                // hook_state_for and PromptObservation require completion confirmation
                // (_yinz_agy_turn_complete == true) before treating the turn as finished.
                hook.hook_event_name = "Stop".to_owned();
            }
        },
        other => {
            hook.hook_event_name = other.to_owned();
        },
    }

    hook
}

#[cfg(test)]
#[path = "unit/agy_tests.rs"]
mod tests;
