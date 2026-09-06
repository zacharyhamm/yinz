use super::*;
use serde_json::json;
use std::io::Write;
use zellij_types::{
    agent::AgentState, input::actions::hook_report_actions, input::actions::Action,
};

const META: &str = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\"}}\n";
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
            zellij_types::agent::waiting_needs_a_person(&event),
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
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(7)),
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
        assert!(broadcast.raw.len() <= zellij_types::agent::HOOK_BROADCAST_MAX_BYTES);
        let received: HookEvent = serde_json::from_str(&broadcast.raw).unwrap();
        assert_eq!(received.approvals_reviewer.as_deref(), Some("auto_review"));
        assert_eq!(
            zellij_types::agent::hook_state(&received),
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
            zellij_types::agent::hook_state(&event),
            Some(AgentState::Waiting)
        );
        assert!(zellij_types::agent::waiting_needs_a_person(&event));
    }
}
