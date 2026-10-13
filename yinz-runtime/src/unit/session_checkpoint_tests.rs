use super::*;
use std::os::unix::fs::MetadataExt;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

fn layout(contents: &str) -> (String, BTreeMap<String, String>) {
    (
        "layout {\n tab {\n pane contents_file=\"initial_contents_1\"\n }\n}\n".to_owned(),
        BTreeMap::from([("initial_contents_1".to_owned(), contents.to_owned())]),
    )
}

fn assert_snapshot(checkpoint: &SessionCheckpoint, expected: &str) {
    assert_eq!(
        fs::read_to_string(checkpoint.metadata_path()).unwrap(),
        expected
    );
    assert_eq!(
        fs::read_to_string(checkpoint.layout_path()).unwrap(),
        layout(expected).0
    );
    assert_eq!(
        fs::read_to_string(checkpoint.directory.join("initial_contents_1")).unwrap(),
        expected
    );
}

#[test]
fn failed_staging_and_publication_preserve_the_complete_previous_snapshot() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "old", &layout("old")).unwrap();
    let old = fs::read_link(root.path().join(CURRENT)).unwrap();
    for failure in ["initial_contents_1", LAYOUT, METADATA, CURRENT] {
        let result = write_checkpoint_with(
            root.path(),
            "new",
            &layout("new"),
            |path| {
                if path.file_name().unwrap() == failure {
                    if failure != CURRENT {
                        // Model a write that leaves partial bytes in its staged file.
                        fs::write(path, "partial")?;
                    }
                    return Err(io::Error::other("injected storage failure"));
                }
                Ok(())
            },
            |_| {},
        );
        assert!(result.is_err(), "{failure}");
        assert_eq!(fs::read_link(root.path().join(CURRENT)).unwrap(), old);
        assert_snapshot(&SessionCheckpoint::read(root.path()).unwrap(), "old");
    }
    write_checkpoint(root.path(), "new", &layout("new")).unwrap();
    assert_snapshot(&SessionCheckpoint::read(root.path()).unwrap(), "new");
}

#[test]
fn a_sync_failure_after_publication_keeps_both_complete_snapshots() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "old", &layout("old")).unwrap();
    let previous = current_directory(root.path()).unwrap().unwrap();
    let result = write_checkpoint_with(
        root.path(),
        "new",
        &layout("new"),
        |path| {
            if path.parent() == Some(root.path())
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(PREFIX)
            {
                return Err(io::Error::other("injected final sync failure"));
            }
            Ok(())
        },
        |_| {},
    );
    assert!(result.is_err());
    assert!(previous.is_dir());
    assert_snapshot(&SessionCheckpoint::read(root.path()).unwrap(), "new");
    // The unchanged retry must still sync the published directory entries.
    write_checkpoint(root.path(), "new", &layout("new")).unwrap();
    assert_snapshot(&SessionCheckpoint::read(root.path()).unwrap(), "new");
}

#[test]
fn unchanged_files_are_reused_and_old_generations_are_cleaned() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "one", &layout("contents")).unwrap();
    let first = current_directory(root.path()).unwrap().unwrap();
    write_checkpoint(root.path(), "one", &layout("contents")).unwrap();
    assert_eq!(current_directory(root.path()).unwrap().unwrap(), first);
    let orphan = tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(root.path())
        .unwrap()
        .into_path();
    write_checkpoint(root.path(), "two", &layout("contents")).unwrap();
    let second = current_directory(root.path()).unwrap().unwrap();
    assert_eq!(
        fs::metadata(first.join(LAYOUT)).unwrap().ino(),
        fs::metadata(second.join(LAYOUT)).unwrap().ino()
    );
    assert_eq!(
        fs::metadata(first.join("initial_contents_1"))
            .unwrap()
            .ino(),
        fs::metadata(second.join("initial_contents_1"))
            .unwrap()
            .ino()
    );
    assert_ne!(
        fs::metadata(first.join(METADATA)).unwrap().ino(),
        fs::metadata(second.join(METADATA)).unwrap().ino()
    );
    assert!(!orphan.exists());
    write_checkpoint(root.path(), "three", &layout("contents")).unwrap();
    assert!(!first.exists());
    assert!(second.exists());
}

#[test]
fn metadata_only_saves_and_legacy_migration_preserve_recovery_files() {
    let root = tempfile::tempdir().unwrap();
    let empty = (String::new(), BTreeMap::new());
    write_checkpoint(root.path(), "startup", &empty).unwrap();
    assert!(!SessionCheckpoint::read(root.path())
        .unwrap()
        .layout_path()
        .exists());
    write_checkpoint(root.path(), "old", &layout("old")).unwrap();
    write_checkpoint(root.path(), "new metadata", &empty).unwrap();
    let checkpoint = SessionCheckpoint::read(root.path()).unwrap();
    assert_eq!(
        fs::read_to_string(checkpoint.metadata_path()).unwrap(),
        "new metadata"
    );
    assert_eq!(
        fs::read_to_string(checkpoint.directory.join("initial_contents_1")).unwrap(),
        "old"
    );
    drop(checkpoint);

    let legacy = tempfile::tempdir().unwrap();
    fs::write(legacy.path().join(METADATA), "old").unwrap();
    fs::write(legacy.path().join(LAYOUT), layout("old").0).unwrap();
    fs::write(legacy.path().join("initial_contents_1"), "old").unwrap();
    assert_snapshot(&SessionCheckpoint::read(legacy.path()).unwrap(), "old");
    write_checkpoint(legacy.path(), "old", &empty).unwrap();
    assert_snapshot(&SessionCheckpoint::read(legacy.path()).unwrap(), "old");
    assert!(
        legacy.path().join(LAYOUT).is_file(),
        "legacy files remain available to readers racing migration"
    );
}

#[test]
fn readers_pin_one_snapshot_and_writers_serialize() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "old", &layout("old")).unwrap();
    let checkpoint = SessionCheckpoint::read(root.path()).unwrap();
    let other_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join(LOCK))
        .unwrap();
    assert!(matches!(
        other_lock.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let barrier = Arc::new(Barrier::new(3));
    let (done, completed) = mpsc::channel();
    let mut writers = Vec::new();
    for value in ["one", "two"] {
        let root = root.path().to_owned();
        let barrier = barrier.clone();
        let done = done.clone();
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            write_checkpoint(&root, value, &layout(value)).unwrap();
            done.send(()).unwrap();
        }));
    }
    barrier.wait();
    assert_snapshot(&checkpoint, "old");
    drop(checkpoint);
    for _ in 0..2 {
        completed.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    for writer in writers {
        writer.join().unwrap();
    }
    let checkpoint = SessionCheckpoint::read(root.path()).unwrap();
    let metadata = fs::read_to_string(checkpoint.metadata_path()).unwrap();
    assert!(metadata == "one" || metadata == "two");
    assert_snapshot(&checkpoint, &metadata);
}

#[test]
fn checkpoint_progress_identifies_a_blocked_publication_without_publishing_early() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "old", &layout("old")).unwrap();
    let old = fs::read_link(root.path().join(CURRENT)).unwrap();
    let (entered, reached) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    std::thread::scope(|scope| {
        let root = root.path();
        scope.spawn(move || {
            let mut phases = Vec::new();
            write_checkpoint_with_progress(root, "new", &layout("new"), |phase| {
                phases.push(phase);
                if phase == "publication" {
                    entered.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
            })
            .unwrap();
            done.send(phases).unwrap();
        });
        reached.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(fs::read_link(root.join(CURRENT)).unwrap(), old);
        assert!(matches!(
            completed.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let phases = completed.recv_timeout(Duration::from_secs(10)).unwrap();
        for expected in [
            "file lock",
            "file sync",
            "staging directory sync",
            "parent directory sync",
            "publication",
            "published directory sync",
            "cleanup",
        ] {
            assert!(phases.contains(&expected), "missing {expected}: {phases:?}");
        }
        assert_snapshot(&SessionCheckpoint::read(root).unwrap(), "new");
    });
}

#[test]
fn external_file_paths_cannot_escape_the_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "../escape",
        "/absolute",
        METADATA,
        LAYOUT,
        ".checkpoint.lock",
    ] {
        let files = BTreeMap::from([(name.to_owned(), "bad".to_owned())]);
        assert!(
            write_checkpoint(root.path(), "metadata", &("layout {}".to_owned(), files)).is_err()
        );
        assert!(!root.path().join(CURRENT).exists());
    }
}
