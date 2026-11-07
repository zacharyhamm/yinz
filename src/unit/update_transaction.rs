use super::*;

struct Installation {
    _directory: tempfile::TempDir,
    source: PathBuf,
    binary: PathBuf,
    data: PathBuf,
}

impl Installation {
    fn new(existing_builtins: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("release");
        fs::create_dir_all(source.join("plugins/builtins/nested")).unwrap();
        fs::write(source.join("yinz"), b"new binary").unwrap();
        fs::write(source.join("plugins/builtins/main.scm"), b"new plugin").unwrap();
        fs::write(
            source.join("plugins/builtins/nested/library.scm"),
            b"new library",
        )
        .unwrap();
        let binary = directory.path().join("yinz");
        fs::write(&binary, b"old binary").unwrap();
        let data = directory.path().join("data");
        if existing_builtins {
            fs::create_dir_all(data.join("plugins/builtins")).unwrap();
            fs::write(data.join("plugins/builtins/main.scm"), b"old plugin").unwrap();
            fs::write(data.join("plugins/builtins/custom.scm"), b"custom plugin").unwrap();
        }
        Self {
            _directory: directory,
            source,
            binary,
            data,
        }
    }

    fn prepare(&self) -> Transaction {
        Transaction::prepare(&self.source, &self.binary, &self.data, "operation-1").unwrap()
    }

    fn open(&self) -> Transaction {
        Transaction::open(&self.binary, &self.data, "operation-1").unwrap()
    }

    fn assert_old(&self) {
        assert_eq!(fs::read(&self.binary).unwrap(), b"old binary");
        assert_eq!(
            fs::read(self.data.join("plugins/builtins/main.scm")).unwrap(),
            b"old plugin"
        );
        assert_eq!(
            fs::read(self.data.join("plugins/builtins/custom.scm")).unwrap(),
            b"custom plugin"
        );
    }

    fn assert_new(&self) {
        assert_eq!(fs::read(&self.binary).unwrap(), b"new binary");
        assert_eq!(
            fs::read(self.data.join("plugins/builtins/main.scm")).unwrap(),
            b"new plugin"
        );
        assert_eq!(
            fs::read(self.data.join("plugins/builtins/nested/library.scm")).unwrap(),
            b"new library"
        );
    }
}

#[test]
fn update_transaction_prepare_does_not_publish_and_survives_helper_exit() {
    let installation = Installation::new(true);
    let transaction = installation.prepare();
    assert_eq!(transaction.status(), Status::Prepared);
    installation.assert_old();
    drop(transaction);
    let mut resumed = installation.open();
    resumed.publish().unwrap();
    resumed.publish().unwrap();
    installation.assert_new();
    assert_eq!(fs::read(resumed.binary()).unwrap(), b"old binary");
    assert_eq!(
        fs::read(resumed.generation().join("main.scm")).unwrap(),
        b"old plugin"
    );
    assert_eq!(
        fs::read(installation.data.join("plugins/builtins/custom.scm")).unwrap(),
        b"custom plugin"
    );
    let (binary_root, data_root) = resumed.backup_roots_for_test();
    resumed.commit().unwrap();
    assert!(!binary_root.exists());
    assert!(!data_root.exists());
    assert!(!resumed.location.claim.exists());
    drop(resumed);
    let mut receipt = installation.open();
    assert_eq!(receipt.status(), Status::Committed);
    receipt.commit().unwrap();
    assert!(receipt.rollback().is_err());
}

#[test]
fn update_transaction_recovers_every_publication_crash_window() {
    // A helper can disappear before the first exchange, between exchanges, or
    // after the second exchange but before publishing the final phase record.
    for exchanges in 0..=2 {
        let installation = Installation::new(true);
        let mut transaction = installation.prepare();
        transaction.set_status(Status::Publishing).unwrap();
        if exchanges >= 1 {
            transaction.publish_builtins().unwrap();
        }
        if exchanges >= 2 {
            transaction.publish_binary().unwrap();
        }
        drop(transaction);
        let mut resumed = installation.open();
        resumed.rollback().unwrap();
        resumed.rollback().unwrap();
        installation.assert_old();
        drop(resumed);
        let mut receipt = installation.open();
        assert_eq!(receipt.status(), Status::RolledBack);
        receipt.rollback().unwrap();
        assert!(receipt.publish().is_err());
    }
}

#[test]
fn update_transaction_can_finish_publication_after_either_exchange() {
    for exchanges in 0..=2 {
        let installation = Installation::new(true);
        let mut transaction = installation.prepare();
        transaction.set_status(Status::Publishing).unwrap();
        if exchanges >= 1 {
            transaction.publish_builtins().unwrap();
        }
        if exchanges >= 2 {
            transaction.publish_binary().unwrap();
        }
        drop(transaction);
        let mut resumed = installation.open();
        resumed.publish().unwrap();
        installation.assert_new();
        resumed.commit().unwrap();
    }
}

#[test]
fn update_transaction_first_builtin_installation_can_be_rolled_back() {
    let installation = Installation::new(false);
    let mut transaction = installation.prepare();
    assert!(!installation.data.join("plugins/builtins").exists());
    transaction.publish().unwrap();
    installation.assert_new();
    drop(transaction);
    installation.open().rollback().unwrap();
    assert_eq!(fs::read(&installation.binary).unwrap(), b"old binary");
    assert!(!installation.data.join("plugins/builtins").exists());
}

#[test]
fn update_transaction_locks_binary_and_shared_data_across_helper_exits() {
    let installation = Installation::new(true);
    let transaction = installation.prepare();
    assert!(Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "operation-2"
    )
    .is_err());
    drop(transaction);
    // Persistent claims, rather than the now-released process lock, exclude a
    // competing operation between the prepare and publish helper invocations.
    assert!(Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "operation-2"
    )
    .is_err());
    let other_binary = installation.binary.with_file_name("other-yinz");
    fs::write(&other_binary, b"other binary").unwrap();
    assert!(Transaction::prepare(
        &installation.source,
        &other_binary,
        &installation.data,
        "operation-2"
    )
    .is_err());
    let other_data = installation.data.with_file_name("other-data");
    assert!(Transaction::prepare(
        &installation.source,
        &installation.binary,
        &other_data,
        "operation-2"
    )
    .is_err());
    installation.open().rollback().unwrap();
    Transaction::prepare(
        &installation.source,
        &other_binary,
        &installation.data,
        "operation-2",
    )
    .unwrap()
    .rollback()
    .unwrap();
}

#[test]
fn update_transaction_refuses_changed_ownership_without_destroying_backups() {
    let installation = Installation::new(true);
    let mut transaction = installation.prepare();
    transaction.publish().unwrap();
    let foreign = installation.binary.with_file_name("foreign");
    fs::write(&foreign, b"unrelated installation").unwrap();
    fs::rename(&foreign, &installation.binary).unwrap();
    assert!(transaction.rollback().is_err());
    assert_eq!(
        fs::read(&installation.binary).unwrap(),
        b"unrelated installation"
    );
    assert_eq!(fs::read(transaction.binary()).unwrap(), b"old binary");
    assert_eq!(
        fs::read(transaction.generation().join("main.scm")).unwrap(),
        b"old plugin"
    );
}

#[test]
fn update_transaction_rejects_symlinks_before_touching_active_files() {
    let installation = Installation::new(true);
    std::os::unix::fs::symlink(
        "main.scm",
        installation.source.join("plugins/builtins/link.scm"),
    )
    .unwrap();
    assert!(Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "operation-1"
    )
    .is_err());
    installation.assert_old();
    let receipt = installation.open();
    assert_eq!(receipt.status(), Status::RolledBack);
    assert!(!receipt.location.claim.exists());
}

#[test]
fn update_transaction_recovery_resumes_interrupted_rollback_and_cleanup() {
    let installation = Installation::new(true);
    let mut transaction = installation.prepare();
    transaction.publish().unwrap();
    transaction.set_status(Status::RollingBack).unwrap();
    transition(
        &transaction.location.executable,
        &transaction.binary(),
        Some(transaction.journal.old_binary),
        transaction.journal.new_binary.unwrap(),
        false,
    )
    .unwrap();
    drop(transaction);
    installation.open().rollback().unwrap();
    installation.assert_old();
    // A commit receipt is written before deleting either backup. Losing the
    // helper during cleanup therefore never permits a subsequent rollback.
    let mut transaction = Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "operation-2",
    )
    .unwrap();
    transaction.publish().unwrap();
    transaction.mark_committed_for_test().unwrap();
    fs::remove_dir_all(&transaction.journal.binary_root).unwrap();
    drop(transaction);
    let mut resumed =
        Transaction::open(&installation.binary, &installation.data, "operation-2").unwrap();
    resumed.commit().unwrap();
    installation.assert_new();
}

#[test]
fn update_transaction_standalone_retry_recovers_but_refuses_session_journal() {
    let installation = Installation::new(true);
    let transaction = installation.prepare();
    drop(transaction);
    assert!(recover_standalone(&installation.binary, &installation.data).is_err());
    installation.open().rollback().unwrap();
    let mut transaction = Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "standalone-test",
    )
    .unwrap();
    transaction.set_status(Status::Publishing).unwrap();
    transaction.publish_builtins().unwrap();
    drop(transaction);
    recover_standalone(&installation.binary, &installation.data).unwrap();
    installation.assert_old();
}

#[test]
fn update_transaction_lookup_distinguishes_absence_from_another_active_operation() {
    let installation = Installation::new(true);
    assert!(
        Transaction::lookup(&installation.binary, &installation.data, "operation-1")
            .unwrap()
            .is_none()
    );
    drop(installation.prepare());
    assert!(Transaction::lookup(&installation.binary, &installation.data, "operation-2").is_err());
    installation.open().rollback().unwrap();
    assert!(
        Transaction::lookup(&installation.binary, &installation.data, "operation-2")
            .unwrap()
            .is_none()
    );
    assert!(Transaction::prepare(
        &installation.source,
        &installation.binary,
        &installation.data,
        "operation-1"
    )
    .is_err());
}

#[test]
fn update_transaction_unconfirmed_rollback_leaves_another_operations_claim() {
    let installation = Installation::new(true);
    installation.prepare().publish().unwrap();
    // Another executable sharing the data directory has no journal, but its preparation is
    // refused by the first operation's claim.
    let other_binary = installation.binary.with_file_name("other-yinz");
    fs::write(&other_binary, b"other binary").unwrap();
    let refused = Transaction::prepare(
        &installation.source,
        &other_binary,
        &installation.data,
        "operation-2",
    )
    .err()
    .expect("the first operation claims the shared data directory");
    assert!(
        format!("{refused:#}")
            .contains("builtin installation belongs to unfinished update operation-1"),
        "{refused:#}"
    );
    assert!(Transaction::lookup_for_rollback(
        &other_binary,
        &installation.data,
        "operation-2",
        false
    )
    .is_err());
    assert!(Transaction::lookup_for_rollback(
        &other_binary,
        &installation.data,
        "operation-2",
        true
    )
    .unwrap()
    .is_none());
    assert_eq!(fs::read(&other_binary).unwrap(), b"other binary");
    installation.assert_new();
    assert_eq!(installation.open().status(), Status::Published);
}

#[test]
fn update_transaction_finished_binary_can_update_another_data_directory() {
    let installation = Installation::new(true);
    let mut transaction = installation.prepare();
    transaction.publish().unwrap();
    transaction.commit().unwrap();
    drop(transaction);
    let other_data = installation.data.with_file_name("other-data");
    let mut next = Transaction::prepare(
        &installation.source,
        &installation.binary,
        &other_data,
        "operation-2",
    )
    .unwrap();
    next.publish().unwrap();
    next.commit().unwrap();
    assert_eq!(
        fs::read(other_data.join("plugins/builtins/main.scm")).unwrap(),
        b"new plugin"
    );
    assert_eq!(
        fs::read(installation.data.join("plugins/builtins/custom.scm")).unwrap(),
        b"custom plugin"
    );
}

#[test]
fn update_transaction_preserves_private_custom_directory_modes() {
    let installation = Installation::new(true);
    let private = installation.data.join("plugins/builtins/private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(private.join("custom.scm"), b"private").unwrap();
    let mut transaction = installation.prepare();
    transaction.publish().unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::read(private.join("custom.scm")).unwrap(), b"private");
}

#[test]
fn update_transaction_cleans_read_only_backups_without_changing_active_permissions() {
    for rollback in [false, true] {
        let installation = Installation::new(true);
        let private = installation.data.join("plugins/builtins/private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("custom.scm"), b"private").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o500)).unwrap();
        let mut transaction = installation.prepare();
        transaction.publish().unwrap();
        if rollback {
            transaction.rollback().unwrap();
        } else {
            transaction.commit().unwrap();
        }
        assert!(!transaction.journal.binary_root.exists());
        assert!(!transaction.journal.data_root.exists());
        assert_eq!(
            fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o500
        );
        assert_eq!(fs::read(private.join("custom.scm")).unwrap(), b"private");
        // Allow TempDir to dispose the intentionally read-only active fixture.
        fs::set_permissions(private, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn update_transaction_directory_lock_excludes_generation_readers() {
    let installation = Installation::new(true);
    let plugins = installation.data.join("plugins");
    let reader = yinz_runtime::builtin_generation::read_guard(&installation.data).unwrap();
    assert!(lock_directory(&plugins, std::time::Duration::ZERO).is_err());
    drop(reader);
    let transaction = installation.prepare();
    let reader = File::open(&plugins).unwrap();
    assert!(matches!(
        reader.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(transaction);
    reader.try_lock_shared().unwrap();
}
