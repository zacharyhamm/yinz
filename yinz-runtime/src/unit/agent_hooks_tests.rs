use super::*;
use serde_json::json;
use std::io::Write;
use yinz_types::{agent::AgentState, input::actions::hook_report_actions, input::actions::Action};

const META: &str = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\"}}\n";
/// Prose that cannot fit the broadcast ceiling wherever the ceiling moves: `雪` is three bytes
/// in UTF-8 and unescaped in JSON, so half the ceiling in characters is half again over it in
/// bytes. Sized off the constant so raising the ceiling cannot quietly stop these tests from
/// exercising a prune.
const OVERSIZE_CHARS: usize = yinz_types::agent::HOOK_BROADCAST_MAX_BYTES / 2;

const AUTO: &str = "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn\",\"approvals_reviewer\":\"auto_review\"}}\n";

fn fixture(contents: &str) -> (tempfile::NamedTempFile, Value) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    let hook = json!({
        "hook_event_name": "PermissionRequest", "session_id": "session", "turn_id": "turn",
        "transcript_path": file.path(), "permission_mode": "default",
        "tool_name": "Bash", "tool_input": {"command": "git status"}, "future_field": [1, 2]
    });
    (file, hook)
}

#[test]
fn codex_permission_uses_the_newest_matching_turn_and_preserves_the_payload() {
    for reviewer in ["auto_review", "user", "future_reviewer"] {
        let contents = format!(
            "{META}{AUTO}{}{}",
            AUTO.replace("auto_review", reviewer),
            AUTO.replace("turn\"", "other-turn\"")
        );
        let (_file, hook) = fixture(&contents);
        let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
        assert_eq!(event.approvals_reviewer.as_deref(), Some(reviewer));
        let mut expected = hook;
        expected["approvals_reviewer"] = reviewer.into();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), expected);
        assert_eq!(
            yinz_types::agent::waiting_needs_a_person(&event),
            reviewer != "auto_review"
        );
    }
}

#[test]
fn codex_permission_finds_the_newest_turn_context_before_the_message_tail() {
    let line = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"progress\"}}\n";
    let padding = line.repeat(TAIL_BYTES as usize / line.len() + 2);
    for reviewer in [Some("auto_review"), Some("user"), None] {
        let latest = json!({"type":"turn_context", "payload":{
            "turn_id":"turn", "approvals_reviewer": reviewer,
        }});
        let (_file, mut hook) = fixture(&format!("{META}{AUTO}{latest}\n{padding}"));
        hook.as_object_mut().unwrap().remove("permission_mode");
        let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
        assert_eq!(event.approvals_reviewer.as_deref(), reviewer);
        assert_eq!(
            yinz_types::agent::waiting_needs_a_person(&event),
            reviewer != Some("auto_review"),
        );
        let broadcast: HookEvent = serde_json::from_str(&raw).unwrap();
        assert_eq!(broadcast.approvals_reviewer, event.approvals_reviewer);
        assert!(
            event.messages_omitted,
            "reviewer lookup must not expand the message tail"
        );
    }
}

#[test]
fn codex_permission_without_a_mode_uses_verified_reviewer_evidence() {
    let (_file, mut hook) = fixture(&format!("{META}{AUTO}"));
    hook.as_object_mut().unwrap().remove("permission_mode");
    let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
    let actions = hook_report_actions(
        "codex",
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
        &event,
        raw,
    );
    let [Action::SetPaneAgent {
        agent: Some(agent), ..
    }, Action::AgentEvent(broadcast)] = &actions[..]
    else {
        panic!("{actions:?}")
    };
    assert_eq!(agent.state, AgentState::Busy);
    assert!(!agent.wait_needs_a_person);
    assert_eq!(agent.detail.as_deref(), Some("Bash git"));
    assert_eq!(broadcast.state, Some(AgentState::Busy));
    // Missing evidence is not proof of automatic approval. Preserve a possible real dialog.
    hook.as_object_mut().unwrap().remove("transcript_path");
    let (event, _) = parse_hook_event("codex", hook.to_string()).unwrap();
    assert_eq!(
        yinz_types::agent::hook_state_for("codex", &event),
        Some(AgentState::Waiting)
    );
    assert!(yinz_types::agent::waiting_needs_a_person(&event));
}

#[test]
fn codex_permission_falls_back_when_transcript_identity_or_context_is_missing() {
    for contents in [
        String::new(),
        META.to_owned(),
        format!("{}{AUTO}", META.replace("session\"", "other\"")),
        format!("{META}{}", AUTO.replace("turn\"", "other\"")),
        format!("{META}{}", AUTO.trim_end()),
        format!("{META}{AUTO}{{\"type\":\"turn_context\""),
        format!("{META}{AUTO}{{broken}}\n"),
        format!("{META}{AUTO}{{\"type\":\"turn_context\",\"payload\":{{\"turn_id\":\"turn\"}}}}\n"),
        format!("{META}{}", AUTO.replace("\"auto_review\"", "null")),
    ] {
        let (_file, hook) = fixture(&contents);
        let raw = hook.to_string();
        let (event, unchanged) = parse_hook_event("codex", raw.clone()).unwrap();
        assert_eq!(event.approvals_reviewer, None, "{contents}");
        assert_eq!(unchanged, raw);
    }
    let (file, hook) = fixture(&format!("{META}{AUTO}"));
    for key in ["session_id", "turn_id", "transcript_path"] {
        for missing in [Value::Null, json!("")] {
            let mut incomplete = hook.clone();
            if missing.is_null() {
                incomplete.as_object_mut().unwrap().remove(key);
            } else {
                incomplete[key] = missing;
            }
            assert_eq!(
                parse_hook_event("codex", incomplete.to_string())
                    .unwrap()
                    .0
                    .approvals_reviewer,
                None
            );
        }
    }
    drop(file);
    assert_eq!(
        parse_hook_event("codex", hook.to_string())
            .unwrap()
            .0
            .approvals_reviewer,
        None
    );
    let dir = tempfile::tempdir().unwrap();
    let mut hook = hook;
    hook["transcript_path"] = json!(dir.path());
    assert_eq!(
        parse_hook_event("codex", hook.to_string())
            .unwrap()
            .0
            .approvals_reviewer,
        None
    );
}

#[test]
fn codex_permission_only_reads_bounded_complete_records() {
    let padding =
        json!({"type": "response_item", "payload": "x".repeat(TAIL_BYTES as usize)}).to_string();
    let oversized_metadata = json!({
        "type": "session_meta", "payload": {"id": "session", "instructions": "x".repeat(METADATA_BYTES as usize)}
    });
    for (contents, expected) in [
        (format!("{META}{AUTO}{padding}\n"), None),
        (format!("{META}{padding}\n{AUTO}"), Some("auto_review")),
        (format!("{padding}\n{META}{AUTO}"), None),
        (format!("{oversized_metadata}\n{AUTO}"), None),
    ] {
        let (_file, hook) = fixture(&contents);
        assert_eq!(
            parse_hook_event("codex", hook.to_string())
                .unwrap()
                .0
                .approvals_reviewer
                .as_deref(),
            expected
        );
    }
}

#[test]
fn codex_permission_enrichment_does_not_change_other_events_or_explicit_reviewers() {
    let (_file, hook) = fixture(&format!("{META}{AUTO}"));
    for (kind, name, reviewer) in [
        ("claude", "PermissionRequest", Value::Null),
        ("codex", "PreToolUse", Value::Null),
        ("codex", "Elicitation", Value::Null),
        ("codex", "PermissionRequest", json!("user")),
    ] {
        let mut hook = hook.clone();
        hook["hook_event_name"] = name.into();
        hook["approvals_reviewer"] = reviewer;
        let raw = hook.to_string();
        let (_, enriched) = parse_hook_event(kind, raw.clone()).unwrap();
        let mut enriched: Value = serde_json::from_str(&enriched).unwrap();
        enriched.as_object_mut().unwrap().remove("tool_capture");
        assert_eq!(enriched, serde_json::from_str::<Value>(&raw).unwrap());
    }
}

#[test]
fn codex_permission_state_and_broadcast_agree_even_after_pruning() {
    let (_file, hook) = fixture(&format!("{META}{AUTO}"));
    for size in [0, 128 * 1024] {
        let mut hook = hook.clone();
        hook["tool_input"]["content"] = "x".repeat(size).into();
        let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
        let actions = hook_report_actions(
            "codex",
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            &event,
            raw,
        );
        let [Action::SetPaneAgent {
            agent: Some(agent), ..
        }, Action::AgentEvent(broadcast)] = &actions[..]
        else {
            panic!("{actions:?}")
        };
        assert_eq!(agent.state, AgentState::Busy);
        assert_eq!(agent.waiting_on, None);
        assert!(!agent.wait_needs_a_person);
        assert_eq!(broadcast.state, Some(AgentState::Busy));
        assert!(broadcast.raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        let received: HookEvent = serde_json::from_str(&broadcast.raw).unwrap();
        assert_eq!(received.approvals_reviewer.as_deref(), Some("auto_review"));
        assert_eq!(
            yinz_types::agent::hook_state(&received),
            Some(AgentState::Busy)
        );
    }
    for (name, tool) in [
        ("PreToolUse", "request_user_input"),
        ("Elicitation", "Bash"),
    ] {
        let mut hook = hook.clone();
        hook["hook_event_name"] = name.into();
        hook["tool_name"] = tool.into();
        hook["approvals_reviewer"] = "auto_review".into();
        let (event, _) = parse_hook_event("codex", hook.to_string()).unwrap();
        assert_eq!(
            yinz_types::agent::hook_state(&event),
            Some(AgentState::Waiting)
        );
        assert!(yinz_types::agent::waiting_needs_a_person(&event));
    }
}

fn assistant(id: Option<&str>, phase: &str, text: &str) -> String {
    format!(
        "{}\n",
        json!({"type":"response_item","payload":{
            "type":"message","role":"assistant","id":id,"phase":phase,
            "content":[{"type":"output_text","text":text}]
        }})
    )
}

#[test]
fn codex_messages_replay_completed_prose_in_turn_order_and_ignore_partial_writes() {
    let commentary = assistant(Some("m1"), "commentary", "checking 雪\n");
    let final_answer = assistant(None, "final_answer", "done");
    let contents = format!(
        "{META}{AUTO}{commentary}{}{}{}{}{}",
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"secret"}})
            .to_string()
            + "\n",
        assistant(Some("reason"), "analysis", "reasoning"),
        final_answer,
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn\"}}\n",
        assistant(Some("orphan"), "commentary", "not in a turn")
    );
    let (mut file, mut hook) = fixture(&contents);
    hook["hook_event_name"] = "PostToolUse".into();
    let (event, _) = parse_hook_event("codex", hook.to_string()).unwrap();
    assert_eq!(
        event
            .assistant_messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["checking 雪\n", "done"]
    );
    assert!(event.assistant_messages.iter().all(|m| m.turn_id == "turn"));
    assert!(event.assistant_messages[1].source_id.contains("offset:"));
    file.write_all(b"{\"type\":\"response_item\"").unwrap();
    let (partial, _) = parse_hook_event("codex", hook.to_string()).unwrap();
    assert_eq!(partial.assistant_messages, event.assistant_messages);
    hook["session_id"] = "wrong".into();
    assert!(parse_hook_event("codex", hook.to_string())
        .unwrap()
        .0
        .assistant_messages
        .is_empty());
}

#[test]
fn codex_message_tail_and_payload_limits_are_marked_and_keep_newest_in_order() {
    let padding = json!({"type":"unused","text":"x".repeat(TAIL_BYTES as usize)});
    let (_file, hook) = fixture(&format!(
        "{META}{AUTO}{padding}\n{}",
        assistant(None, "commentary", "no boundary")
    ));
    assert!(parse_hook_event("codex", hook.to_string())
        .unwrap()
        .0
        .assistant_messages
        .is_empty());
    assert!(
        parse_hook_event("codex", hook.to_string())
            .unwrap()
            .0
            .messages_omitted
    );
    let body: String = (0..100)
        .map(|n| {
            assistant(
                Some(&format!("m{n}")),
                "commentary",
                &"雪".repeat(OVERSIZE_CHARS / 25),
            )
        })
        .collect();
    let (_file, hook) = fixture(&format!("{META}{padding}\n{AUTO}{body}"));
    let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
    assert!(raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    assert!(event.messages_omitted);
    assert!(!event.assistant_messages.is_empty());
    assert_eq!(
        event.assistant_messages.last().unwrap().source_id,
        "session:m99"
    );
    assert!(event
        .assistant_messages
        .windows(2)
        .all(|pair| { pair[0].source_id < pair[1].source_id }));
    let (_file, hook) = fixture(&format!(
        "{META}{AUTO}{}",
        assistant(Some("huge"), "final_answer", &"雪".repeat(OVERSIZE_CHARS))
    ));
    let (event, raw) = parse_hook_event("codex", hook.to_string()).unwrap();
    assert!(raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    assert!(event.assistant_messages[0].incomplete);
}

#[test]
fn codex_messages_require_known_records_and_explicit_turn_boundaries() {
    for middle in [
        "{broken}\n".to_owned(),
        "{\"type\":\"turn_context\",\"payload\":{}}\n".to_owned(),
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_interrupted\"}}\n".to_owned(),
    ] {
        let (_file, hook) = fixture(&format!(
            "{META}{AUTO}{middle}{}",
            assistant(None, "commentary", "unattributed")
        ));
        assert!(parse_hook_event("codex", hook.to_string())
            .unwrap()
            .0
            .assistant_messages
            .is_empty());
    }
    let (_file, hook) = fixture(&format!(
        "{META}{AUTO}{}{}",
        assistant(Some("one"), "commentary", "same"),
        AUTO.replace("turn\"", "second\"") + &assistant(Some("two"), "final_answer", "same")
    ));
    let messages = parse_hook_event("codex", hook.to_string())
        .unwrap()
        .0
        .assistant_messages;
    assert_eq!(messages.len(), 2);
    assert_ne!(messages[0].turn_id, messages[1].turn_id);
}

#[test]
fn display_message_bursts_only_broadcast_events_and_preserve_pruned_batch_metadata() {
    // Every assertion below is per-batch, so the burst is only as long as it takes to have a
    // run of pruned batches followed by the empty final one. Each oversize delta costs a
    // binary-search prune over a quarter-megabyte, which is not free in a debug build.
    const BATCHES: u64 = 8;
    for index in 0..BATCHES {
        let event = HookEvent {
            hook_event_name: "MessageDisplay".into(),
            session_id: "session".into(),
            turn_id: Some("turn".into()),
            message_id: Some("message".into()),
            index: Some(index),
            final_batch: Some(index == BATCHES - 1),
            delta: Some(if index == BATCHES - 1 {
                String::new()
            } else {
                "雪".repeat(OVERSIZE_CHARS)
            }),
            ..Default::default()
        };
        let actions = hook_report_actions(
            "claude",
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            &event,
            serde_json::to_string(&event).unwrap(),
        );
        let [Action::AgentEvent(broadcast)] = &actions[..] else {
            panic!("display batches must not set pane state: {actions:?}")
        };
        assert_eq!(broadcast.state, None);
        assert_eq!(broadcast.detail, None);
        assert!(broadcast.raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        let received: HookEvent = serde_json::from_str(&broadcast.raw).unwrap();
        assert_eq!(received.turn_id, event.turn_id);
        assert_eq!(received.message_id, event.message_id);
        assert_eq!(received.index, event.index);
        assert_eq!(received.final_batch, event.final_batch);
        assert_eq!(received.message_incomplete, index != BATCHES - 1);
    }
}

/// opencode's plugins forward opencode's own event shape, so the parse that every road uses
/// reads it into the same `HookEvent` the other agents write - and keeps the translated event,
/// not the native one, as `raw`, because the broker validates `raw` and the daemon re-parses it.
#[test]
fn opencode_events_are_read_at_the_seam_and_the_translated_event_is_what_travels_as_raw() {
    let (event, raw) = parse_hook_event(
        "opencode",
        json!({
            "type": "tool.execute.before",
            "properties": {
                "sessionID": "ses_1", "cwd": "/work", "callID": "call_1",
                "tool": "bash", "args": {"command": "cargo build --release"},
            },
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "PreToolUse");
    assert_eq!(event.tool_name.as_deref(), Some("Bash"));
    assert_eq!(event.session_id, "ses_1");
    assert_eq!(event.cwd.as_deref(), Some("/work"));
    // `raw` is a `HookEvent`, which is what the broker and agent-service require of it.
    let raw_event: HookEvent = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw_event, event);
    assert_eq!(raw_event.tool_name.as_deref(), Some("Bash"));

    let actions = hook_report_actions(
        "opencode",
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
        &event,
        raw,
    );
    let [Action::SetPaneAgent { agent, .. }, Action::AgentEvent(broadcast)] = &actions[..] else {
        panic!("a tool call sets the record and broadcasts: {actions:?}")
    };
    assert_eq!(
        agent.as_ref().map(|agent| agent.kind.as_str()),
        Some("opencode")
    );
    assert_eq!(broadcast.kind, "opencode");
    assert_eq!(broadcast.state, Some(AgentState::Busy));
}

/// A Muse `Notification` fires when the agent needs approval, where Claude Code's
/// notification kinds are mostly not waits. It is translated to the unconditional
/// wait on the way in, so the record, the daemon's feed and a waited prompt all
/// read it the same way; everything else parses as the Claude Code shape it is.
#[test]
fn muse_notifications_are_translated_to_the_wait_they_name() {
    let notification = json!({
        "hook_event_name": "Notification",
        "session_id": "ses_1",
        "cwd": "/work",
        "model": "muse-spark",
        "permission_mode": "default",
        "message": "Allow `cargo build`?",
        "transcript_path": null,
    })
    .to_string();
    let (event, raw) = parse_hook_event("muse", notification).unwrap();
    assert_eq!(event.hook_event_name, "Elicitation");
    assert_eq!(event.session_id, "ses_1");
    assert_eq!(event.message.as_deref(), Some("Allow `cargo build`?"));
    let raw_event: HookEvent = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw_event, event);

    let actions = hook_report_actions(
        "muse",
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
        &event,
        raw,
    );
    let [Action::SetPaneAgent { agent, .. }, Action::AgentEvent(broadcast)] = &actions[..] else {
        panic!("an approval need sets the record and broadcasts: {actions:?}")
    };
    let agent = agent.as_ref().unwrap();
    assert_eq!(agent.kind, "muse");
    assert_eq!(agent.state, AgentState::Waiting);
    assert!(agent.wait_needs_a_person);
    assert_eq!(broadcast.kind, "muse");
    assert_eq!(broadcast.state, Some(AgentState::Waiting));

    // The rest of the feed parses untouched: a Claude Code shape under a muse kind.
    let submit = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "ses_1",
        "cwd": "/work",
        "prompt": "fix it",
        "turn_id": "turn_1",
        "permission_mode": "default",
        "transcript_path": null,
    })
    .to_string();
    let (event, unchanged) = parse_hook_event("muse", submit.clone()).unwrap();
    assert_eq!(event.hook_event_name, "UserPromptSubmit");
    assert_eq!(event.turn_id.as_deref(), Some("turn_1"));
    assert_eq!(unchanged, submit);
}

#[test]
fn agy_events_are_translated_and_enriched_from_transcript() {
    let mut transcript = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        transcript,
        r#"{{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nfix authentication timeout\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\ntime\n</ADDITIONAL_METADATA>"}}"#
    )
    .unwrap();
    writeln!(
        transcript,
        r#"{{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"I fixed the timeout in auth.rs."}}"#
    )
    .unwrap();

    // 1. PreInvocation at turn start (invocationNum == 0) translates to UserPromptSubmit and extracts prompt
    let (event, raw) = parse_hook_event(
        "agy",
        json!({
            "hook_event_name": "PreInvocation",
            "conversationId": "agy-conv-1",
            "workspacePaths": ["/home/user/code"],
            "transcriptPath": transcript.path(),
            "invocationNum": 0,
            "stepIdx": 1,
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "UserPromptSubmit");
    assert_eq!(event.session_id, "agy-conv-1");
    assert_eq!(event.cwd.as_deref(), Some("/home/user/code"));
    assert_eq!(event.prompt.as_deref(), Some("fix authentication timeout"));
    assert!(event.prompt_sha256.is_some());
    let raw_event: HookEvent = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw_event, event);

    // 2. Stop translates to Stop and extracts last assistant message
    let (event, _) = parse_hook_event(
        "agy",
        json!({
            "hook_event_name": "Stop",
            "conversationId": "agy-conv-1",
            "workspacePaths": ["/home/user/code"],
            "transcriptPath": transcript.path(),
            "terminationReason": "model_stop",
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "Stop");
    assert_eq!(
        event.last_assistant_message.as_deref(),
        Some("I fixed the timeout in auth.rs.")
    );

    // 3. PreToolUse with ask_question translates to Waiting state
    let (event, raw) = parse_hook_event(
        "agy",
        json!({
            "hook_event_name": "PreToolUse",
            "conversationId": "agy-conv-1",
            "workspacePaths": ["/home/user/code"],
            "toolCall": {
                "name": "ask_question",
                "args": {
                    "questions": [{ "question": "Apply migration?" }]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "PreToolUse");
    let actions = hook_report_actions(
        "agy",
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3)),
        &event,
        raw,
    );
    let [Action::SetPaneAgent { agent, .. }, Action::AgentEvent(broadcast)] = &actions[..] else {
        panic!("unexpected actions: {actions:?}");
    };
    assert_eq!(agent.as_ref().unwrap().state, AgentState::Waiting);
    assert_eq!(broadcast.state, Some(AgentState::Waiting));
}

#[test]
fn agy_last_assistant_message_is_bounded_to_current_turn() {
    let mut transcript = tempfile::NamedTempFile::new().unwrap();
    // Turn 1 completes successfully
    writeln!(
        transcript,
        r#"{{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nfirst request\n</USER_REQUEST>"}}"#
    )
    .unwrap();
    writeln!(
        transcript,
        r#"{{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"first turn answer"}}"#
    )
    .unwrap();

    // Turn 2: User prompt submitted, but fails before any PLANNER_RESPONSE
    writeln!(
        transcript,
        r#"{{"step_index":2,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","content":"<USER_REQUEST>\nresumed request that fails\n</USER_REQUEST>"}}"#
    )
    .unwrap();

    let (event, _) = parse_hook_event(
        "agy",
        json!({
            "hook_event_name": "Stop",
            "conversationId": "agy-conv-1",
            "workspacePaths": ["/home/user/code"],
            "transcriptPath": transcript.path(),
            "terminationReason": "error",
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "StopFailure");
    assert_eq!(event.last_assistant_message, None);

    // If Turn 2 later produces a PLANNER_RESPONSE, it should be extracted
    writeln!(
        transcript,
        r#"{{"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","content":"second turn answer"}}"#
    )
    .unwrap();

    let (event, _) = parse_hook_event(
        "agy",
        json!({
            "hook_event_name": "Stop",
            "conversationId": "agy-conv-1",
            "workspacePaths": ["/home/user/code"],
            "transcriptPath": transcript.path(),
            "terminationReason": "model_stop",
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(event.hook_event_name, "Stop");
    assert_eq!(
        event.last_assistant_message.as_deref(),
        Some("second turn answer")
    );
}

#[test]
fn agy_prompt_preserves_request_delimiters_in_submitted_prompt() {
    let test_prompts = [
        "Explain the </USER_REQUEST> closing tag",
        "Explain the <USER_REQUEST> opening tag",
        "<USER_REQUEST>nested</USER_REQUEST> and more </USER_REQUEST>",
        "prompt ending with </USER_REQUEST>",
        "Explain </USER_REQUEST> and how <ADDITIONAL_METADATA> works",
    ];

    for expected_prompt in test_prompts {
        let mut transcript = tempfile::NamedTempFile::new().unwrap();
        // Transcript contains the wrapper, the prompt containing literal delimiters,
        // and trailing metadata sections.
        let record = json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "content": format!(
                "<USER_REQUEST>\n{expected_prompt}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\ntime: 2026-10-01\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nmodel\n</USER_SETTINGS_CHANGE>"
            )
        });
        writeln!(transcript, "{record}").unwrap();

        let (event, raw) = parse_hook_event(
            "agy",
            json!({
                "hook_event_name": "PreInvocation",
                "conversationId": "agy-conv-1",
                "workspacePaths": ["/home/user/code"],
                "transcriptPath": transcript.path(),
                "invocationNum": 0,
                "stepIdx": 1,
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(event.hook_event_name, "UserPromptSubmit");
        assert_eq!(
            event.prompt.as_deref(),
            Some(expected_prompt),
            "Extracted prompt must match original prompt without truncating at delimiters"
        );
        let expected_sha256 = yinz_types::agent::control::prompt_sha256(expected_prompt);
        assert_eq!(
            event.prompt_sha256.as_deref(),
            Some(expected_sha256.as_str()),
            "Fingerprint must match full submitted prompt"
        );

        // Verify PromptObservation accepts the prompt as Submitted instead of Interrupted
        let mut observation = yinz_types::agent::control::PromptObservation::new(
            "agy",
            "agy-conv-1",
            expected_prompt,
        );
        let agent_event = yinz_types::agent::AgentEvent::from_hook(
            "agy",
            yinz_types::internal_ids::TerminalId::new(1),
            &event,
            raw,
        );
        assert_eq!(
            observation.observe(&agent_event, Some(&event)),
            Some(yinz_types::agent::control::AgentOutcome::Submitted),
            "PromptObservation must report Submitted when prompt delimiters are preserved"
        );
    }
}

#[test]
fn agy_native_submissions_and_normalized_confirmations_round_trip() {
    let (submission, normalized) = parse_hook_event(
        "agy",
        serde_json::json!({
            "hook_event_name":"PreInvocation", "conversationId":"conversation", "initialNumSteps":3,
            "invocationNum":0
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(submission.prompt_id.as_deref(), Some("conversation:3"));
    assert_eq!(submission.turn_id.as_deref(), Some("3"));
    assert_eq!(parse_hook_event("agy", normalized).unwrap().0, submission);
    for complete in [false, true] {
        let mut confirmation = submission.clone();
        confirmation.hook_event_name = "Stop".into();
        confirmation.agy_turn_complete = Some(complete);
        confirmation.last_assistant_message = Some("final answer".into());
        let json = serde_json::to_string(&confirmation).unwrap();
        let (parsed, raw) = parse_hook_event("agy", json).unwrap();
        assert_eq!(parsed, confirmation);
        assert_eq!(parse_hook_event("agy", raw).unwrap().0, confirmation);
    }
}

#[test]
fn tool_payload_survives_host_normalization_and_broadcast_pruning() {
    for provider in ["claude", "codex", "muse", "pi", "opencode", "custom"] {
        let raw = json!({"hook_event_name":"PostToolUse", "session_id":"s", "tool_use_id":"c", "tool_name":"shell", "agent_id":"worker", "tool_response":{"text":"雪".repeat(200000), "password":"must-not-persist"}}).to_string();
        let (event, raw) = parse_hook_event(provider, raw).unwrap();
        let capture = event.tool_capture.clone().unwrap();
        let mut bounded = event.broadcast_json(raw);
        assert!(bounded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        for _ in 0..2 {
            let (decoded, raw) = parse_hook_event(provider, bounded).unwrap();
            assert_eq!(decoded.tool_capture.as_ref(), Some(&capture), "{provider}");
            let serialized: HookEvent = serde_json::from_str(&raw).unwrap();
            assert_eq!(
                serialized.tool_capture.as_ref(),
                Some(&capture),
                "{provider}"
            );
            bounded = decoded.broadcast_json(raw);
            assert!(bounded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        }
        assert!(!capture.text().contains("must-not-persist"));
    }
}

#[test]
fn forwarding_preserves_redacted_payload_at_serialized_limit() {
    let raw = json!({
        "hook_event_name":"PostToolUse", "tool_name":"shell", "tool_use_id":"call",
        "tool_response":format!("PASSWORD=a {}", "x".repeat(65_413))
    })
    .to_string();
    let (event, raw) = parse_hook_event("codex", raw).unwrap();
    let capture = event.tool_capture.clone().unwrap();
    let output = capture.output.as_ref().unwrap();
    assert!(output.redacted);
    assert!(!output.truncated);
    assert_eq!(serde_json::to_vec(output).unwrap().len(), 64 * 1024);
    let mut bounded = event.broadcast_json(raw);
    for _ in 0..2 {
        let (decoded, raw) = parse_hook_event("codex", bounded).unwrap();
        let forwarded = decoded.tool_capture.as_ref().unwrap();
        assert_eq!(forwarded.output.as_ref().unwrap().text.len(), 65_424);
        assert_eq!(forwarded, &capture);
        let serialized: HookEvent = serde_json::from_str(&raw).unwrap();
        assert_eq!(serialized.tool_capture.as_ref(), Some(&capture));
        bounded = decoded.broadcast_json(raw);
    }
}

#[test]
fn malformed_tool_metadata_diagnostics_do_not_quote_payload_values() {
    let raw = json!({"hook_event_name":"PostToolUse", "tool_name":"shell", "duration_ms":"confidential-value"});
    let error = parse_hook_event("claude", raw.to_string())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("confidential-value"));
    assert!(error.contains("bytes"));
}

#[test]
fn supplied_tool_captures_are_bounded_and_sanitized_before_broadcast() {
    use yinz_types::agent::{
        report::{AgentReport, AgentReportState},
        tool_capture::Payload,
        AgentEvent,
    };
    use yinz_types::{
        internal_ids::{ProcessId, TerminalId},
        pane_id::PaneId,
    };

    let (mut hook, _) = parse_hook_event(
        "codex",
        json!({"hook_event_name":"PostToolUse", "session_id":"s", "tool_name":"shell",
            "tool_use_id":"call", "agent_id":"worker", "tool_response":{"exit_code":1}})
        .to_string(),
    )
    .unwrap();
    // Forwarded hooks may no longer carry the evidence used to prepare the capture.
    hook.tool_response = None;
    hook.prompt = Some("prose".repeat(100000));
    let capture = hook.tool_capture.as_mut().unwrap();
    let identity = capture.identity.clone();
    let metadata = format!(
        "\x1b[31mPASSWORD=metadata-secret\x1b[0m {}",
        "\"\\雪".repeat(10000)
    );
    capture.name = metadata.clone();
    capture.call_id = Some(metadata.clone());
    capture.turn_id = Some(metadata.clone());
    capture.worker_id = Some(metadata.clone());
    capture.parent_call_id = Some(metadata.clone());
    capture.source_event = metadata;
    capture.arguments = Some(Payload {
        text: json!({"nested":{"api_key":"argument-secret"},
            "image":{"type":"image", "data":"image-body"},
            "text":"\"\\雪\t".repeat(30000)})
        .to_string(),
        format: "json".into(),
        original_bytes: 1_000_000,
        redacted: false,
        truncated: false,
        attachments_omitted: false,
    });
    capture.output = Some(Payload {
        text: format!(
            "\x1b[31mPASSWORD=output-secret\x1b[0m {}",
            "x".repeat(300 * 1024)
        ),
        format: "text".into(),
        original_bytes: 1_000_000,
        redacted: false,
        truncated: false,
        attachments_omitted: true,
    });
    capture.error = Some(Payload {
        text: format!("PASSWORD=error-secret {}", "\"\\雪\t".repeat(30000)),
        format: "untrusted-format".repeat(30000),
        original_bytes: 1_000_000,
        redacted: true,
        truncated: true,
        attachments_omitted: false,
    });

    for provider in ["claude", "codex", "muse", "pi", "opencode", "agy", "custom"] {
        let (event, raw) =
            parse_hook_event(provider, serde_json::to_string(&hook).unwrap()).unwrap();
        let capture = event.tool_capture.as_ref().unwrap();
        assert_eq!(capture.identity, identity);
        assert_eq!(capture.status, "failed");
        assert!(!capture.output_unavailable);
        assert!(capture.metadata_redacted && capture.metadata_truncated);
        for field in [
            Some(&capture.name),
            capture.call_id.as_ref(),
            capture.turn_id.as_ref(),
            capture.worker_id.as_ref(),
            capture.parent_call_id.as_ref(),
            Some(&capture.source_event),
        ] {
            assert!(serde_json::to_vec(field.unwrap()).unwrap().len() <= 512);
        }
        for (payload, limit) in [
            (&capture.arguments, 16 * 1024),
            (&capture.output, 64 * 1024),
            (&capture.error, 8 * 1024),
        ] {
            let payload = payload.as_ref().unwrap();
            assert!(serde_json::to_vec(payload).unwrap().len() <= limit);
            assert_eq!(payload.original_bytes, 1_000_000);
            assert!(payload.redacted && payload.truncated);
            assert!(matches!(payload.format.as_str(), "json" | "text"));
        }
        assert!(capture.arguments.as_ref().unwrap().attachments_omitted);
        assert!(capture.output.as_ref().unwrap().attachments_omitted);
        let text = capture.text();
        for forbidden in [
            "metadata-secret",
            "argument-secret",
            "output-secret",
            "error-secret",
            "image-body",
            "\x1b",
            "[31m",
        ] {
            assert!(!text.contains(forbidden), "{provider}: {forbidden}");
        }
        let serialized: HookEvent = serde_json::from_str(&raw).unwrap();
        assert_eq!(serialized.tool_capture.as_ref(), Some(capture));
        let mut bounded = event.broadcast_json(raw);
        assert!(bounded.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
        for _ in 0..2 {
            let (decoded, raw) = parse_hook_event(provider, bounded).unwrap();
            assert_eq!(decoded.tool_capture.as_ref(), Some(capture), "{provider}");
            bounded = decoded.broadcast_json(raw);
        }
        let report = AgentReport {
            report_id: uuid::Uuid::new_v4().to_string(),
            pane_id: PaneId::Terminal(TerminalId::new(1)),
            state: AgentReportState::Unchanged,
            event: Some(AgentEvent::from_hook(
                provider,
                TerminalId::new(1),
                &event,
                bounded,
            )),
            trace: false,
            process_id: ProcessId::new(1),
            kind: provider.into(),
        };
        crate::agent::report::validate_report(&report).unwrap();
    }
}

#[test]
fn supplied_tool_capture_identity_and_status_errors_do_not_quote_values() {
    let (_, raw) = parse_hook_event(
        "codex",
        json!({"hook_event_name":"PostToolUse", "tool_name":"shell", "tool_use_id":"call"})
            .to_string(),
    )
    .unwrap();
    for field in ["identity", "status"] {
        let mut hook: Value = serde_json::from_str(&raw).unwrap();
        hook["tool_capture"][field] = "confidential-value".repeat(30000).into();
        for provider in ["claude", "codex"] {
            let Err(error) = parse_hook_event(provider, hook.to_string()) else {
                panic!("invalid capture must be rejected");
            };
            let error = error.to_string();
            assert!(!error.contains("confidential-value"));
            assert!(error.contains("tool capture"));
        }
    }
}
