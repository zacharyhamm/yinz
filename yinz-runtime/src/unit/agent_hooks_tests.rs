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
        assert_eq!(parse_hook_event(kind, raw.clone()).unwrap().1, raw);
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
    // `raw` is a `HookEvent`, which is what the broker and hocus-agentd require of it.
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
