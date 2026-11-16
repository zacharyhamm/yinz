use super::*;
use yinz_types::{
    internal_ids::{PluginId, TabId, TerminalId},
    pane_id::PaneId,
    session_migration::*,
};

#[test]
fn cold_release_discards_destination_mappings_and_preserves_terminal_state() {
    let root = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let store = Store::open(&root.path().join("session"), root.path(), &operation).unwrap();
    let tab = TabId::new(1);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(PluginId::new(2));
    let snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: store.journal.operation.clone(),
        inventory: SessionMigrationInventory {
            format: SESSION_MIGRATION_FORMAT,
            session_name: "session".into(),
            instance_id: "source".into(),
            tabs: vec![TabInventory {
                tab_id: tab,
                title: "retained".into(),
                panes: vec![terminal, plugin],
            }],
            clients: vec![],
            remote_hosts: vec![],
            global_last_active_tab: tab,
            tab_group_preferences: vec![],
            paneless_plugins: vec![],
            display_size: None,
        },
        phase: MigrationPhase::Adopted,
        tabs: vec![MigrationTab {
            source_tab_id: tab,
            slots: vec![terminal, plugin],
            payload: Default::default(),
            terminal_states: vec![(terminal, serde_json::json!({"retained": "terminal state"}))],
            local_replay: vec![(terminal, b"retained output".to_vec())],
            state: TabState {
                focus_pane: Some(plugin),
                fullscreen: Some(plugin),
                ..Default::default()
            },
        }],
        mapping: SessionMigrationMap {
            tabs: vec![(tab, TabId::new(9))],
            panes: vec![
                (terminal, PaneId::Terminal(TerminalId::new(8))),
                (plugin, PaneId::Plugin(PluginId::new(9))),
            ],
        },
        errors: vec!["replacement exited".into()],
        ownership: vec![(
            terminal,
            MigrationOwnership::Destination {
                terminal_id: TerminalId::new(8),
            },
        )],
    };
    journal::atomic_json(&store.journal.snapshot, &snapshot).unwrap();
    release_cold_snapshot(&store).unwrap();
    let released = read_recovery_snapshot(&store.journal.snapshot).unwrap();
    assert_eq!(released.phase, MigrationPhase::Released);
    assert_eq!(released.mapping, SessionMigrationMap::default());
    assert!(released.errors.is_empty());
    assert_eq!(
        released.ownership,
        vec![(terminal, MigrationOwnership::Released)]
    );
    assert_eq!(
        serde_json::to_value(&released.tabs).unwrap(),
        serde_json::to_value(&snapshot.tabs).unwrap()
    );
    assert_eq!(released.inventory, snapshot.inventory);
}

#[test]
fn retired_update_releases_claim_and_allows_another_installation() {
    recover_retired_source_update(true, false);
}

#[test]
fn retired_update_explicit_recovery_restores_missing_socket_directory() {
    recover_retired_source_update(false, false);
}

#[test]
fn retired_update_source_cleanup_failure_is_retryable() {
    recover_retired_source_update(false, true);
}

fn recover_retired_source_update(automatic: bool, fail_cleanup: bool) {
    use std::fs;
    use update::transaction::Transaction;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let binary = root.path().join("yinz");
    let release = root.path().join("release");
    fs::create_dir_all(release.join("plugins/builtins")).unwrap();
    fs::write(&binary, "old").unwrap();
    fs::write(release.join("yinz"), "new").unwrap();
    fs::write(release.join("plugins/builtins/test.scm"), "new plugin").unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut transaction = Transaction::prepare(&release, &binary, &data, &operation).unwrap();
    transaction.publish().unwrap();
    drop(transaction);
    let runtime = root.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    let mut store =
        Store::open_with_source(&runtime.join("session"), &data, &operation, true).unwrap();
    let checkout = store.directory.join("source/checkout");
    let artifacts = store.directory.join("source-artifacts");
    fs::create_dir_all(checkout.join("target")).unwrap();
    fs::write(checkout.join("target/build-output"), "build output").unwrap();
    if fail_cleanup {
        // A file at the directory path reliably fails removal, even as root.
        fs::write(&artifacts, "obstruction").unwrap();
    } else {
        fs::create_dir(&artifacts).unwrap();
        fs::write(artifacts.join("package"), "packaged artifacts").unwrap();
    }
    let journal_path = store.directory.join("journal.json");
    // Use a reaped child, not a guessed nonexistent PID.
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let identity = yinz_types::session_update::ServerIdentity {
        server_pid: pid,
        server_instance: "retired".into(),
    };
    store.journal.source = Some(Inventory {
        session: "session".into(),
        server_pid: pid,
        server_instance: "retired".into(),
        executable: binary.clone(),
        running_version: "1.0.0".into(),
        running_binary_sha256: None,
        assets: Default::default(),
        configuration: serde_json::Value::String(String::new()),
        themes: None,
        clients: vec![],
    });
    store.journal.replacement_identity = Some(identity);
    store.journal.handoff_committed = true;
    store.journal.participants.push(Participant {
        installation: Installation {
            host: "local".into(),
            executable: binary.clone(),
            data_dir: data.clone(),
            running_version: "1.0.0".into(),
            installed_version: "2.0.0".into(),
            target: "test".into(),
            capabilities: capabilities(),
        },
        route: Route::Server,
        state: "published".into(),
    });
    store.phase(Phase::RecoveryRequired).unwrap();
    // A still-live recorded process must keep the backup and ownership claim.
    store
        .journal
        .replacement_identity
        .as_mut()
        .unwrap()
        .server_pid = std::process::id();
    assert!(finish_retired(&mut store, None).is_err());
    assert_eq!(
        Transaction::open(&binary, &data, &operation)
            .unwrap()
            .status(),
        update::transaction::Status::Published
    );
    store
        .journal
        .replacement_identity
        .as_mut()
        .unwrap()
        .server_pid = pid;
    store.save().unwrap();
    drop(store);
    fs::remove_dir(&runtime).unwrap();
    if fail_cleanup {
        let error = crate::live_update::recover_operation(&data, &operation).unwrap_err();
        assert!(format!("{error:#}").contains("clean committed source build artifacts"));
        let journal: Journal = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert_ne!(journal.phase, Phase::Complete);
        fs::remove_file(&artifacts).unwrap();
    }
    if automatic {
        update::transaction::recover_standalone(&binary, &data).unwrap();
    } else {
        crate::live_update::recover_operation(&data, &operation).unwrap();
    }
    assert!(runtime.is_dir());
    assert!(!checkout.exists());
    assert!(!artifacts.exists());
    let journal: Journal = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(journal.phase, Phase::Complete);
    assert_eq!(fs::read_to_string(&binary).unwrap(), "new");
    let mut retry = Transaction::prepare(&release, &binary, &data, "standalone-retry").unwrap();
    retry.rollback().unwrap();
}
