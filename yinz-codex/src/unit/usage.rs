use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

async fn fixture(account: Value, limits: Value) -> ProviderUsage {
    let (client, server) = tokio::io::duplex(16384);
    let client = async {
        let (input, mut output) = tokio::io::split(client);
        read_usage(&mut output, &mut BufReader::new(input))
            .await
            .unwrap()
    };
    let server = async {
        let (input, mut output) = tokio::io::split(server);
        let mut input = BufReader::new(input);
        loop {
            let mut line = String::new();
            if input.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            let message: Value = serde_json::from_str(&line).unwrap();
            let result = match message["method"].as_str().unwrap() {
                "initialize" => json!({}),
                "initialized" => continue,
                "account/read" => {
                    assert_eq!(message["params"], json!({"refreshToken": false}));
                    json!({"account": account})
                },
                "account/rateLimits/read" => limits.clone(),
                method => panic!("unexpected method: {method}"),
            };
            output
                .write_all(b"{\"method\":\"account/updated\",\"params\":{}}\n")
                .await
                .unwrap();
            output
                .write_all(
                    format!("{}\n", json!({"id": message["id"], "result": result})).as_bytes(),
                )
                .await
                .unwrap();
        }
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(client, server).0
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn quota_rpc_keeps_percentages_and_discards_account_identity() {
    let usage = fixture(json!({"type": "chatgpt", "planType": "pro", "email": "secret@example.test"}), json!({
        "rateLimits": {
            "primary": {"usedPercent": 125.5, "windowDurationMins": 300, "resetsAt": 1700000000},
            "secondary": {"usedPercent": 12, "windowDurationMins": 10080, "resetsAt": null}
        }
    })).await;
    assert_eq!(usage.status, ProviderUsageStatus::Ok);
    assert_eq!(usage.plan.as_deref(), Some("pro"));
    assert_eq!(usage.windows[0].used_percent, 125.5);
    assert_eq!(usage.windows[0].resets_at_ms, Some(1700000000000));
    assert_eq!(usage.windows[1].label, "Weekly limit");
    assert_eq!(usage.windows[1].resets_at_ms, None);
    assert!(usage.observed_at_ms.is_some());
    assert!(!serde_json::to_string(&usage)
        .unwrap()
        .contains("secret@example.test"));
}

#[tokio::test]
async fn account_types_and_missing_windows_remain_explicit() {
    for (account, status) in [
        (Value::Null, ProviderUsageStatus::Unavailable),
        (json!({"type": "apiKey"}), ProviderUsageStatus::Unsupported),
        (
            json!({"type": "amazonBedrock"}),
            ProviderUsageStatus::Unsupported,
        ),
        (json!({"type": "chatgpt"}), ProviderUsageStatus::Unavailable),
    ] {
        let usage = fixture(account, json!({"rateLimits": {"primary": null}})).await;
        assert_eq!(usage.status, status);
        assert!(usage.windows.is_empty());
        assert_eq!(usage.observed_at_ms, None);
    }
}

#[tokio::test]
async fn rpc_errors_are_redacted_and_large_lines_are_rejected() {
    let mut output =
        BufReader::new(&b"{\"id\":1,\"error\":{\"message\":\"sentinel-token\"}}\n"[..]);
    let error = request(
        &mut tokio::io::sink(),
        &mut output,
        1,
        "account/read",
        Value::Null,
    )
    .await
    .unwrap_err();
    assert!(!format!("{error:#}").contains("sentinel-token"));
    let large = vec![b'x'; RESPONSE_LIMIT as usize + 2];
    let mut output = BufReader::new(large.as_slice());
    assert!(request(
        &mut tokio::io::sink(),
        &mut output,
        1,
        "account/read",
        Value::Null
    )
    .await
    .is_err());
}

#[test]
fn invalid_percentages_do_not_become_zero_and_reset_overflow_is_unknown() {
    assert!(normalize(
        &json!({"rateLimits":{"primary":{"usedPercent":-1}}}),
        &Value::Null
    )
    .is_err());
    assert!(normalize(
        &json!({"rateLimits":{"primary":{"usedPercent":"secret"}}}),
        &Value::Null
    )
    .is_err());
    let usage = normalize(
        &json!({"rateLimits":{"primary":{"usedPercent":0,"resetsAt":u64::MAX}}}),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(usage.windows[0].resets_at_ms, None);
}

#[tokio::test]
async fn one_shot_child_is_reaped_after_an_account_reply() {
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("codex-fixture");
    let pid_file = temp.path().join("pid");
    yinz_test_support::write_warm_script(&program, &format!(
        "printf '%s' $$ > '{}'\nread -r line\nprintf '%s\\n' '{{\"id\":0,\"result\":{{}}}}'\nread -r line\nread -r line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"account\":null}}}}'\nexec cat >/dev/null\n", pid_file.display()
    ));
    let usage = provider_usage(&program).await.unwrap();
    assert_eq!(usage.status, ProviderUsageStatus::Unavailable);
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}
