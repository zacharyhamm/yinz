use super::*;
use std::os::unix::fs::PermissionsExt;
#[test]
fn private_distinct_records_refresh_and_remove_only_their_owner() {
    let temp = tempfile::tempdir().unwrap();
    let addr = "127.0.0.1:1234".parse().unwrap();
    let first = OwnedRecord::create(temp.path(), addr, "first".into(), None).unwrap();
    let second = OwnedRecord::create(temp.path(), addr, "second".into(), None).unwrap();
    let path = first.dir.join(&first.name);
    assert_ne!(first.name, second.name);
    assert_eq!(
        std::fs::metadata(&first.dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    first.refresh("renamed".into()).unwrap();
    let mut record: ListenerRecord =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(record.session, "renamed");
    assert_eq!(record.address, addr);
    record.instance = "replacement owner".into();
    atomic_write(
        &first.dir,
        &first.name,
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    drop(first);
    assert!(path.exists(), "old owner removed another instance's record");
    let second_path = second.dir.join(&second.name);
    drop(second);
    assert!(!second_path.exists());
}
#[test]
fn failed_first_publication_cleans_reserved_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(OwnedRecord::create(
        temp.path(),
        "127.0.0.1:1".parse().unwrap(),
        "x".repeat(65536),
        None
    )
    .is_err());
    assert_eq!(
        std::fs::read_dir(temp.path().join("web/listeners"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn legacy_snapshot_stale_cleanup_alive_ambiguous_and_replacement_race() {
    let t = tempfile::tempdir().unwrap();
    let record =
        OwnedRecord::create(t.path(), "127.0.0.1:1".parse().unwrap(), "old".into(), None).unwrap();
    let path = record.dir.join(&record.name);
    let rows = inspect_with(t.path(), |_| None, |_| false).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].record.startup_config.is_none());
    assert!(path.exists());
    // Even a dead recorded PID cannot remove a record when another listener answers.
    assert_eq!(
        inspect_with(t.path(), |_| Some("other".into()), |_| true)
            .unwrap()
            .len(),
        1
    );
    let mut calls = 0;
    let rows = inspect_with(
        t.path(),
        |_| None,
        |_| {
            calls += 1;
            if calls == 1 {
                let (mut r, _) = read_record(&path).unwrap();
                r.instance = "a".repeat(32);
                atomic_write(&record.dir, &record.name, &serde_json::to_vec(&r).unwrap()).unwrap();
            }
            true
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(path.exists());
    drop(record);
    assert!(path.exists());
    assert!(inspect_with(t.path(), |_| None, |_| true)
        .unwrap()
        .is_empty());
    assert!(!path.exists());
}
#[test]
fn record_scan_rejects_unsafe_bounded_files_and_refresh_preserves_replacement() {
    let t = tempfile::tempdir().unwrap();
    let config = yinz_types::input::web::WebConfig {
        port: Some(0),
        ..Default::default()
    };
    let record = OwnedRecord::create(
        t.path(),
        "[::1]:1234".parse().unwrap(),
        "first".into(),
        Some(config.clone()),
    )
    .unwrap();
    let path = record.dir.join(&record.name);
    record.refresh("second".into()).unwrap();
    let (r, _) = read_record(&path).unwrap();
    assert_eq!(r.startup_config, Some(config));
    assert_eq!(r.session, "second");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_record(&path).is_err());
    assert!(inspect_with(t.path(), |_| None, |_| true)
        .unwrap()
        .is_empty());
    assert!(path.exists());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut r = r;
    r.instance = "b".repeat(32);
    atomic_write(&record.dir, &record.name, &serde_json::to_vec(&r).unwrap()).unwrap();
    assert!(record.refresh("cannot overwrite".into()).is_err());
    atomic_write(&record.dir, &record.name, &vec![b'x'; 65537]).unwrap();
    assert!(read_record(&path).is_err());
    drop(record);
    assert!(path.exists());
}
#[test]
fn probe_never_connects_to_nonlocal_record_and_dead_pid_test_is_conservative() {
    assert!(probe("192.0.2.1:7419".parse().unwrap()).is_none());
    assert!(!dead(std::process::id()));
    assert!(!dead(0));
    assert!(!dead(u32::MAX));
}
