use super::*;

#[test]
fn source_update_status_keeps_pinned_identity_and_build_log_visible() {
    let root = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let active = serde_json::json!({"source_build":true,"operation":operation,"phase":"Discover"});
    assert!(source_build_status(&active, root.path())
        .unwrap()
        .contains("waiting to pin"));
    let directory = root.path().join("updates").join(&operation).join("source");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("build.json"),
        serde_json::to_vec(&serde_json::json!({
            "commit":"pinned-commit",
            "targets":["x86_64-unknown-linux-gnu","aarch64-unknown-linux-gnu"],
            "log":"/operation/build.log",
        }))
        .unwrap(),
    )
    .unwrap();
    let status = source_build_status(&active, root.path()).unwrap();
    for required in [
        "Building local main",
        "pinned-commit",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "/operation/build.log",
    ] {
        assert!(status.contains(required), "{status}");
    }
    assert_eq!(
        source_build_status(&serde_json::json!({"source_build":false}), root.path()).unwrap(),
        ". Channel: stable; version: pending"
    );
}

#[test]
fn prerelease_update_status_names_the_pinned_version_and_channel() {
    let root = tempfile::tempdir().unwrap();
    let active =
        serde_json::json!({"channel":"Prerelease", "release":{"tag_name":"v0.1.2-alpha.1"}});
    let status = source_build_status(&active, root.path()).unwrap();
    assert!(status.contains("including prereleases"));
    assert!(status.contains("v0.1.2-alpha.1"));
}
