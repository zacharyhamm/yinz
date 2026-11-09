use super::*;
#[test]
fn funnel_matching_all_ports_foreground_aliases_and_unknown_shapes() {
    let target = "127.0.0.1:7419".parse().unwrap();
    for (proxy, target) in [
        ("http://127.0.0.1:7419", target),
        ("http://localhost:7419/path", target),
        ("http://[::1]:7419/path", "[::1]:7419".parse().unwrap()),
    ] {
        let config = serde_json::json!({"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":proxy}}}},"AllowFunnel":{"box.ts.net:443":true}});
        assert_eq!(exposure(&config, &[target]), Exposure::Exposed);
        assert_eq!(
            exposure(&serde_json::json!({"Foreground":{"id":config}}), &[target]),
            Exposure::Exposed
        );
    }
    let alias = serde_json::json!({"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.2:7419"}}}},"AllowFunnel":{"box.ts.net:443":true}});
    assert_eq!(exposure(&alias, &[target]), Exposure::NoMatch);
    let unrelated = serde_json::json!({"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":"http://localhost:22"}}}},"AllowFunnel":{"box.ts.net:443":true}});
    assert_eq!(exposure(&unrelated, &[target]), Exposure::NoMatch);
    for invalid in [
        serde_json::json!({"AllowFunnel":{"box:443":"yes"}}),
        serde_json::json!({"Services":{"svc:x":{}}}),
        serde_json::json!({"NewSchema":{}}),
        serde_json::json!({"TCP":"malformed"}),
        Value::Null,
    ] {
        assert_eq!(exposure(&invalid, &[target]), Exposure::Unknown);
    }
    assert_eq!(backend("http://evil.example:7419"), None);
    assert_eq!(backend("https://localhost:7419"), None);
}
#[tokio::test]
async fn diagnostic_bounded_errors_timeout_and_no_raw_output() {
    let t = tempfile::tempdir().unwrap();
    let script = t.path().join("tailscale");
    yinz_test_support::write_warm_script(
        &script,
        "printf '%s' '{\"Self\":{\"DNSName\":\"box.ts.net.\"}}'\n",
    );
    assert_eq!(
        diagnostic(&script, &["status", "--json"]).await.unwrap()["Self"]["DNSName"],
        "box.ts.net."
    );
    yinz_test_support::write_warm_script(&script, "printf 'secret malformed output'\n");
    assert!(diagnostic(&script, &[]).await.is_none());
    yinz_test_support::write_warm_script(&script, "exec /usr/bin/sleep 10\n");
    assert!(diagnostic(&script, &[]).await.is_none());
    yinz_test_support::write_warm_script(&script, "exec /usr/bin/head -c 262145 /dev/zero\n");
    assert!(diagnostic(&script, &[]).await.is_none());
    assert!(diagnostic(&t.path().join("missing"), &[]).await.is_none());
}
#[test]
fn cli_status_stays_local_and_loads_config_without_layout() {
    let t = tempfile::tempdir().unwrap();
    let stub = t.path().join("tailscale");
    yinz_test_support::write_warm_script(&stub,"if [ \"$1\" = serve ]; then printf '{}'; else printf '%s' '{\"Self\":{\"DNSName\":\"box.ts.net.\"}}'; fi");
    let config = t.path().join("config.kdl");
    std::fs::write(
        &config,
        "web { enabled true; port 4321; https_port_hint 8443; }\n",
    )
    .unwrap();
    let output =
        std::process::Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz"))
            .env("PATH", t.path())
            .args(["--remote", "must-not-contact.invalid", "--data-dir"])
            .arg(t.path())
            .arg("--config")
            .arg(&config)
            .args(["web", "status", "--https-port", "9443"])
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8(output.stdout).unwrap();
    assert!(out.contains("enabled=true bind=127.0.0.1:4321"), "{out}");
    assert!(out.contains("--https=9443"));
    assert!(out.contains("configured-only"));
    assert!(!out.contains("phc"));
    assert!(!out.contains("verifier_id"));
}

#[test]
fn cli_status_defaults_config_hint_funnel_exit_and_secret_free_sessions() {
    let t = tempfile::tempdir().unwrap();
    let stub = t.path().join("tailscale");
    let config = t.path().join("config.kdl");
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz");
    let invoke = || {
        std::process::Command::new(&binary)
            .env("PATH", t.path())
            .arg("--data-dir")
            .arg(t.path())
            .arg("--config")
            .arg(&config)
            .args(["web", "status"])
            .output()
            .unwrap()
    };
    yinz_test_support::write_warm_script(&stub, "printf '%s' '{}' ");
    std::fs::write(&config, "invalid {{{").unwrap();
    let output = invoke();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("values below are defaults"));
    assert!(text.contains("--https=7443"));
    std::fs::write(
        &config,
        "web { enabled true; port 4321; https_port_hint 8443; }\n",
    )
    .unwrap();
    yinz_test_support::write_warm_script(
        &stub,
        r#"if [ "$1" = serve ]; then printf '%s' '{"Web":{"box.ts.net:443":{"Handlers":{"/":{"Proxy":"http://localhost:4321/path"}}}},"AllowFunnel":{"box.ts.net:443":true}}'; else printf '%s' '{"Self":{"DNSName":"box.ts.net."}}'; fi"#,
    );
    password::write(
        t.path(),
        &zeroize::Zeroizing::new("status test password".into()),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let auth = std::sync::Arc::new(yinz_web::auth::Auth::new(
        t.path().into(),
        yinz_web::auth::AuthConfig::default(),
        std::sync::Arc::new(SystemClock),
    ));
    let response = rt.block_on(auth.login(
        zeroize::Zeroizing::new(
            br#"{"password":"status test password","label":"Status Phone"}"#.to_vec(),
        ),
        None,
        "test",
    ));
    assert_eq!(response.status, 200);
    let token = response.json["token"].as_str().unwrap();
    let password::PasswordState::Ready(verifier) = password::read_state(t.path()) else {
        panic!()
    };
    let output = invoke();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--https=8443"));
    assert!(text.contains("https://box.ts.net:8443"));
    assert!(text.contains("WARNING: Funnel"));
    assert!(text.contains("Status Phone"));
    assert!(!text.contains(token));
    assert!(!text.contains(&verifier.phc));
    assert!(!text.contains(&verifier.verifier_id));
    assert!(!text.contains("status test password"));
}
