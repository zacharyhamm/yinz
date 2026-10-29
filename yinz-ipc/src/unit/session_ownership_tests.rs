use super::*;
use std::os::unix::net::UnixListener;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

#[test]
fn session_locks_refuse_unsafe_directories_without_touching_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    for directory in [
        crate::consts::SESSION_OWNERSHIP_DIR,
        crate::consts::SESSION_CREATION_LOCK_DIR,
    ] {
        symlink(&target, root.path().join(directory)).unwrap();
        assert!(open_lock_file(&root.path().join("session"), directory).is_err());
        assert!(!target.join("session").exists());
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    let shared = root.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(SessionOwnership::acquire(&shared.join("session")).is_err());
    assert!(lock_creation(&shared.join("session"), Instant::now()).is_err());
    assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
    assert_eq!(
        shared.metadata().unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn startup_waits_for_retained_teardown_claim() {
    for stale in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session");
        let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
        let listener = owner.bind().unwrap();
        owner.shutdown().unwrap();
        drop(listener);
        if stale {
            drop(UnixListener::bind(&path).unwrap());
        }
        let mut retained_owner = Some(owner);
        let winner = SessionOwnership::acquire_until(
            &path,
            Instant::now() + Duration::from_secs(10),
            |interval| {
                assert_eq!(interval, Duration::from_millis(50));
                assert!(SessionOwnership::acquire(&path).unwrap().is_none());
                assert!(retained_owner.take().is_some());
            },
        )
        .unwrap()
        .unwrap();
        assert!(retained_owner.is_none());
        let _listener = winner.bind().unwrap();
        assert!(crate::consts::ipc_connect(&path).is_ok());
    }
}

#[test]
fn startup_yields_when_the_other_creator_binds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    let mut listener = None;
    let contender =
        SessionOwnership::acquire_until(&path, Instant::now() + Duration::from_secs(10), |_| {
            assert!(listener.is_none());
            listener = Some(owner.bind().unwrap());
        })
        .unwrap();
    assert!(contender.is_none());
    assert!(listener.is_some());
    assert!(SessionOwnership::acquire(&path).unwrap().is_none());
}

#[test]
fn startup_times_out_without_releasing_another_owners_claim() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let _owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    let error = SessionOwnership::acquire_until(&path, Instant::now(), |_| {
        panic!("the deadline has already elapsed")
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("timed out"));
    assert!(SessionOwnership::acquire(&path).unwrap().is_none());
    assert!(!path.exists());
}

#[test]
fn contention_covers_before_bind_and_teardown() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    assert!(SessionOwnership::acquire(&path).unwrap().is_none());
    let listener = owner.bind().unwrap();
    owner.shutdown().unwrap();
    assert!(!path.exists());
    assert!(SessionOwnership::acquire(&path).unwrap().is_none());
    drop(owner);
    let winner = SessionOwnership::acquire(&path).unwrap().unwrap();
    let _winner_listener = winner.bind().unwrap();
    drop(listener);
    assert!(crate::consts::ipc_connect(&path).is_ok());
    assert!(root.path().join(".session-locks/session").is_file());
}

#[test]
fn simultaneous_claims_have_one_owner() {
    let root = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let claims: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = root.path().join("session");
            std::thread::spawn(move || {
                barrier.wait();
                let claim = SessionOwnership::acquire(&path).unwrap();
                barrier.wait();
                claim
            })
        })
        .collect();
    let claims: Vec<_> = claims
        .into_iter()
        .map(|claim| claim.join().unwrap())
        .collect();
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
}

#[test]
fn creation_lock_excludes_until_released() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let creator = lock_creation(&path, Instant::now()).unwrap();
    let error = lock_creation(&path, Instant::now()).unwrap_err();
    assert!(error.to_string().contains("still creating"), "{error:#}");
    // The server the creator starts must still be able to claim the name.
    assert!(SessionOwnership::acquire(&path).unwrap().is_some());
    drop(creator);
    let _next = lock_creation(&path, Instant::now()).unwrap();
    assert!(root
        .path()
        .join(".session-creation-locks/session")
        .is_file());
}

#[test]
fn refuses_live_unclaimed_socket_and_recovers_stale_socket() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let old_server = UnixListener::bind(&path).unwrap();
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    assert!(owner.bind().is_err());
    drop(owner);
    assert!(crate::consts::ipc_connect(&path).is_ok());
    drop(old_server);
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    let _listener = owner.bind().unwrap();
    assert!(crate::consts::ipc_connect(&path).is_ok());
}

#[test]
fn ambiguous_socket_errors_preserve_the_path() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    std::fs::create_dir(&path).unwrap();
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    assert!(owner.bind().is_err());
    assert!(path.is_dir());
}

#[test]
fn rename_claims_destination_before_transaction_and_rolls_back_on_failure() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let new = root.path().join("new");
    let owner = SessionOwnership::acquire(&old).unwrap().unwrap();
    let _listener = owner.bind().unwrap();
    let contender = SessionOwnership::acquire(&new).unwrap().unwrap();
    assert!(owner
        .rename(&new, || panic!("destination is owned"))
        .is_err());
    drop(contender);
    assert!(owner
        .rename(&new, || {
            assert!(SessionOwnership::acquire(&old)?.is_none());
            assert!(SessionOwnership::acquire(&new)?.is_none());
            bail!("transaction failed")
        })
        .is_err());
    assert!(SessionOwnership::acquire(&old).unwrap().is_none());
    assert!(SessionOwnership::acquire(&new).unwrap().is_some());
    assert!(crate::consts::ipc_connect(&old).is_ok());
}

#[test]
fn shutdown_after_rename_and_late_listener_drop_preserve_reused_old_name() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let new = root.path().join("new");
    let owner = SessionOwnership::acquire(&old).unwrap().unwrap();
    let listener = owner.bind().unwrap();
    drop(UnixListener::bind(&new).unwrap());
    owner
        .rename(&new, || {
            std::fs::rename(&old, &new).context("rename test socket")
        })
        .unwrap();
    assert!(SessionOwnership::acquire(&new).unwrap().is_none());
    let replacement = SessionOwnership::acquire(&old).unwrap().unwrap();
    let _replacement_listener = replacement.bind().unwrap();
    owner.shutdown().unwrap();
    owner.shutdown().unwrap();
    drop(listener);
    assert!(!new.exists());
    assert!(crate::consts::ipc_connect(&old).is_ok());
    assert!(SessionOwnership::acquire(&new).unwrap().is_none());
    drop(owner);
    assert!(SessionOwnership::acquire(&new).unwrap().is_some());
}

#[test]
fn rename_preserves_live_and_non_socket_destinations() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let new = root.path().join("new");
    let owner = SessionOwnership::acquire(&old).unwrap().unwrap();
    let _source = owner.bind().unwrap();
    let destination = UnixListener::bind(&new).unwrap();
    assert!(owner.rename(&new, || panic!("live destination")).is_err());
    assert!(crate::consts::ipc_connect(&new).is_ok());
    drop(destination);
    std::fs::remove_file(&new).unwrap();
    std::fs::write(&new, "preserve").unwrap();
    assert!(owner
        .rename(&new, || panic!("non-socket destination"))
        .is_err());
    assert_eq!(std::fs::read_to_string(&new).unwrap(), "preserve");
    std::fs::remove_file(&new).unwrap();
    std::os::unix::fs::symlink(root.path().join("absent"), &new).unwrap();
    assert!(owner.rename(&new, || panic!("dangling symlink")).is_err());
    assert!(std::fs::symlink_metadata(&new).unwrap().is_symlink());
    assert!(crate::consts::ipc_connect(&old).is_ok());
    assert!(SessionOwnership::acquire(&old).unwrap().is_none());
}

#[test]
fn shutdown_serializes_with_rename() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let new = root.path().join("new");
    let owner = Arc::new(SessionOwnership::acquire(&old).unwrap().unwrap());
    let _listener = owner.bind().unwrap();
    let (entered, received) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let renaming = {
        let owner = owner.clone();
        let new = new.clone();
        std::thread::spawn(move || {
            owner.rename(&new, || {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(10)).unwrap();
                std::fs::rename(old, &new).context("rename test socket")
            })
        })
    };
    received.recv_timeout(Duration::from_secs(10)).unwrap();
    let shutting_down = {
        let owner = owner.clone();
        std::thread::spawn(move || owner.shutdown())
    };
    release.send(()).unwrap();
    renaming.join().unwrap().unwrap();
    shutting_down.join().unwrap().unwrap();
    assert!(!new.exists());
    assert!(owner
        .rename(&root.path().join("third"), || panic!("already stopped"))
        .is_err());
}

#[test]
fn process_death_releases_claim() {
    use std::io::{BufRead, Read, Write};
    const CHILD_PATH: &str = "YINZ_TEST_OWNERSHIP_CHILD";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let _owner = SessionOwnership::acquire(Path::new(&path))
            .unwrap()
            .unwrap();
        println!("CLAIMED");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "session_ownership::tests::process_death_releases_claim",
            "--nocapture",
        ])
        .env(CHILD_PATH, &path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap() == "CLAIMED" {
                ready.send(()).unwrap();
                break;
            }
        }
    });
    let result = received.recv_timeout(Duration::from_secs(10));
    let contested = SessionOwnership::acquire(&path).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    result.unwrap();
    assert!(contested.is_none());
    assert!(SessionOwnership::acquire(&path).unwrap().is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_live_backlog_is_not_reclaimed() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let listener = UnixListener::bind(&path).unwrap();
    // Safety: the listener owns this live socket; only its backlog limit changes.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let _first = crate::consts::ipc_connect(&path).unwrap();
    let _second = crate::consts::ipc_connect(&path).unwrap();
    let owner = SessionOwnership::acquire(&path).unwrap().unwrap();
    assert!(SessionOwnership::acquire_for_startup(&path).is_err());
    let error = owner.bind().err().unwrap();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::TimedOut
    );
    drop(owner);
    let source = root.path().join("source");
    let owner = SessionOwnership::acquire(&source).unwrap().unwrap();
    let _source_listener = owner.bind().unwrap();
    assert!(owner.rename(&path, || panic!("full live backlog")).is_err());
    assert!(crate::consts::ipc_connect(&source).is_ok());
    assert!(path.exists());
}
