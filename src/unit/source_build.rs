use super::*;

fn repository(root: &Path) {
    git(root, &["init", "--initial-branch=main"]).unwrap();
    git(root, &["config", "user.name", "Source test"]).unwrap();
    git(root, &["config", "commit.gpgsign", "false"]).unwrap();
    git(root, &["config", "core.hooksPath", "/dev/null"]).unwrap();
    git(root, &["config", "user.email", "source@example.invalid"]).unwrap();
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:zacharyhamm/yinz.git",
        ],
    )
    .unwrap();
    fs::write(root.join("source"), "committed\n").unwrap();
    git(root, &["add", "source"]).unwrap();
    git(root, &["commit", "-m", "source"]).unwrap();
}

#[test]
fn source_main_is_found_through_a_linked_feature_worktree() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main");
    fs::create_dir(&main).unwrap();
    repository(&main);
    let feature = directory.path().join("feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            feature.to_str().unwrap(),
        ],
    )
    .unwrap();
    let (actual, commit) = main_checkout(&feature).unwrap();
    assert_eq!(actual, main.canonicalize().unwrap());
    assert_eq!(commit, git(&main, &["rev-parse", "HEAD"]).unwrap().trim());
    fs::write(main.join("source"), "dirty\n").unwrap();
    assert!(main_checkout(&feature)
        .unwrap_err()
        .to_string()
        .contains("dirty"));
}

#[test]
fn source_main_refuses_missing_checkout_and_changed_origin() {
    let directory = tempfile::tempdir().unwrap();
    repository(directory.path());
    git(directory.path(), &["checkout", "-b", "feature"]).unwrap();
    assert!(main_checkout(directory.path())
        .unwrap_err()
        .to_string()
        .contains("checked out"));
    git(directory.path(), &["checkout", "main"]).unwrap();
    git(
        directory.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/someone/yinz",
        ],
    )
    .unwrap();
    assert!(main_checkout(directory.path())
        .unwrap_err()
        .to_string()
        .contains("no longer points"));
}

#[test]
fn source_targets_deduplicate_and_refuse_unsupported_participants() {
    assert_eq!(
        targets(&[
            "aarch64-unknown-linux-gnu".into(),
            "x86_64-unknown-linux-gnu".into(),
            "aarch64-unknown-linux-gnu".into()
        ])
        .unwrap(),
        vec!["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"]
    );
    for target in [
        "aarch64-apple-darwin",
        "i686-unknown-linux-gnu",
        "../escape",
    ] {
        assert!(targets(&[target.into()]).is_err());
    }
    assert!(targets(&[]).is_err());
}

#[test]
fn archived_source_stays_at_the_pinned_commit_after_main_advances() {
    let directory = tempfile::tempdir().unwrap();
    repository(directory.path());
    let (_, commit) = main_checkout(directory.path()).unwrap();
    fs::write(directory.path().join("source"), "later main\n").unwrap();
    git(directory.path(), &["commit", "-am", "later"]).unwrap();
    let archive = directory.path().join("pinned.tar");
    output(
        Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["archive", "--format=tar", "--output"])
            .arg(&archive)
            .arg(commit),
    )
    .unwrap();
    let snapshot = tempfile::tempdir().unwrap();
    tar::Archive::new(fs::File::open(archive).unwrap())
        .unpack(snapshot.path())
        .unwrap();
    assert_eq!(
        fs::read_to_string(snapshot.path().join("source")).unwrap(),
        "committed\n"
    );
    assert!(!snapshot.path().join("target").exists());
}

#[test]
fn closed_build_pane_takes_precedence_over_a_racing_worker_receipt() {
    for success in [true, false] {
        let receipt =
            serde_json::json!({"success":success,"error":"source build interrupted by SIGHUP"});
        let error = validate_result(&receipt, false, Path::new("build.log")).unwrap_err();
        assert!(error.to_string().contains("was closed"));
        assert!(!error.to_string().contains("pane and log retained"));
    }
    let receipt = serde_json::json!({"success":false,"error":"compiler failed"});
    assert!(validate_result(&receipt, true, Path::new("build.log"))
        .unwrap_err()
        .to_string()
        .contains("compiler failed; pane and log retained"));
}

#[test]
fn source_retry_keeps_pinned_targets_after_a_client_disconnects() {
    let original = targets(&[
        "x86_64-unknown-linux-gnu".into(),
        "aarch64-unknown-linux-gnu".into(),
    ])
    .unwrap();
    let remaining = targets(&["x86_64-unknown-linux-gnu".into()]).unwrap();
    assert_eq!(
        retain_pinned_targets(&original, &remaining).unwrap(),
        original
    );
    assert_eq!(
        retain_pinned_targets(&original, &original).unwrap(),
        original
    );
    assert_eq!(retain_pinned_targets(&[], &remaining).unwrap(), remaining);
    assert!(retain_pinned_targets(&remaining, &original)
        .unwrap_err()
        .to_string()
        .contains("unbuilt source target"));
}
