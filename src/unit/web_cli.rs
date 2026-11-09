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

#[test]
fn dual_stack_funnel_ambiguity_and_confirmed_match_precedence() {
    let wildcard = "[::]:7419".parse().unwrap();
    let ipv4 = "127.0.0.1:7419".parse().unwrap();
    for (proxy, target, expected) in [
        ("http://127.0.0.1:7419", wildcard, Exposure::PossiblyExposed),
        (
            "http://127.0.0.2:7419/path",
            wildcard,
            Exposure::PossiblyExposed,
        ),
        ("http://localhost:7419", wildcard, Exposure::PossiblyExposed),
        ("http://127.0.0.1:7420", wildcard, Exposure::NoMatch),
        (
            "http://[::1]:7419",
            "0.0.0.0:7419".parse().unwrap(),
            Exposure::NoMatch,
        ),
        ("http://192.0.2.1:7419", wildcard, Exposure::NoMatch),
    ] {
        let config = serde_json::json!({"Web":{"box:8443":{"Handlers":{"/path":{"Proxy":proxy}}}},"AllowFunnel":{"box:8443":true}});
        assert_eq!(exposure(&config, &[target]), expected);
        assert_eq!(
            exposure(&serde_json::json!({"Foreground":{"id":config}}), &[target]),
            expected
        );
        let mut disabled = config;
        disabled["AllowFunnel"]["box:8443"] = false.into();
        assert_eq!(exposure(&disabled, &[target]), Exposure::NoMatch);
    }
    let config = serde_json::json!({"Web":{"box:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7419"}}}},"AllowFunnel":{"box:443":true}});
    assert_eq!(exposure(&config, &[wildcard, ipv4]), Exposure::Exposed);
}

#[test]
fn cli_possible_dual_stack_funnel_warns_and_exits_one() {
    let t = tempfile::tempdir().unwrap();
    let config = t.path().join("config.kdl");
    std::fs::write(
        &config,
        "web { enabled true; address \"::\"; allow_non_loopback true; port 7419; }\n",
    )
    .unwrap();
    let stub = t.path().join("tailscale");
    for (proxy, allow, code, message) in [
        (
            "http://127.0.0.1:7419",
            true,
            1,
            "may reach this IPv6 wildcard listener",
        ),
        (
            "http://localhost:7419/path",
            true,
            1,
            "reachability is unverified",
        ),
        ("http://[::1]:7419", true, 1, "Funnel exposes"),
        ("http://127.0.0.1:22", true, 0, "No matching Funnel mapping"),
        (
            "http://127.0.0.1:7419",
            false,
            0,
            "No matching Funnel mapping",
        ),
    ] {
        for foreground in [false, true] {
            let mut serve = serde_json::json!({"Web":{"box:8443":{"Handlers":{"/":{"Proxy":proxy}}}},"AllowFunnel":{"box:8443":allow}});
            if foreground {
                serve = serde_json::json!({"Foreground":{"id":serve}});
            }
            yinz_test_support::write_warm_script(
                &stub,
                &format!(
                    "if [ \"$1\" = serve ]; then printf '%s' '{serve}'; else printf '{{}}'; fi"
                ),
            );
            let output = std::process::Command::new(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz"),
            )
            .env("PATH", t.path())
            .arg("--data-dir")
            .arg(t.path())
            .arg("--config")
            .arg(&config)
            .args(["web", "status"])
            .output()
            .unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(output.status.code(), Some(code), "{text}");
            assert!(text.contains(message), "{text}");
            if code == 1 {
                assert!(!text.contains("No matching Funnel mapping"), "{text}");
            }
        }
    }
}

#[test]
fn cli_same_name_stale_policy_is_unverified_against_native_listener() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt, sync::Arc};
    struct Native(yinz_test_support::Watched<bool>);
    impl yinz_web::Backend for Native {
        fn dispatch(&self, _: yinz_types::web::WebRequest) -> yinz_web::DispatchFuture {
            Box::pin(async { None })
        }
        fn site(&self) -> Option<Arc<yinz_types::web::Site>> {
            None
        }
        fn quiesced(&self) -> bool {
            false
        }
        fn activate(&self) {
            *self.0.lock() = true;
        }
        fn session_name(&self) -> String {
            "reused-name".into()
        }
    }
    let t = tempfile::tempdir().unwrap();
    password::write(
        t.path(),
        &zeroize::Zeroizing::new("native status password".into()),
    )
    .unwrap();
    let config = WebConfig {
        port: Some(0),
        tailscale_users: vec!["current@example.com".into()],
        ..Default::default()
    };
    let settings = yinz_web::WebSettings::from_config(&config, t.path()).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let active = yinz_test_support::Watched::new(false);
    let server =
        yinz_web::WebServer::start(settings, Arc::new(Native(active.clone())), rt.handle());
    active.wait_until("native listener activated", Duration::from_secs(15), |v| {
        v.then_some(())
    });
    let rows = yinz_web::listener_file::inspect(t.path()).unwrap();
    assert!(rows[0].is_current());
    let mut stale = rows.into_iter().next().unwrap().record;
    stale.instance = if stale.instance == "a".repeat(32) {
        "b".repeat(32)
    } else {
        "a".repeat(32)
    };
    stale.pid = u32::MAX;
    stale.startup_config.as_mut().unwrap().tailscale_users = vec!["obsolete@example.com".into()];
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(t.path().join("web/listeners/stale.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(&stale).unwrap())
        .unwrap();
    drop(file);
    let config_file = t.path().join("config.kdl");
    std::fs::write(&config_file, "web { port 0; }\n").unwrap();
    yinz_test_support::write_warm_script(&t.path().join("tailscale"), "printf '{}'");
    let output =
        std::process::Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz"))
            .env("PATH", t.path())
            .arg("--data-dir")
            .arg(t.path())
            .arg("--config")
            .arg(&config_file)
            .args(["web", "status"])
            .output()
            .unwrap();
    // Stop and await all listener work before assertions about command output.
    server.stop();
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            while server.status() != yinz_web::WebStatus::Stopped {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    });
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.matches("Effective startup config:").count(),
        1,
        "{text}"
    );
    assert!(text.contains("current@example.com"), "{text}");
    assert!(text.contains("Effective policy: unverified"), "{text}");
    assert!(!text.contains("obsolete@example.com"), "{text}");
    assert_eq!(
        text.matches("Suggested: tailscale serve").count(),
        1,
        "{text}"
    );
    assert!(!text.contains("configured-only"), "{text}");
}

#[test]
fn local_guidance_preserves_paths_and_rejects_unrepresentable_commands() {
    use std::os::unix::ffi::OsStringExt;
    let opts = CliArgs {
        config: Some("/tmp/config's $x.kdl".into()),
        config_dir: Some("/tmp/config dir".into()),
        remote: Some("excluded".into()),
        ..Default::default()
    };
    assert_eq!(suggested_command(&opts, Path::new("/tmp/data's `cmd`"), &["web", "status"]), "yinz --data-dir '/tmp/data'\\''s `cmd`' --config '/tmp/config'\\''s $x.kdl' --config-dir '/tmp/config dir' web status");
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
    assert!(suggested_command(&opts, &path, &["set-password"]).starts_with("manually run"));
    assert!(
        suggested_command(&opts, Path::new("/tmp/data\nnext"), &["set-password"])
            .starts_with("manually run")
    );
}
