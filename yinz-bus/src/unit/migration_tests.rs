use crate::migration::*;
use yinz_types::internal_ids::SessionName;

fn snapshot(operation: &str) -> SessionMigrationSnapshot {
    SessionMigrationSnapshot {
        format: SESSION_MIGRATION_FORMAT,
        operation: operation.into(),
        inventory: SessionMigrationInventory {
            format: SESSION_MIGRATION_FORMAT,
            session_name: SessionName::new("source"),
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
    }
}

#[test]
fn snapshot_is_private_durable_and_operation_scoped() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.json");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = snapshot(&operation);
    save_snapshot(&path, &source).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    source.phase = MigrationPhase::Released;
    save_snapshot(&path, &source).unwrap();
    assert_eq!(
        load_snapshot(&path, &operation).unwrap().phase,
        MigrationPhase::Released
    );
    assert!(load_snapshot(&path, &uuid::Uuid::new_v4().to_string()).is_err());
    assert!(save_snapshot(&path, &snapshot(&operation)).is_err());
    assert_eq!(
        load_snapshot(&path, &operation).unwrap().phase,
        MigrationPhase::Released
    );
}

#[test]
fn snapshot_refuses_symlinks_public_permissions_and_unknown_formats() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.json");
    let link = directory.path().join("link.json");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = snapshot(&operation);
    save_snapshot(&path, &source).unwrap();
    symlink(&path, &link).unwrap();
    assert!(load_snapshot(&link, &operation).is_err());
    assert!(save_snapshot(&link, &source).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_snapshot(&path, &operation).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    source.format += 1;
    // Unsupported formats cannot be passed to an owner-transfer operation.
    std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    assert!(load_snapshot(&path, &operation).is_err());
}
