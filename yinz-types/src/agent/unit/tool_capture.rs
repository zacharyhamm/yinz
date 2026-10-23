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

#[test]
fn supplied_small_payloads_are_sanitized_even_with_preexisting_flags() {
    for (format, text, attachments) in [
        (
            "text",
            "PASS\x1b[31mWORD=hidden\x1b[0m\n雪\tresult\x07".into(),
            false,
        ),
        (
            "json",
            r#"{"password":"hidden", "unfinished":"value"#.into(),
            false,
        ),
        ("unknown", "PASSWORD=hidden".into(), false),
        (
            "json",
            json!({"nested":{"api_key":"hidden"},
            "content":[{"type":"image", "data":"image-body"}],
            "text":"\x1b]0;window-title\x07雪\tresult\x1b[31m"})
            .to_string(),
            true,
        ),
    ] {
        let hook = HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("PASS\nWORD=metadata-hidden\x1b[31m".into()),
            tool_use_id: Some("call".into()),
            ..Default::default()
        };
        let mut capture = prepare("codex", &hook).unwrap();
        let identity = capture.identity.clone();
        capture.output = Some(Payload {
            text,
            format: format.into(),
            original_bytes: 1234,
            redacted: true,
            truncated: true,
            attachments_omitted: false,
        });
        capture.normalize().unwrap();
        assert_eq!(capture.identity, identity);
        assert_eq!(capture.status, "completed");
        assert!(!capture.name.contains("metadata-hidden"));
        assert!(capture.metadata_redacted);
        let output = capture.output.as_ref().unwrap();
        assert_eq!(output.original_bytes, 1234);
        assert!(output.redacted && output.truncated);
        assert_eq!(output.attachments_omitted, attachments);
        assert!(matches!(output.format.as_str(), "json" | "text"));
        for forbidden in ["hidden", "image-body", "\x1b", "window-title", "[31m"] {
            assert!(!output.text.contains(forbidden), "{format}: {forbidden}");
        }
        let normalized = capture.clone();
        capture.normalize().unwrap();
        assert_eq!(capture, normalized);
    }
}

#[test]
fn normalization_preserves_redacted_payload_at_serialized_limit() {
    let hook = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("shell".into()),
        tool_response: Some(json!(format!("PASSWORD=a {}", "x".repeat(65_413)))),
        ..Default::default()
    };
    let mut capture = prepare("codex", &hook).unwrap();
    let output = capture.output.as_ref().unwrap();
    assert!(output.redacted);
    assert!(!output.truncated);
    assert_eq!(output.text.len(), 65_424);
    assert_eq!(serde_json::to_vec(output).unwrap().len(), OUTPUT_BYTES);
    let original = capture.clone();
    for _ in 0..2 {
        capture.normalize().unwrap();
        assert_eq!(capture.output.as_ref().unwrap().text.len(), 65_424);
        assert_eq!(capture, original);
    }
}

#[test]
fn prepared_capture_normalization_preserves_outcomes_provenance_and_omissions() {
    let hook = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("shell".into()),
        tool_use_id: Some("call".into()),
        tool_response: Some(json!({"password":"x", "text":"雪".repeat(30000)})),
        tool_input: Some(json!({"password":"x"})),
        duration_ms: Some(123),
        ..Default::default()
    };
    for status in [
        "requested",
        "cancelled",
        "failed",
        "denied",
        "succeeded",
        "completed",
    ] {
        for omitted in [false, true] {
            let mut capture = prepare("codex", &hook).unwrap();
            capture.status = status.into();
            capture.metadata_redacted = true;
            capture.metadata_truncated = true;
            if omitted {
                capture.identity = None;
                capture.omit_payload();
            }
            let original = capture.clone();
            for _ in 0..2 {
                capture.normalize().unwrap();
                assert_eq!(capture, original);
            }
        }
    }
}

#[test]
fn malformed_prepared_identities_are_rejected_without_rewriting_them() {
    let hook = HookEvent {
        hook_event_name: "PostToolUse".into(),
        tool_name: Some("shell".into()),
        ..Default::default()
    };
    for identity in [
        String::new(),
        format!("tool:{}", "a".repeat(63)),
        format!("tool:{}", "a".repeat(65)),
        format!("tool:{}", "g".repeat(64)),
        format!("other:{}", "a".repeat(64)),
    ] {
        let mut capture = prepare("codex", &hook).unwrap();
        capture.identity = Some(identity);
        let original = capture.clone();
        assert!(capture.normalize().is_err());
        assert_eq!(capture, original);
    }
}
