use super::*;
use std::os::unix::fs::PermissionsExt;
#[test]
fn private_distinct_records_refresh_and_remove_only_their_owner() {
    let temp = tempfile::tempdir().unwrap();
    let addr = "127.0.0.1:1234".parse().unwrap();
    let first = OwnedRecord::create(temp.path(), addr, "first".into()).unwrap();
    let second = OwnedRecord::create(temp.path(), addr, "second".into()).unwrap();
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
        "x".repeat(4096)
    )
    .is_err());
    assert_eq!(
        std::fs::read_dir(temp.path().join("web/listeners"))
            .unwrap()
            .count(),
        0
    );
}
