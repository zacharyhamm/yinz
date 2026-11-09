use super::*;
use std::os::unix::fs::PermissionsExt;
fn fixture() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let v = password::tests::ready(t.path());
    (t, v.verifier_id)
}
fn issue(s: &mut Store, t: &Path, v: &str, now: i64, remember: bool) -> (String, SessionInfo) {
    let _lock = password::lock(&t.join("web")).unwrap();
    s.issue(t, v, "Test Browser".into(), remember, 30, now)
        .unwrap()
}
#[test]
fn expiry_boundaries_remember_absolute_eviction_and_hash_only() {
    let (t, v) = fixture();
    let mut s = Store::default();
    let (token, info) = issue(&mut s, t.path(), &v, 1000, false);
    assert!(valid_token(&token));
    assert_eq!(info.id.len(), 16);
    let bytes = std::fs::read_to_string(t.path().join("web/sessions.json")).unwrap();
    assert!(!bytes.contains(&token));
    assert!(bytes.contains(&hash(&token)));
    assert!(s
        .authenticate(t.path(), &v, &token, 1000 + 12 * 3600000)
        .unwrap()
        .is_none());
    let (remembered, _) = issue(&mut s, t.path(), &v, 1000, true);
    assert!(s
        .authenticate(t.path(), &v, &remembered, 1000 + 14 * 86400000)
        .unwrap()
        .is_none());
    let (absolute, _) = issue(&mut s, t.path(), &v, 1000, true);
    for day in 1..30 {
        assert!(s
            .authenticate(t.path(), &v, &absolute, 1000 + day * 86400000)
            .unwrap()
            .is_some());
    }
    assert!(s
        .authenticate(t.path(), &v, &absolute, 1000 + 30 * 86400000)
        .unwrap()
        .is_none());
    password::revoke_sessions(t.path()).unwrap();
    let password::PasswordState::Ready(v) = password::read_state(t.path()) else {
        panic!()
    };
    let (old, _) = issue(&mut s, t.path(), &v.verifier_id, 1000, false);
    for i in 1..=16 {
        issue(&mut s, t.path(), &v.verifier_id, 1000 + i, false);
    }
    assert_eq!(read(t.path(), &v.verifier_id).unwrap().sessions.len(), 16);
    assert!(s
        .authenticate(t.path(), &v.verifier_id, &old, 2000)
        .unwrap()
        .is_none());
}
#[test]
fn external_rewrite_permissions_corruption_symlink_fifo_and_missing_fail_closed() {
    let (t, v) = fixture();
    let mut a = Store::default();
    let (token, _) = issue(&mut a, t.path(), &v, 1, false);
    assert!(a.authenticate(t.path(), &v, &token, 2).unwrap().is_some());
    let path = t.path().join("web/sessions.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(a.authenticate(t.path(), &v, &token, 3).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut snapshot = read(t.path(), &v).unwrap();
    snapshot.sessions[0].token_hash = "f".repeat(64);
    write(t.path(), &snapshot).unwrap();
    assert!(a.authenticate(t.path(), &v, &token, 3).unwrap().is_none());
    password::atomic_write(&t.path().join("web"), "sessions.json", b"{}").unwrap();
    assert!(a.snapshot(t.path(), &v).is_err());
    assert!(a.issue(t.path(), &v, "a".into(), false, 30, 3).is_err());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(a.snapshot(t.path(), &v).is_err());
    std::fs::remove_file(&path).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: p is NUL-terminated; mkfifo creates only the test fixture path.
    assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
    assert!(a.snapshot(t.path(), &v).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(a.snapshot(t.path(), &v).unwrap().sessions.is_empty());
}
#[test]
fn dirty_merge_preserves_external_login_logout_fields_and_verifier_rotation() {
    let (t, v) = fixture();
    let (mut a, mut b) = (Store::default(), Store::default());
    let (first, info) = issue(&mut a, t.path(), &v, 1000000, false);
    a.authenticate(t.path(), &v, &first, 1000001).unwrap();
    let (second, _) = issue(&mut b, t.path(), &v, 1000002, false);
    let mut current = read(t.path(), &v).unwrap();
    current.sessions[0].label = "external label".into();
    write(t.path(), &current).unwrap();
    a.flush(t.path(), &v, 1000003).unwrap();
    let rows = read(t.path(), &v).unwrap().sessions;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label, "external label");
    a.authenticate(t.path(), &v, &first, 1000004).unwrap();
    b.logout_guarded(t.path(), &info.id, 1000005, || true)
        .unwrap();
    a.flush(t.path(), &v, 1000006).unwrap();
    assert!(a
        .authenticate(t.path(), &v, &first, 1000007)
        .unwrap()
        .is_none());
    assert!(a
        .authenticate(t.path(), &v, &second, 1000007)
        .unwrap()
        .is_some());
    password::revoke_sessions(t.path()).unwrap();
    a.flush(t.path(), &v, 1000010).unwrap();
    assert!(!t.path().join("web/sessions.json").exists());
}
#[test]
fn concurrent_stores_do_not_drop_logins_or_restore_revocations() {
    let (t, v) = fixture();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|scope| {
        let mut handles = vec![];
        for _ in 0..2 {
            let barrier = barrier.clone();
            let data = t.path();
            let v = &v;
            handles.push(scope.spawn(move || {
                let mut s = Store::default();
                barrier.wait();
                issue(&mut s, data, v, 1000, false)
            }));
        }
        barrier.wait();
        for h in handles {
            h.join().unwrap();
        }
    });
    assert_eq!(read(t.path(), &v).unwrap().sessions.len(), 2);
}

#[test]
fn pending_touch_extends_local_idle_without_restoring_removed_rows() {
    let (t, v) = fixture();
    let mut s = Store::default();
    let (token, _) = issue(&mut s, t.path(), &v, 1000, false);
    let deadline = 1000 + 12 * 3600000;
    // A different login just flushed the store: this token's next touch stays dirty.
    issue(&mut s, t.path(), &v, deadline - 120000, false);
    assert!(s
        .authenticate(t.path(), &v, &token, deadline - 60000)
        .unwrap()
        .is_some());
    assert!(s
        .authenticate(t.path(), &v, &token, deadline + 60000)
        .unwrap()
        .is_some());
    s.flush(t.path(), &v, deadline + 60000).unwrap();
    let row = read(t.path(), &v)
        .unwrap()
        .sessions
        .into_iter()
        .find(|r| r.token_hash == hash(&token))
        .unwrap();
    assert_eq!(row.last_seen_ms, deadline + 60000);
}
#[test]
fn invalid_versions_duplicates_timestamps_and_size_are_rejected() {
    let (t, v) = fixture();
    let mut s = Store::default();
    issue(&mut s, t.path(), &v, 1000, false);
    let original = read(t.path(), &v).unwrap();
    let mut bad = original.clone();
    bad.version = 2;
    write(t.path(), &bad).unwrap();
    assert!(s.snapshot(t.path(), &v).is_err());
    let mut bad = original.clone();
    bad.sessions.push(bad.sessions[0].clone());
    write(t.path(), &bad).unwrap();
    assert!(s.snapshot(t.path(), &v).is_err());
    let mut bad = original;
    bad.sessions[0].last_seen_ms = 0;
    write(t.path(), &bad).unwrap();
    assert!(s.snapshot(t.path(), &v).is_err());
    password::atomic_write(
        &t.path().join("web"),
        "sessions.json",
        &vec![b'x'; MAX_BYTES as usize + 1],
    )
    .unwrap();
    assert!(s.snapshot(t.path(), &v).is_err());
}
