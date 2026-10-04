use super::*;
use serde_json::json;

#[test]
fn capture_redacts_before_bounding_and_preserves_unknown_outcomes() {
    let event = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("exec_command".into()),
        tool_use_id: Some("call-1".into()),
        agent_id: Some("worker".into()),
        tool_input: Some(
            json!({"nested": {"api_key": "sensitive", "api_token":"token-value", "auth_token":"auth-value"}, "command": "PASSWORD=hidden echo ok"}),
        ),
        tool_response: Some(
            json!({"content": [{"type": "image", "data": "attachment"}], "text": "雪\n".repeat(100000)}),
        ),
        ..Default::default()
    };
    let capture = prepare("codex", &event).unwrap();
    let stored = serde_json::to_string(&capture).unwrap();
    assert!(!stored.contains("sensitive"));
    assert!(!stored.contains("token-value"));
    assert!(!stored.contains("auth-value"));
    assert!(!stored.contains("hidden"));
    assert!(!stored.contains("\"data\":\"attachment\""));
    assert_eq!(capture.status, "completed");
    assert!(capture.output.as_ref().unwrap().truncated);
    assert!(capture.output.as_ref().unwrap().attachments_omitted);
    assert!(stored.len() <= 96 * 1024);
    assert!(stored.len() + serde_json::to_string(&capture.text()).unwrap().len() <= 192 * 1024);
    assert!(capture.event_id().unwrap().starts_with("tool:"));
}

#[test]
fn identity_outcome_and_unavailable_output_are_truthful() {
    let mut hook = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("shell".into()),
        ..Default::default()
    };
    let capture = prepare("custom", &hook).unwrap();
    assert!(capture.event_id().is_none());
    assert!(capture.output_unavailable);
    hook.tool_response = Some(json!({"exit_code": 1, "text": "fine"}));
    assert_eq!(prepare("codex", &hook).unwrap().status, "failed");
    hook.hook_event_name = "PermissionRequest".into();
    assert!(prepare("claude", &hook).is_none());
    hook.hook_event_name = "PostToolUseFailure".into();
    hook.is_interrupt = Some(true);
    assert_eq!(prepare("claude", &hook).unwrap().status, "cancelled");
    assert!(
        !redact_secrets("-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----")
            .contains("secret")
    );
}

#[test]
fn all_payloads_and_escaped_metadata_respect_serialized_ceilings() {
    let value =
        json!({"text":"\"\\雪\t".repeat(30000), "nested":[{"authorization":"Bearer hidden"}]});
    let hook = HookEvent {
        hook_event_name: "PostToolUseFailure".into(),
        tool_name: Some("\"\\".repeat(10000)),
        tool_use_id: Some("\"\\".repeat(10000)),
        agent_id: Some("\"\\".repeat(10000)),
        turn_id: Some("\"\\".repeat(10000)),
        parent_tool_use_id: Some("\"\\".repeat(10000)),
        tool_input: Some(value.clone()),
        tool_response: Some(value.clone()),
        error: Some(value),
        ..Default::default()
    };
    let capture = prepare("custom", &hook).unwrap();
    for (payload, cap) in [
        (&capture.arguments, 16 * 1024),
        (&capture.output, 64 * 1024),
        (&capture.error, 8 * 1024),
    ] {
        let payload = payload.as_ref().unwrap();
        assert!(payload.truncated && payload.redacted);
        assert!(serde_json::to_vec(payload).unwrap().len() <= cap);
    }
    let bytes = serde_json::to_vec(&capture).unwrap();
    assert!(bytes.len() <= 96 * 1024);
    assert!(bytes.len() + serde_json::to_vec(&capture.text()).unwrap().len() <= 192 * 1024);
    assert!(capture.metadata_truncated);
    assert!(capture.event_id().is_some());
}

#[test]
fn textual_credentials_are_removed_without_erasing_paths_or_commit_hashes() {
    let input = "PASSWORD=hidden Bearer credential== postgres://person:pass@database/path sk-abcdefghijklmnopqrstuvwxyz -----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY-----\n/home/me/repo abcdef0123456789abcdef0123456789abcdef0123";
    let safe = redact_secrets(input);
    for secret in [
        "hidden",
        "credential",
        "person:pass",
        "sk-abcdef",
        "\nprivate\n",
    ] {
        assert!(!safe.contains(secret));
    }
    assert!(safe.contains("database/path"));
    assert!(safe.contains("/home/me/repo"));
    assert!(safe.contains("abcdef0123456789abcdef0123456789abcdef0123"));
}

#[test]
fn status_uses_structured_evidence_and_never_output_words() {
    for (response, expected) in [
        (json!({"text":"error failed denied"}), "completed"),
        (json!({"isError":false}), "succeeded"),
        (json!({"exit_code":0}), "succeeded"),
        (json!({"exit_code":3}), "failed"),
    ] {
        let hook = HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("shell".into()),
            tool_response: Some(response),
            ..Default::default()
        };
        assert_eq!(prepare("codex", &hook).unwrap().status, expected);
    }
}

#[test]
fn attachments_keep_nested_mime_metadata_and_omit_the_body() {
    let hook = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("read_image".into()),
        tool_response: Some(
            json!({"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":"unretained-image-body"}}),
        ),
        ..Default::default()
    };
    let capture = prepare("claude", &hook).unwrap();
    let output = capture.output.unwrap();
    assert!(output.attachments_omitted);
    assert!(output.text.contains("image/png"));
    assert!(!output.text.contains("unretained-image-body"));
}
