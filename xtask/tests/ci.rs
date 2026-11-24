use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn gate_runs_integration_and_aggregates_failures() {
    let root = tempfile::tempdir().unwrap();
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    fs::create_dir_all(root.path().join("target/debug")).unwrap();
    fs::create_dir_all(root.path().join("example-plugins")).unwrap();
    fs::create_dir_all(root.path().join("yinz-steel-builtins/assets/plugins")).unwrap();
    fs::write(
        root.path().join("example-plugins/main.scm"),
        "(define value 1)\n",
    )
    .unwrap();
    let binary = root.path().join("target/debug/yinz");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GATE_LOG\"\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    for (out, src, generated) in [
        (
            "yinz-ipc/assets/prost_ipc",
            "yinz-ipc/src/client_server_contract",
            "generated_client_server_api.rs",
        ),
        (
            "yinz-host-protocol/assets/prost_nested_session",
            "yinz-ipc/src/nested_session_contract",
            "generated_nested_session_api.rs",
        ),
    ] {
        let assets = root.path().join(out);
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join(generated), "").unwrap();
        fs::create_dir_all(root.path().join(src)).unwrap();
    }
    // The gate starts with the real dependency policy, which requires the crates it names to
    // exist: give it this checkout's members, without the edges metadata.rs's own tests cover.
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(checkout.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut members: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for package in members["packages"].as_array_mut().unwrap() {
        package["dependencies"] = serde_json::json!([]);
    }
    let metadata = root.path().join("metadata.json");
    fs::write(&metadata, members.to_string()).unwrap();
    let cargo = root.path().join("cargo-stub");
    fs::write(
        &cargo,
        r#"#!/bin/sh
printf '%s|%s\n' "$NEXTEST_TEST_THREADS" "$*" >> "$GATE_LOG"
case "$*" in
  'metadata '*) cat "$GATE_METADATA" ;;
  'tree -q -p yinz -e '*) [ "$GATE_FAILURE" = 1 ] && echo 'nix v0.30.1 [ioctl]' || echo 'nix v0.30.1 []' ;;
  'tree -q -p yinz-integration-tests '*) echo 'nix v0.30.1 []' ;;
  'nextest run --profile unit'*|'test --no-fail-fast'*|'test --doc'*)
    [ "$YINZ_SOCKET_DIR" = "$YINZ_TEST_CACHE_ROOT/sockets" ] || exit 99
    exit "$GATE_FAILURE" ;;
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
        assert!(
            calls.contains("3|tree -q -p yinz -e normal,build --prefix none"),
            "{calls}"
        );
        assert!(
            calls.contains("3|tree -q -p yinz-integration-tests -e normal,build,dev --prefix none"),
            "{calls}"
        );
        assert!(calls.contains("3|fmt --all --check"), "{calls}");
        assert!(calls.contains("3|clippy --workspace"), "{calls}");
        if failure == "0" {
            assert!(calls.contains("plugins check --file"), "{calls}");
            assert!(calls.contains("plugins format --file"), "{calls}");
        }
        if no_nextest == "0" {
            assert!(calls.contains("3|nextest run --profile unit"), "{calls}");
            assert!(
                calls.contains("16|nextest run -p yinz-integration-tests\n"),
                "{calls}"
            );
        } else {
            assert!(
                calls.contains("16|test -p yinz-integration-tests -- --test-threads=1\n"),
                "{calls}"
            );
        }
        if failure == "1" {
            assert!(
                stdout.contains(
                    ">> Failed: integration-test features, format --check, lint, test, integration-test"
                ),
                "{stdout}"
            );
        }
    }
}

#[test]
fn live_proto_targets_generate() {
    let root = tempfile::tempdir().unwrap();
    let targets = [
        (
            "yinz-ipc/src/client_server_contract",
            "yinz-ipc/assets/prost_ipc",
            "generated_client_server_api.rs",
            "client_fixture",
            "ClientMessage",
        ),
        (
            "yinz-ipc/src/nested_session_contract",
            "yinz-host-protocol/assets/prost_nested_session",
            "generated_nested_session_api.rs",
            "nested_fixture",
            "NestedMessage",
        ),
    ];
    for (src, _, _, package, message) in targets {
        let src = root.path().join(src);
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("fixture.proto"),
            format!(r#"syntax = "proto3"; package {package}; message {message} {{}}"#),
        )
        .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("proto")
        .env("CARGO_MANIFEST_DIR", root.path().join("xtask"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (_, out, include, package, message) in targets {
        let out = root.path().join(out);
        let index = fs::read_to_string(out.join(include)).unwrap();
        assert!(index.contains(&format!("pub mod {package} {{")));
        assert!(index.contains(&format!(r#"include!("{package}.rs");"#)));
        let definitions = fs::read_to_string(out.join(format!("{package}.rs"))).unwrap();
        assert!(definitions.contains(&format!("pub struct {message} {{")));
    }
    assert!(!root.path().join("yinz-plugin-api").exists());
}
