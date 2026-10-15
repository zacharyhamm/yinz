use super::*;
use tempfile::TempDir;

/// `resolve_from` rather than `host_id` throughout, for two reasons. The public function caches
/// in a process-wide `OnceLock`, so only the first call in a test binary would decide anything
/// and every test after it would assert against the first one's data dir. And the environment is
/// process-global too: a test that set `YINZ_HOST_ID` would be seen by every test running
/// beside it under a plain `cargo test`, which shares one process. `resolve_from` takes both out
/// of the picture and is the whole of the behaviour otherwise.
#[test]
fn an_id_is_minted_and_persisted_when_the_data_dir_has_none() {
    let dir = TempDir::new().unwrap();
    let first = resolve_from(None, dir.path(), None);
    assert!(is_valid_host_id(&first), "minted {:?} is not valid", first);
    assert!(dir.path().join(HOST_ID_FILE).exists());
    assert_eq!(
        resolve_from(None, dir.path(), None),
        first,
        "a second resolve minted a new id instead of reading the stored one"
    );
}

#[test]
fn a_late_first_start_adopts_the_published_identity() {
    let dir = TempDir::new().unwrap();
    // Both startups observed a missing file before either tried to publish its candidate.
    assert!(read_file(dir.path()).is_none());
    assert_eq!(persist_id(dir.path(), "first").unwrap(), "first");
    assert_eq!(persist_id(dir.path(), "late").unwrap(), "first");
    assert_eq!(read_file(dir.path()).as_deref(), Some("first"));
}

#[test]
fn concurrent_first_starts_share_one_identity() {
    let dir = TempDir::new().unwrap();
    let barrier = std::sync::Barrier::new(8);
    let ids = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    resolve_from(None, dir.path(), None)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    let stored = read_file(dir.path()).unwrap();
    assert!(ids.iter().all(|id| id == &stored), "{ids:?}");
}

#[test]
fn two_data_dirs_get_two_ids() {
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    assert_ne!(
        resolve_from(None, one.path(), None),
        resolve_from(None, two.path(), None)
    );
}

/// The property the whole module exists for: reinstalling yinz does not disturb the data dir,
/// so whatever else changes around it, an id already on disk is the id that comes back.
#[test]
fn a_stored_id_survives_anything_that_leaves_the_data_dir_alone() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "stable-id").unwrap();
    std::fs::write(dir.path().join("other-data"), b"unrelated").unwrap();
    assert_eq!(resolve_from(None, dir.path(), None), "stable-id");
}

#[test]
fn the_config_option_wins_over_a_stored_id() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "stored").unwrap();
    assert_eq!(
        resolve_from(None, dir.path(), Some("configured")),
        "configured"
    );
    // ...and does not overwrite what is stored, so unsetting it restores the old identity
    // rather than stranding the rows written under it.
    assert_eq!(resolve_from(None, dir.path(), None), "stored");
}

#[test]
fn an_invalid_configured_id_falls_through_rather_than_being_used() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "stored").unwrap();
    assert_eq!(resolve_from(None, dir.path(), Some("has:colon")), "stored");
    assert_eq!(resolve_from(None, dir.path(), Some("")), "stored");
}

/// A file holding something unusable is treated as no file at all. Refusing to start a server
/// over a corrupt id would be a worse outcome than minting a new one.
#[test]
fn a_corrupt_id_file_is_replaced() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "not a valid id!").unwrap();
    let resolved = resolve_from(None, dir.path(), None);
    assert!(is_valid_host_id(&resolved));
    assert_eq!(
        resolve_from(None, dir.path(), None),
        resolved,
        "the fix was not stored"
    );
}

#[test]
fn surrounding_whitespace_in_the_file_is_ignored() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "  padded-id \n").unwrap();
    assert_eq!(resolve_from(None, dir.path(), None), "padded-id");
}

/// An unwritable data dir costs the id its persistence, not the server its startup.
#[test]
fn an_unwritable_data_dir_still_yields_an_id() {
    let dir = TempDir::new().unwrap();
    let data_dir = dir.path().join("nested");
    std::fs::write(&data_dir, b"a file where a directory should be").unwrap();
    let resolved = resolve_from(None, &data_dir, None);
    assert!(is_valid_host_id(&resolved));
}

#[test]
fn the_env_var_wins_over_everything() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "stored").unwrap();
    assert_eq!(
        resolve_from(Some("from-env"), dir.path(), Some("configured")),
        "from-env"
    );
}

#[test]
fn an_invalid_env_var_falls_through_rather_than_being_used() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(HOST_ID_FILE), "stored").unwrap();
    assert_eq!(resolve_from(Some("has:colon"), dir.path(), None), "stored");
    assert_eq!(resolve_from(Some(""), dir.path(), None), "stored");
}

#[test]
fn what_counts_as_a_host_id() {
    for ok in ["a", "A1", "with-dash", "with_underscore", &"x".repeat(64)] {
        assert!(is_valid_host_id(ok), "{:?} should be valid", ok);
    }
    for bad in [
        "",
        "has:colon",
        "has space",
        "has/slash",
        "has.dot",
        "nul\0byte",
        &"x".repeat(65),
    ] {
        assert!(!is_valid_host_id(bad), "{:?} should be invalid", bad);
    }
}

/// The ids this module mints must themselves pass its own validation, and must not collide.
#[test]
fn a_minted_id_is_valid_and_unique() {
    let one = mint();
    assert!(is_valid_host_id(&one), "minted {:?} is not valid", one);
    assert_ne!(one, mint());
}

#[test]
fn display_hostnames_keep_only_the_first_label() {
    assert_eq!(short_hostname("box.example.com").as_deref(), Some("box"));
    assert_eq!(short_hostname("box").as_deref(), Some("box"));
    assert_eq!(short_hostname(""), None);
    assert_eq!(short_hostname(".local"), None);
}
