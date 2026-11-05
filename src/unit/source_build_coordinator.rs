use super::*;
use std::fs;

struct BuildFixture {
    store: Store,
    inventory: Inventory,
    _root: tempfile::TempDir,
}

impl BuildFixture {
    fn new(probe: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let store = Store::open_with_source(
            &root.path().join("session"),
            &root.path().join("data"),
            &operation,
            true,
        )
        .unwrap();
        let build = store.directory.join("source");
        fs::create_dir(&build).unwrap();
        fs::create_dir(build.join("checkout")).unwrap();
        journal::atomic_json(
            &build.join("build.json"),
            &serde_json::json!({
                "operation": operation,
                "repository": root.path(),
                "commit": "pinned",
                "version": "1.0.0",
                "snapshot": build.join("checkout"),
                "snapshot_ready": true,
                "targets": ["x86_64-unknown-linux-gnu"],
                "log": build.join("build.log"),
                "artifacts": {},
                "sha256": {},
                "pane": "terminal_42"
            }),
        )
        .unwrap();
        journal::atomic_json(
            &build.join("launch.json"),
            &serde_json::json!({"operation": operation}),
        )
        .unwrap();
        fs::write(build.join("build.log"), "retained build output\n").unwrap();
        let executable = root.path().join("yinz");
        // Refuse any attempt to launch a second worker or close the retained pane.
        yinz_test_support::write_warm_script(
            &executable,
            &format!("case \"$*\" in *'action list-panes --json') {probe} ;; *) exit 1 ;; esac"),
        );
        let inventory = Inventory {
            session: "session".into(),
            server_pid: std::process::id(),
            server_instance: "source".into(),
            executable,
            running_version: "1.0.0".into(),
            running_binary_sha256: None,
            assets: Default::default(),
            configuration: serde_json::json!({}),
            clients: vec![],
        };
        Self {
            store,
            inventory,
            _root: root,
        }
    }

    fn fail(&mut self) -> anyhow::Error {
        let error = source_build::build(
            &self.store,
            &self.inventory,
            &["x86_64-unknown-linux-gnu".into()],
        )
        .unwrap_err()
        .context("build source release");
        record_source_build_failure(&mut self.store, &error).unwrap();
        error
    }

    fn assert_retry(self, phase: Phase) {
        let persisted: Journal =
            serde_json::from_slice(&fs::read(self.store.directory.join("journal.json")).unwrap())
                .unwrap();
        assert_eq!(persisted.phase, phase);
        let old = self.store.journal.operation.clone();
        let socket = self.store.journal.source_socket.clone();
        drop(self.store);
        let new = uuid::Uuid::new_v4().to_string();
        let retry =
            Store::open_with_source(&socket, &self._root.path().join("data"), &new, true).unwrap();
        assert_eq!(
            retry.journal.operation,
            if phase == Phase::Discover { old } else { new }
        );
        assert_eq!(retry.journal.phase, Phase::Discover);
    }
}

#[test]
fn exited_source_worker_without_receipt_finalizes_operation() {
    let mut fixture = BuildFixture::new("printf '%s' '[{\"id\":42,\"exited\":true}]'");
    let error = fixture.fail();
    assert!(error.is::<source_build::TerminalFailure>());
    assert!(format!("{error:#}").contains("worker exited without a result"));
    let build = fixture.store.directory.join("source");
    assert!(!build.join("result.json").exists());
    assert!(!build.join("cancel").exists());
    assert_eq!(
        fs::read_to_string(build.join("build.log")).unwrap(),
        "retained build output\n"
    );
    fixture.assert_retry(Phase::Failed);
}

#[test]
fn closed_source_pane_finalizes_operation_with_or_without_receipt() {
    for receipt in [None, Some(true), Some(false)] {
        let mut fixture = BuildFixture::new("printf '%s' '[]'");
        if let Some(success) = receipt {
            journal::atomic_json(
                &fixture.store.directory.join("source/result.json"),
                &serde_json::json!({"success": success}),
            )
            .unwrap();
        }
        let error = fixture.fail();
        assert!(error.is::<source_build::TerminalFailure>());
        assert!(format!("{error:#}").contains("pane was closed"));
        fixture.assert_retry(Phase::Failed);
    }
}

#[test]
fn source_worker_cleanup_failure_keeps_operation_resumable() {
    let mut fixture = BuildFixture::new("printf '%s' '[{\"id\":42,\"exited\":true}]'");
    fs::write(
        fixture.store.directory.join("source/process.json"),
        "invalid",
    )
    .unwrap();
    let error = fixture.fail();
    assert!(!error.is::<source_build::TerminalFailure>());
    assert!(format!("{error:#}").contains("source-update command failed"));
    fixture.assert_retry(Phase::Discover);
}

#[test]
fn source_worker_probe_failure_keeps_operation_resumable() {
    let mut fixture = BuildFixture::new("exit 1");
    let error = fixture.fail();
    assert!(!error.is::<source_build::TerminalFailure>());
    fixture.assert_retry(Phase::Discover);
}

#[test]
fn ambiguous_source_worker_launch_keeps_operation_resumable() {
    let mut fixture = BuildFixture::new("exit 1");
    let record_path = fixture.store.directory.join("source/build.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["pane"] = serde_json::Value::Null;
    journal::atomic_json(&record_path, &record).unwrap();
    let error = fixture.fail();
    assert!(format!("{error:#}").contains("launch was interrupted"));
    fixture.assert_retry(Phase::Discover);
}

#[test]
fn interrupted_source_build_finalizes_as_failed_despite_obsolete_markers() {
    let mut fixture = BuildFixture::new("printf '%s' '[{\"id\":42,\"exited\":true}]'");
    fs::write(fixture.store.directory.join("cancel"), "").unwrap();
    fs::write(fixture.store.directory.join("source/cancel"), "").unwrap();
    journal::atomic_json(
        &fixture.store.directory.join("source/result.json"),
        &serde_json::json!({"success": false, "error": "source build interrupted"}),
    )
    .unwrap();
    let error = fixture.fail();
    assert!(format!("{error:#}").contains("source build interrupted"));
    assert!(fixture.store.directory.join("source/build.log").is_file());
    fixture.assert_retry(Phase::Failed);
}

#[test]
fn refusal_after_a_finished_source_build_is_terminal() {
    let mut fixture = BuildFixture::new("exit 1");
    journal::atomic_json(
        &fixture.store.directory.join("source/result.json"),
        &serde_json::json!({"success":true}),
    )
    .unwrap();
    record_discovery_failure(&mut fixture.store, &anyhow!("invalid source artifact")).unwrap();
    fixture.assert_retry(Phase::Failed);
}

#[test]
fn failed_source_receipt_still_finalizes_operation() {
    let mut fixture = BuildFixture::new("printf '%s' '[{\"id\":42,\"exited\":true}]'");
    journal::atomic_json(
        &fixture.store.directory.join("source/result.json"),
        &serde_json::json!({"success": false, "error": "compiler failed"}),
    )
    .unwrap();
    let error = fixture.fail();
    assert!(format!("{error:#}").contains("compiler failed"));
    fixture.assert_retry(Phase::Failed);
}
