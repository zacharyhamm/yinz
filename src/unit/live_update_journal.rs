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
fn cancellation_is_durable_but_does_not_change_the_recorded_ownership_phase() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let data = root.path().join("data");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open(&socket, &data, &operation).unwrap();
    store.phase(Phase::Prepare).unwrap();
    std::fs::write(store.directory.join("cancel"), b"").unwrap();
    drop(store);
    let retry = Store::open(&socket, &data, &operation).unwrap();
    assert!(retry.cancel_requested());
    assert_eq!(retry.journal.phase, Phase::Prepare);
    let mut retry = retry;
    assert!(retry.phase(Phase::Transfer).is_err());
    assert_eq!(retry.journal.phase, Phase::Prepare);
}

#[test]
fn retry_before_inventory_preserves_the_original_release_or_source_mode() {
    for source_build in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("session");
        let data = root.path().join("data");
        let operation = uuid::Uuid::new_v4().to_string();
        let original = Store::open_with_source(&socket, &data, &operation, source_build).unwrap();
        assert_eq!(original.journal.phase, Phase::Discover);
        assert!(original.journal.source.is_none());
        drop(original);
        // Simulate a coordinator crash before the first server inventory. The
        // opposite palette action must resume this operation without changing
        // its artifact source, even though no release or inventory exists yet.
        let retry = Store::open_with_source(
            &socket,
            &data,
            &uuid::Uuid::new_v4().to_string(),
            !source_build,
        )
        .unwrap();
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
