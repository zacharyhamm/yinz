use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn a_probe_in_progress_does_not_make_a_dead_lease_look_held() {
    let root = tempfile::tempdir().unwrap();
    let registration = Registration::new(root.path(), "dead-", HashMap::new()).unwrap();
    registration._lease.unlock().unwrap();
    // Another hook's probe of the same dead lease, caught while it holds its lock.
    let concurrent_probe = File::open(registration.dir.path().join("lease")).unwrap();
    concurrent_probe.try_lock_shared().unwrap();
    assert!(!lease_held(registration.dir.path()).unwrap());
    concurrent_probe.unlock().unwrap();
    registration._lease.lock().unwrap();
    assert!(lease_held(registration.dir.path()).unwrap());
}

#[test]
fn lookup_uses_the_latest_live_binding_and_ignores_invalid_entries() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        latest_binding::<Value>(&root.path().join("missing"), |_| true)
            .unwrap()
            .is_none()
    );
    let first = Registration::new(root.path(), "first-", HashMap::new()).unwrap();
    let second = Registration::new(root.path(), "second-", HashMap::new()).unwrap();
    for (registration, number) in [(&first, 1), (&second, 2)] {
        let path = registration.dir.path().join("binding.json");
        write_json(&path, &json!({"session": "selected", "number": number})).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(number)),
            )
            .unwrap();
    }
    let lookup = || latest_binding::<Value>(root.path(), |b| b["session"] == "selected").unwrap();
    assert_eq!(lookup().unwrap()["number"], 2);
    second._lease.unlock().unwrap();
    assert_eq!(lookup().unwrap()["number"], 1);
    second._lease.lock().unwrap();
    let path = second.dir.path().join("binding.json");
    for invalid in ["broken JSON".to_owned(), " ".repeat(16 * 1024 + 1)] {
        std::fs::write(&path, invalid).unwrap();
        assert_eq!(lookup().unwrap()["number"], 1);
    }
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(first.dir.path().join("binding.json"), &path).unwrap();
    assert!(read_binding::<Value>(second.dir.path()).is_err());
    assert_eq!(lookup().unwrap()["number"], 1);
    std::fs::remove_file(path).unwrap();
    std::fs::create_dir(second.dir.path().join("binding.json")).unwrap();
    assert!(read_binding::<Value>(second.dir.path()).unwrap().is_none());
    assert_eq!(lookup().unwrap()["number"], 1);
    drop(first);
    assert!(lookup().is_none());
}

#[test]
fn leases_reject_symlinks_and_atomic_publication_keeps_the_old_binding_on_error() {
    let root = tempfile::tempdir().unwrap();
    let registration = Registration::new(root.path(), "live-", HashMap::new()).unwrap();
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(lease_held(registration.dir.path()).unwrap());
    let fake = tempfile::tempdir_in(root.path()).unwrap();
    std::os::unix::fs::symlink(
        registration.dir.path().join("lease"),
        fake.path().join("lease"),
    )
    .unwrap();
    assert!(lease_held(fake.path()).is_err());

    let path = registration.dir.path().join("binding.json");
    write_json(&path, &json!({"session": "original"})).unwrap();
    let invalid = std::collections::BTreeMap::from([(vec![1, 2], "not a JSON object key")]);
    assert!(write_json(&path, &invalid).is_err());
    let (value, _) = read_binding::<Value>(registration.dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(value, json!({"session": "original"}));
}
