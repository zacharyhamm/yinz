use super::*;
use serde_json::json;

fn credential(expiry: Option<u64>) -> ClaudeCredentials {
    credentials(
        &serde_json::to_vec(&json!({"claudeAiOauth": {
            "accessToken": "sentinel-token", "expiresAt": expiry, "subscriptionType": "max"
        }}))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn credential_files_are_bounded_and_errors_do_not_include_contents() {
    let temp = tempfile::tempdir().unwrap();
    assert!(file_credentials(temp.path()).unwrap().is_none());
    let file = temp.path().join(".credentials.json");
    std::fs::write(&file, b"sentinel-token").unwrap();
    let error = file_credentials(temp.path()).err().unwrap();
    assert!(!format!("{error:#}").contains("sentinel-token"));
    std::fs::write(&file, vec![b'x'; DATA_LIMIT as usize + 1]).unwrap();
    assert!(file_credentials(temp.path()).is_err());
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(file_credentials(temp.path()).is_err());
}

#[test]
fn keychain_accepts_raw_and_hex_json_without_exposing_tokens() {
    let raw = br#"{"claudeAiOauth":{"accessToken":"sentinel-token"}}"#;
    let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        keychain_credentials(raw).unwrap().access_token,
        "sentinel-token"
    );
    assert_eq!(
        keychain_credentials(hex.as_bytes()).unwrap().access_token,
        "sentinel-token"
    );
    assert!(keychain_credentials(b"not-hex-or-json").is_err());
}

#[test]
fn expired_credentials_skip_http_and_upstream_errors_are_sanitized() {
    let usage = claude_with(credential(Some(100)), Some(100), |_| {
        panic!("expired credential was sent")
    });
    assert_eq!(usage.status, ProviderUsageStatus::Expired);
    for (status, expected) in [
        (401, ProviderUsageStatus::Expired),
        (429, ProviderUsageStatus::Error),
        (500, ProviderUsageStatus::Error),
    ] {
        let usage = claude_with(credential(None), Some(100), |_| {
            Ok((status, b"sentinel-token".to_vec()))
        });
        assert_eq!(usage.status, expected);
        assert_eq!(usage.observed_at_ms, None);
        assert!(!serde_json::to_string(&usage)
            .unwrap()
            .contains("sentinel-token"));
        if status == 429 {
            assert!(usage
                .message
                .unwrap()
                .contains("does not mean your quota is exhausted"));
        }
    }
    let usage = claude_with(credential(None), Some(100), |_| {
        Err(anyhow!("sentinel-token"))
    });
    assert!(!serde_json::to_string(&usage)
        .unwrap()
        .contains("sentinel-token"));
}

#[test]
fn claude_quota_windows_keep_real_percentages_and_optional_resets() {
    let usage = claude_with(credential(None), Some(100), |token| {
        assert_eq!(token, "sentinel-token");
        Ok((200, serde_json::to_vec(&json!({
            "five_hour": {"utilization": 120.5, "resets_at": "2026-01-01T00:00:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "invalid"},
            "limits": [{"kind":"weekly_scoped","scope":{"model":{"display_name":"Sonnet"}},"percent":30}]
        })).unwrap()))
    });
    assert_eq!(usage.status, ProviderUsageStatus::Ok);
    assert_eq!(usage.plan.as_deref(), Some("max"));
    assert_eq!(usage.windows[0].used_percent, 120.5);
    assert_eq!(usage.windows[0].resets_at_ms, Some(1767225600000));
    assert_eq!(usage.windows[1].resets_at_ms, None);
    assert_eq!(usage.windows[2].label, "Weekly Sonnet");
    assert!(!serde_json::to_string(&usage)
        .unwrap()
        .contains("sentinel-token"));
}

#[test]
fn legacy_model_limits_expose_exhaustion_and_deduplicate_modern_windows() {
    let windows = claude_windows(&json!({
        "seven_day_sonnet": {"utilization": 100, "resets_at": "2026-01-01T00:00:00Z"},
        "seven_day_opus": {"utilization": 25},
        "limits": [{
            "kind": "weekly_scoped",
            "scope": {"model": {"display_name": "SONNET "}},
            "percent": 100
        }]
    }))
    .unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].label, "Weekly Sonnet");
    assert_eq!(windows[0].used_percent, 100.0);
    assert_eq!(windows[0].resets_at_ms, Some(1767225600000));
    assert_eq!(windows[1].label, "Weekly Opus");
    assert_eq!(windows[1].used_percent, 25.0);
    assert_eq!(
        claude_windows(&json!({"seven_day_sonnet": {"utilization": 100}}))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn malformed_and_missing_quotas_are_never_zero_usage() {
    for (body, expected) in [
        (json!({}), ProviderUsageStatus::Error),
        (json!({"five_hour":"invalid"}), ProviderUsageStatus::Error),
        (json!({"limits":"invalid"}), ProviderUsageStatus::Error),
        (
            json!({"five_hour":{"utilization":-1}}),
            ProviderUsageStatus::Error,
        ),
        (json!({"five_hour":null}), ProviderUsageStatus::Unavailable),
    ] {
        let usage = claude_with(credential(None), Some(100), |_| {
            Ok((200, serde_json::to_vec(&body).unwrap()))
        });
        assert_eq!(usage.status, expected);
        assert!(usage.windows.is_empty());
        assert_eq!(usage.observed_at_ms, None);
    }
}

#[test]
fn request_keeps_credentials_in_headers_and_uses_fixed_https_origin() {
    let request = claude_request("sentinel-token").unwrap();
    assert_eq!(request.uri(), CLAUDE_USAGE_URL);
    assert_eq!(request.method(), "GET");
    assert_eq!(request.headers()["authorization"], "Bearer sentinel-token");
    assert_eq!(request.headers()["anthropic-beta"], "oauth-2025-04-20");
    assert!(claude_request("invalid\nheader").is_err());
    let unsupported = collect("unknown-provider");
    assert_eq!(unsupported.status, ProviderUsageStatus::Unsupported);
    assert_eq!(unsupported.observed_at_ms, None);
}
