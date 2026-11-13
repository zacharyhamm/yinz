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

fn cached_result(throttled: bool, retry: Option<u64>) -> (ProviderUsage, FetchMetadata) {
    (
        failure("claude", ProviderUsageStatus::Error, "test result"),
        FetchMetadata {
            throttled,
            retry_after: retry.map(Duration::from_secs),
        },
    )
}

#[test]
fn shared_cache_expires_and_backs_off_then_recovers() {
    use std::cell::Cell;
    let cache = ProviderCache::default();
    let clock = Cell::new(Instant::now());
    let now = || clock.get();
    let unexpected_fetches = Cell::new(0);
    cache.collect("claude", now, || cached_result(false, None));
    cache.collect("claude", now, || {
        unexpected_fetches.set(unexpected_fetches.get() + 1);
        cached_result(false, None)
    });
    // Inspect deadlines as well as results: collector panics are deliberately sanitized.
    assert_eq!(
        cache.entry.lock().unwrap().result.as_ref().unwrap().1,
        now() + Duration::from_secs(300)
    );
    clock.set(now() + Duration::from_secs(300));
    for seconds in [900, 1800, 3600, 3600] {
        cache.collect("claude", now, || cached_result(true, None));
        let deadline = cache.entry.lock().unwrap().result.as_ref().unwrap().1;
        assert_eq!(deadline, now() + Duration::from_secs(seconds));
        cache.collect("claude", now, || {
            unexpected_fetches.set(unexpected_fetches.get() + 1);
            cached_result(false, None)
        });
        assert_eq!(
            cache.entry.lock().unwrap().result.as_ref().unwrap().1,
            deadline
        );
        clock.set(deadline);
    }
    assert_eq!(unexpected_fetches.get(), 0);
    cache.collect("claude", now, || cached_result(true, Some(7200)));
    assert_eq!(
        cache.entry.lock().unwrap().result.as_ref().unwrap().1,
        now() + Duration::from_secs(7200)
    );
    clock.set(now() + Duration::from_secs(7200));
    cache.collect("claude", now, || cached_result(false, None));
    assert_eq!(cache.entry.lock().unwrap().throttle_streak, 0);
    clock.set(now() + Duration::from_secs(300));
    cache.collect("claude", now, || cached_result(true, None));
    assert_eq!(
        cache.entry.lock().unwrap().result.as_ref().unwrap().1,
        now() + Duration::from_secs(900)
    );
}

#[test]
fn failed_refresh_retains_observation_through_backoff_and_reopening_then_recovers() {
    use std::cell::Cell;
    let cache = ProviderCache::default();
    let clock = Cell::new(Instant::now());
    let now = || clock.get();
    let success = claude_with(credential(None), Some(100), |_| {
        Ok((200, br#"{"seven_day":{"utilization":42}}"#.to_vec()))
    });
    assert_eq!(
        cache.collect("claude", now, || (
            success.clone(),
            FetchMetadata::default()
        )),
        success
    );
    clock.set(now() + Duration::from_secs(300));
    let throttled = cache.collect("claude", now, || cached_result(true, None));
    assert_eq!(throttled.status, ProviderUsageStatus::Error);
    assert_eq!(throttled.windows, success.windows);
    assert_eq!(throttled.observed_at_ms, success.observed_at_ms);
    assert_eq!(throttled.plan, success.plan);
    assert_eq!(throttled.message.as_deref(), Some("test result"));
    // A new consumer during cooldown gets the same retained observation without fetching.
    assert_eq!(
        cache.collect("claude", now, || panic!("retried during cooldown")),
        throttled
    );
    clock.set(now() + Duration::from_secs(900));
    let mut refreshed = success.clone();
    refreshed.windows[0].used_percent = 50.0;
    assert_eq!(
        cache.collect("claude", now, || (
            refreshed.clone(),
            FetchMetadata::default()
        )),
        refreshed
    );
    clock.set(now() + Duration::from_secs(300));
    let expired = cache.collect("claude", now, || {
        (
            failure("claude", ProviderUsageStatus::Expired, "expired"),
            FetchMetadata::default(),
        )
    });
    assert!(expired.windows.is_empty());
    clock.set(now() + Duration::from_secs(300));
    assert!(cache
        .collect("claude", now, || cached_result(true, None))
        .windows
        .is_empty());
}

#[test]
fn codex_cache_keeps_its_one_minute_refresh_interval() {
    let cache = ProviderCache::default();
    let now = Instant::now();
    cache.collect("codex", || now, || cached_result(false, None));
    assert_eq!(
        cache.entry.lock().unwrap().result.as_ref().unwrap().1,
        now + Duration::from_secs(60)
    );
}

#[test]
fn retry_after_supports_seconds_dates_and_invalid_headers() {
    let epoch = SystemTime::UNIX_EPOCH;
    assert_eq!(retry_after("120", epoch), Some(Duration::from_secs(120)));
    assert_eq!(
        retry_after("Thu, 01 Jan 1970 00:02:00 GMT", epoch),
        Some(Duration::from_secs(120))
    );
    assert_eq!(
        retry_after(
            "Thu, 01 Jan 1970 00:00:00 GMT",
            epoch + Duration::from_secs(1)
        ),
        Some(Duration::ZERO)
    );
    assert_eq!(retry_after("nonsense", epoch), None);
    assert_eq!(retry_after("-10", epoch), None);
}

#[test]
fn concurrent_callers_share_one_fetch_and_providers_are_independent() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cache = ProviderCache::default();
    let other = ProviderCache::default();
    let calls = AtomicUsize::new(0);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let cache_ref = &cache;
        let calls_ref = &calls;
        let first = scope.spawn(move || {
            cache_ref.collect("claude", Instant::now, || {
                calls_ref.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                finish_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                cached_result(false, None)
            })
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let second = scope.spawn(|| {
            cache.collect("claude", Instant::now, || {
                calls.fetch_add(1, Ordering::SeqCst);
                cached_result(false, None)
            })
        });
        other.collect("codex", Instant::now, || cached_result(false, None));
        finish_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn collector_panic_releases_pending_state_and_caches_sanitized_failure() {
    let cache = ProviderCache::default();
    let result = cache.collect("claude", Instant::now, || panic!("sentinel-secret"));
    assert_eq!(result.status, ProviderUsageStatus::Error);
    assert!(!result.message.unwrap().contains("sentinel-secret"));
    assert!(!cache.entry.lock().unwrap().pending);
    assert_eq!(
        cache
            .collect("claude", Instant::now, || panic!("unexpected retry"))
            .status,
        ProviderUsageStatus::Error
    );
}

#[test]
fn transport_errors_keep_the_failure_cause_for_logs() {
    let error = transport_error(isahc::error::ErrorKind::ConnectionFailed.into());
    let message = format!("{error:#}");
    assert!(message.starts_with("Claude usage request failed: "));
    assert!(message.len() > "Claude usage request failed: ".len());
}
