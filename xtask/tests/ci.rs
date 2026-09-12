use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn gate_runs_integration_and_aggregates_failures() {
    let root = tempfile::tempdir().unwrap();
    for (out, src, generated) in [
        (
            "zellij-plugin-api/assets",
            "zellij-plugin-api/src",
            "generated_plugin_api.rs",
        ),
        (
            "zellij-ipc/assets/prost_ipc",
            "zellij-ipc/src/client_server_contract",
            "generated_client_server_api.rs",
        ),
        (
            "zellij-ipc/assets/prost_nested_session",
            "zellij-ipc/src/nested_session_contract",
            "generated_nested_session_api.rs",
        ),
    ] {
        let assets = root.path().join(out);
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join(generated), "").unwrap();
        fs::create_dir_all(root.path().join(src)).unwrap();
    }
    let libraries = [
        "zellij-types",
        "zellij-runtime",
        "zellij-storage",
        "zellij-config",
        "zellij-plugin-api",
        "zellij-ipc",
        "zellij-session",
        "zellij-cli",
        "zellij-terminal",
        "zellij-process",
        "zellij-bus",
        "zellij-routing",
        "zellij-plugin-host",
        "zellij-panes",
    ];
    let metadata = root.path().join("metadata.json");
    fs::write(&metadata, serde_json::json!({
        "packages": libraries.iter().map(|name| serde_json::json!({"id": name, "name": name, "dependencies": []})).collect::<Vec<_>>(),
        "workspace_members": libraries
    }).to_string()).unwrap();
    let cargo = root.path().join("cargo-stub");
    fs::write(
        &cargo,
        r#"#!/bin/sh
printf '%s|%s\n' "$NEXTEST_TEST_THREADS" "$*" >> "$GATE_LOG"
case "$*" in
  'metadata '*) cat "$GATE_METADATA" ;;
  'fmt '*|'clippy '*|'nextest run '*) exit "$GATE_FAILURE" ;;
  'nextest --version') exit "$GATE_NO_NEXTEST" ;;
esac
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    for (failure, no_nextest) in [("0", "0"), ("1", "0"), ("0", "1")] {
        let log = root.path().join("calls");
        fs::write(&log, "").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["ci", "check"])
            .env("CARGO", &cargo)
            .env("CARGO_MANIFEST_DIR", root.path().join("xtask"))
            .env("NEXTEST_TEST_THREADS", "3")
            .env("GATE_LOG", &log)
            .env("GATE_METADATA", &metadata)
            .env("GATE_FAILURE", failure)
            .env("GATE_NO_NEXTEST", no_nextest)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.success(),
            failure == "0",
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.contains("3|fmt --all --check"), "{calls}");
        assert!(calls.contains("3|clippy --workspace"), "{calls}");
        if no_nextest == "0" {
            assert!(calls.contains("3|nextest run --profile unit"), "{calls}");
            assert!(
                calls.contains("16|nextest run -p zellij-integration-tests\n"),
                "{calls}"
            );
        } else {
            assert!(
                calls.contains("16|test -p zellij-integration-tests -- --test-threads=1\n"),
                "{calls}"
            );
        }
        if failure == "1" {
            assert!(
                stdout.contains(">> Failed: format --check, lint, test, integration-test"),
                "{stdout}"
            );
        }
    }
}
