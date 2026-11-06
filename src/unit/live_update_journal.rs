use super::super::journal::{Phase, Store};

#[test]
fn relative_data_directory_and_interrupted_snapshot_become_absolute() {
    use yinz_types::session_migration::{
        MigrationPhase, SessionMigrationInventory, SessionMigrationSnapshot,
        SESSION_MIGRATION_FORMAT,
    };
    let cwd = std::env::current_dir().unwrap();
    let root = tempfile::tempdir_in(&cwd).unwrap();
    let data = root.path().strip_prefix(&cwd).unwrap().join("data");
    let socket = root.path().join("session");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open(&socket, &data, &operation).unwrap();
    assert!(store.directory.is_absolute());
    assert_eq!(
        store.journal.snapshot,
        store.directory.join("snapshot.json")
    );
    let snapshot = SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: operation.clone(),
        inventory: SessionMigrationInventory {
            format: SESSION_MIGRATION_FORMAT,
            session_name: "session".into(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            tabs: vec![],
            clients: vec![],
            remote_hosts: vec![],
            global_last_active_tab: yinz_types::internal_ids::TabId::new(0),
            tab_group_preferences: vec![],
            paneless_plugins: vec![],
            display_size: None,
        },
        phase: MigrationPhase::Prepared,
        tabs: vec![],
        mapping: Default::default(),
        errors: vec![],
        ownership: vec![],
    };
    yinz_bus::migration::save_snapshot(&store.journal.snapshot, &snapshot).unwrap();
    // Model an interrupted journal written before absolute paths were required.
    store.journal.snapshot = data.join("updates").join(&operation).join("snapshot.json");
    store.phase(Phase::Prepare).unwrap();
    drop(store);
    let store = Store::open(&socket, &data, &uuid::Uuid::new_v4().to_string()).unwrap();
    assert_eq!(store.journal.operation, operation);
    assert!(store.directory.is_absolute());
    assert!(store.journal.snapshot.is_absolute());
    yinz_bus::migration::save_snapshot(&store.journal.snapshot, &snapshot).unwrap();
    let persisted: super::super::journal::Journal =
        serde_json::from_slice(&std::fs::read(store.directory.join("journal.json")).unwrap())
            .unwrap();
    assert_eq!(persisted.format, 1);
    assert_eq!(persisted.snapshot, store.journal.snapshot);
}

#[test]
fn interrupted_update_reuses_pinned_operation_and_holds_session_lock() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut original = Store::open(&socket, &data, &operation).unwrap();
    original.journal.release = Some(serde_json::json!({"tag_name":"v1.2.3","assets":[]}));
    original.phase(Phase::Transfer).unwrap();
    assert!(Store::open(&socket, &data, &uuid::Uuid::new_v4().to_string()).is_err());
    drop(original);
    let retry = Store::open(&socket, &data, &uuid::Uuid::new_v4().to_string()).unwrap();
    assert_eq!(retry.journal.operation, operation);
    assert_eq!(retry.journal.phase, Phase::Transfer);
    assert_eq!(retry.journal.release.unwrap()["tag_name"], "v1.2.3");
}

#[test]
fn completed_update_allows_new_operation_and_old_acknowledgements_stay_separate() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let old = uuid::Uuid::new_v4().to_string();
    let new = uuid::Uuid::new_v4().to_string();
    let mut original = Store::open(&socket, &data, &old).unwrap();
    original.journal.handoff_committed = true;
    original.phase(Phase::Complete).unwrap();
    drop(original);
    let next = Store::open(&socket, &data, &new).unwrap();
    assert_eq!(next.journal.operation, new);
    assert_eq!(next.journal.phase, Phase::Discover);
    assert!(!next.journal.handoff_committed);
    let old_record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(data.join("updates").join(old).join("journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(old_record["phase"], "Complete");
}

#[test]
fn socket_aliases_cannot_start_concurrent_updates() {
    let root = tempfile::tempdir().unwrap();
    let sockets = root.path().join("sockets");
    std::fs::create_dir(&sockets).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&sockets, &alias).unwrap();
    let data = root.path().join("data");
    let _owner = Store::open(
        &sockets.join("session"),
        &data,
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(Store::open(
        &alias.join("session"),
        &data,
        &uuid::Uuid::new_v4().to_string()
    )
    .is_err());
}

#[test]
fn obsolete_cancellation_marker_does_not_block_transfer() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open(&socket, &data, &operation).unwrap();
    store.phase(Phase::Prepare).unwrap();
    std::fs::write(store.directory.join("cancel"), b"").unwrap();
    drop(store);
    let mut retry = Store::open(&socket, &data, &operation).unwrap();
    assert_eq!(retry.journal.phase, Phase::Prepare);
    retry.phase(Phase::Transfer).unwrap();
    assert_eq!(retry.journal.phase, Phase::Transfer);
}

#[test]
fn historical_cancelled_journals_are_failed_and_allow_a_new_update() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let store = Store::open(&socket, &data, &operation).unwrap();
    let mut old = serde_json::to_value(&store.journal).unwrap();
    old["phase"] = "Cancelled".into();
    std::fs::write(
        store.directory.join("journal.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let decoded: super::super::journal::Journal = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.phase, Phase::Failed);
    assert_eq!(serde_json::to_value(decoded).unwrap()["phase"], "Failed");
    assert_eq!(
        yinz_runtime::session_update::read_active(&data, &socket)
            .unwrap()
            .unwrap()["phase"],
        "Failed"
    );
    drop(store);
    let next = uuid::Uuid::new_v4().to_string();
    let retry = Store::open(&socket, &data, &next).unwrap();
    assert_eq!(retry.journal.operation, next);
    assert_eq!(retry.journal.phase, Phase::Discover);
}

#[test]
fn retry_before_inventory_rejects_a_different_source_mode() {
    for source_build in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("session");
        let data = root.path().join("data");
        let operation = uuid::Uuid::new_v4().to_string();
        let original = Store::open_with_source(&socket, &data, &operation, source_build).unwrap();
        assert_eq!(original.journal.phase, Phase::Discover);
        assert!(original.journal.source.is_none());
        drop(original);
        // A different palette action cannot silently resume the old mode.
        let error = Store::open_with_source(
            &socket,
            &data,
            &uuid::Uuid::new_v4().to_string(),
            !source_build,
        )
        .err()
        .unwrap();
        assert!(error
            .to_string()
            .contains("update is already in progress; cancel it first"));
        let retry = Store::open_with_source(&socket, &data, &operation, source_build).unwrap();
        assert_eq!(retry.journal.operation, operation);
        assert_eq!(retry.journal.source_build, source_build);
        assert!(retry.journal.release.is_none());
        assert!(retry.journal.source.is_none());
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(retry.directory.join("journal.json")).unwrap())
                .unwrap();
        assert_eq!(stored["source_build"], source_build);
    }
}

#[test]
fn old_journals_default_to_stable_and_reject_prerelease_starts() {
    use yinz_types::session_update::ReleaseChannel;
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let store = Store::open(&socket, &data, &operation).unwrap();
    let path = store.directory.join("journal.json");
    let mut old = serde_json::to_value(&store.journal).unwrap();
    old.as_object_mut().unwrap().remove("channel");
    std::fs::write(path, serde_json::to_vec(&old).unwrap()).unwrap();
    drop(store);
    let error = Store::open_with_channel(
        &socket,
        &data,
        &uuid::Uuid::new_v4().to_string(),
        false,
        ReleaseChannel::Prerelease,
    )
    .err()
    .unwrap();
    assert!(error
        .to_string()
        .contains("stable update is already in progress; cancel it first"));
    let resumed = Store::open(&socket, &data, &operation).unwrap();
    assert_eq!(resumed.journal.channel, ReleaseChannel::Stable);
    assert_eq!(resumed.journal.operation, operation);
}

/// A replacement starts at the size the session had when it froze, not the size its source
/// server's first client had at startup - which no resize ever updates.
#[test]
fn replacement_assets_start_at_the_frozen_session_size() {
    use yinz_types::{
        pane_size::Size,
        session_migration::{SessionMigrationInventory, SESSION_MIGRATION_FORMAT},
        session_update::Inventory,
    };
    let root = tempfile::tempdir().unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open(
        &root.path().join("session"),
        &root.path().join("data"),
        &operation,
    )
    .unwrap();
    let startup = Size {
        rows: 86,
        cols: 317,
    };
    store.journal.source = Some(Inventory {
        session: "session".into(),
        server_pid: 1,
        server_instance: uuid::Uuid::new_v4().to_string(),
        executable: "/usr/bin/true".into(),
        running_version: "0.0.0".into(),
        running_binary_sha256: None,
        assets: yinz_types::input::cli_assets::CliAssets {
            terminal_window_size: startup,
            supports_in_place_update: true,
            ..Default::default()
        },
        configuration: serde_json::Value::String(String::new()),
        clients: vec![],
    });
    let frozen = |display_size| SessionMigrationInventory {
        format: SESSION_MIGRATION_FORMAT,
        session_name: "session".into(),
        instance_id: uuid::Uuid::new_v4().to_string(),
        tabs: vec![],
        clients: vec![],
        remote_hosts: vec![],
        global_last_active_tab: yinz_types::internal_ids::TabId::new(0),
        tab_group_preferences: vec![],
        paneless_plugins: vec![],
        display_size,
    };

    let current = Size {
        rows: 89,
        cols: 319,
    };
    store.journal.migration = Some(frozen(Some(current)));
    let assets = super::super::coordinator::replacement_assets(&store.journal).unwrap();
    assert_eq!(assets.terminal_window_size, current);
    assert_eq!(assets.update_target.as_deref(), Some(operation.as_str()));
    assert!(!assets.supports_in_place_update);

    // A source from before the size was recorded keeps its startup size.
    store.journal.migration = Some(frozen(None));
    let assets = super::super::coordinator::replacement_assets(&store.journal).unwrap();
    assert_eq!(assets.terminal_window_size, startup);
}
