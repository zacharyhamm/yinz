use super::*;
use zeroize::Zeroizing;

pub(crate) fn ready(dir: &Path) -> Verifier {
    let salt = SaltString::encode_b64(&[7; 16]).unwrap();
    let phc = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19456, 2, 1, Some(32)).unwrap(),
    )
    .hash_password(b"twelve words long", &salt)
    .unwrap()
    .to_string();
    let web = private_dir(dir).unwrap();
    publish(
        &web,
        Verifier {
            phc,
            verifier_id: random_id().unwrap(),
            updated_at_ms: 0,
        },
    )
    .unwrap();
    match read_state(dir) {
        PasswordState::Ready(v) => v,
        s => panic!("{s:?}"),
    }
}

#[test]
fn roundtrip_production_rotation_and_clear() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    assert_eq!(read_state(dir), PasswordState::Missing);
    write(dir, &Zeroizing::new("🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈".into())).unwrap();
    let PasswordState::Ready(v) = read_state(dir) else {
        panic!("not ready")
    };
    assert!(v.phc.contains("m=65536,t=3,p=1"));
    assert!(verify(&v, "🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈".as_bytes()));
    assert!(!verify(&v, b"wrong"));
    assert!(!format!("{v:?}").contains(&v.phc));
    std::fs::write(dir.join("web/sessions.json"), b"old sessions").unwrap();
    revoke_sessions(dir).unwrap();
    let PasswordState::Ready(rotated) = read_state(dir) else {
        panic!("not ready")
    };
    assert_eq!(v.phc, rotated.phc);
    assert_ne!(v.verifier_id, rotated.verifier_id);
    assert!(!dir.join("web/sessions.json").exists());
    assert!(clear(dir).unwrap());
    assert!(!clear(dir).unwrap());
    assert!(dir.join("web/.lock").exists());
}

#[test]
fn rejects_unsafe_files_and_verifiers() {
    let temp = tempfile::tempdir().unwrap();
    let v = ready(temp.path());
    let path = temp.path().join("web/password");
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(read_state(temp.path()), PasswordState::Invalid(_)));
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(matches!(read_state(temp.path()), PasswordState::Invalid(_)));
    clear(temp.path()).unwrap();
    assert!(!path.exists());
    for bytes in [
        vec![b' '; 4097],
        b"{}".to_vec(),
        serde_json::to_vec(&Stored {
            version: 2,
            phc: v.phc.clone(),
            verifier_id: v.verifier_id.clone(),
            updated_at_ms: 0,
        })
        .unwrap(),
    ] {
        atomic_write(&temp.path().join("web"), "password", &bytes).unwrap();
        assert!(matches!(read_state(temp.path()), PasswordState::Invalid(_)));
    }
    for phc in [
        v.phc.replace("m=19456", "m=1"),
        v.phc.replace("p=1", "p=5"),
        v.phc.replace("t=2", "t=11"),
        v.phc.replace("argon2id", "argon2i"),
        v.phc.replace("v=19", "v=16"),
        v.phc.replace("p=1", "p=1,x=2"),
        v.phc.replace("p=1", "p=1,p=1"),
    ] {
        assert!(validate_phc(&phc).is_err());
    }
    assert!(write(temp.path(), &Zeroizing::new("too short".into())).is_err());
    assert!(write(temp.path(), &Zeroizing::new("twelve\0characters".into())).is_err());
}

#[test]
fn nonregular_does_not_block_and_failed_publish_keeps_old_verifier() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let v = ready(dir);
    std::fs::create_dir(dir.join("web/sessions.json")).unwrap();
    assert!(revoke_sessions(dir).is_err());
    assert_eq!(read_state(dir), PasswordState::Ready(v));
    std::fs::remove_file(dir.join("web/password")).unwrap();
    let path =
        std::ffi::CString::new(dir.join("web/password").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: path is NUL terminated and points into the owned temporary directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(matches!(read_state(dir), PasswordState::Invalid(_)));
}

#[test]
fn concurrent_updates_share_one_persistent_lock() {
    let temp = tempfile::tempdir().unwrap();
    ready(temp.path());
    let inode = std::fs::metadata(temp.path().join("web/password"))
        .unwrap()
        .ino();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..4 {
                    let _ = revoke_sessions(temp.path());
                }
            });
        }
    });
    assert!(matches!(read_state(temp.path()), PasswordState::Ready(_)));
    assert_ne!(
        std::fs::metadata(temp.path().join("web/password"))
            .unwrap()
            .ino(),
        inode
    );
}

#[test]
fn lock_symlinks_private_directory_and_session_symlink_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    ready(temp.path());
    let outside = temp.path().join("outside");
    std::fs::write(&outside, "do not remove target").unwrap();
    std::os::unix::fs::symlink(&outside, temp.path().join("web/sessions.json")).unwrap();
    revoke_sessions(temp.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "do not remove target"
    );
    std::fs::remove_file(temp.path().join("web/.lock")).unwrap();
    std::os::unix::fs::symlink(&outside, temp.path().join("web/.lock")).unwrap();
    assert!(clear(temp.path()).is_err());
    assert!(matches!(read_state(temp.path()), PasswordState::Ready(_)));
    std::fs::set_permissions(
        temp.path().join("web"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(matches!(read_state(temp.path()), PasswordState::Invalid(_)));
}

#[test]
fn concurrent_password_write_clear_and_reader_never_sees_partial_json() {
    let temp = tempfile::tempdir().unwrap();
    ready(temp.path());
    let finished = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..2 {
                write(temp.path(), &Zeroizing::new("concurrent password".into())).unwrap();
                clear(temp.path()).unwrap();
            }
            finished.store(true, std::sync::atomic::Ordering::Release);
        });
        while !finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(!matches!(
                read_state(temp.path()),
                PasswordState::Invalid(_)
            ));
            std::thread::yield_now();
        }
    });
}

#[test]
fn foreign_owner_metadata_is_rejected_without_chown() {
    let temp = tempfile::tempdir().unwrap();
    ready(temp.path());
    let metadata = std::fs::metadata(temp.path().join("web/password")).unwrap();
    assert!(validate_metadata(&metadata, metadata.uid()).is_ok());
    assert!(validate_metadata(&metadata, metadata.uid().wrapping_add(1)).is_err());
}
