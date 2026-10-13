use super::*;
#[test]
fn nested_config_ignores_legacy_and_rejects_unsafe_directories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".yinz.kdl"), "invalid legacy configuration").unwrap();
    assert!(access_root(dir.path(), None).unwrap().source.is_none());
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join(".yinz")).unwrap();
    assert!(access_root(dir.path(), None).is_err());
    assert!(access_root(dir.path(), Some(("", None))).is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    std::fs::remove_file(dir.path().join(".yinz")).unwrap();
    std::fs::write(dir.path().join(".yinz"), "not a directory").unwrap();
    assert!(access_root(dir.path(), None).is_err());
}

#[test]
fn invalid_configuration_can_be_corrected_without_losing_concurrent_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .arg(dir.path())
        .output()
        .unwrap();
    std::fs::create_dir(dir.path().join(".yinz")).unwrap();
    let path = dir.path().join(".yinz/agent-exit.kdl");
    std::fs::write(&path, "agent_exit { invalid").unwrap();
    let snapshot = access(dir.path(), None).unwrap();
    assert!(snapshot.error.is_some());
    assert_eq!(snapshot.config.default_branch.as_deref(), Some("main"));
    assert!(access(dir.path(), Some(("agent_exit {}", Some("stale")))).is_err());
    let corrected = access(
        dir.path(),
        Some((
            "agent_exit { default_branch \"trunk\"; }",
            snapshot.source.as_deref(),
        )),
    )
    .unwrap();
    assert!(corrected.error.is_none());
    assert_eq!(corrected.config.default_branch.as_deref(), Some("trunk"));
}

#[test]
fn stale_writes_and_symlinks_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .arg("init")
        .arg(dir.path())
        .output()
        .unwrap()
        .status
        .success());
    let source = "agent_exit { default_branch \"trunk\"; }";
    access(dir.path(), Some((source, None))).unwrap();
    assert!(access(dir.path(), Some(("", None))).is_err());
    assert_eq!(
        access(dir.path(), None).unwrap().source.as_deref(),
        Some(source)
    );
    access(dir.path(), Some(("", Some(source)))).unwrap();
    std::fs::remove_file(dir.path().join(".yinz/agent-exit.kdl")).unwrap();
    std::os::unix::fs::symlink("missing", dir.path().join(".yinz/agent-exit.kdl")).unwrap();
    assert!(access(dir.path(), None).is_err());
    assert!(access(dir.path(), Some((source, None))).is_err());
}

#[test]
fn config_writes_recover_after_process_exit_without_worktree_artifacts() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    // Run this test in a child so SIGKILL proves recovery without Rust destructors.
    if let Some(path) = std::env::var_os("YINZ_TEST_CONFIG_LOCK") {
        let lock = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        lock.try_lock().unwrap();
        println!("config lock acquired");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        panic!("parent must kill the lock holder");
    }
    let dir = tempfile::tempdir().unwrap();
    let git = |cwd: &std::path::Path, args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let primary = dir.path().join("primary");
    std::fs::create_dir(&primary).unwrap();
    git(&primary, &["init", "-b", "main"]);
    git(
        &primary,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let linked = dir.path().join("linked checkout");
    git(
        &primary,
        &["worktree", "add", "-b", "topic", linked.to_str().unwrap()],
    );
    assert_eq!(root(&linked).unwrap(), primary.canonicalize().unwrap());
    let source = "agent_exit { default_branch \"trunk\"; }";
    access(&linked, Some((source, None))).unwrap();
    let snapshot = access(&linked, None).unwrap();
    assert_eq!(snapshot.root, primary.canonicalize().unwrap());
    assert_eq!(snapshot.config.default_branch.as_deref(), Some("trunk"));
    assert!(!linked.join(".yinz/agent-exit.kdl").exists());
    std::fs::remove_file(primary.join(".yinz/agent-exit.kdl")).unwrap();
    for root in [primary.as_path(), linked.as_path()] {
        access_root(root, Some(("", None))).unwrap();
        let gitdir = std::path::PathBuf::from(
            git(root, &["rev-parse", "--absolute-git-dir"]).trim_end_matches('\n'),
        );
        // libtest names a test by its path below the crate root, so derive it rather than
        // spell it out: a stale name matches nothing and the child never takes the lock.
        let test = format!(
            "{}::config_writes_recover_after_process_exit_without_worktree_artifacts",
            module_path!().split_once("::").unwrap().1
        );
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--nocapture"])
            .env("YINZ_TEST_CONFIG_LOCK", gitdir.join("agent-exit.kdl.lock"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let ready = std::io::BufReader::new(child.stdout.take().unwrap())
            .lines()
            .any(|line| line.unwrap().contains("config lock acquired"));
        assert!(ready, "child did not acquire lock");
        let refused = access_root(root, Some(("agent_exit {}", Some("")))).unwrap_err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(refused.contains("Cannot lock"), "{refused}");
        // A temporary file abandoned by a dead writer neither blocks nor gets staged.
        std::fs::write(gitdir.join(".tmp-abandoned"), "partial").unwrap();
        access_root(root, Some(("agent_exit {}", Some("")))).unwrap();
        let candidates = git(root, &["ls-files", "--others", "--exclude-standard"]);
        assert_eq!(candidates, ".yinz/agent-exit.kdl\n");

        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let writers: Vec<_> = ["", "agent_exit { default_branch \"trunk\"; }"]
                .into_iter()
                .map(|source| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        access_root(root, Some((source, Some("agent_exit {}"))))
                    })
                })
                .collect();
            assert_eq!(
                writers
                    .into_iter()
                    .map(|writer| writer.join().unwrap())
                    .filter(Result::is_ok)
                    .count(),
                1
            );
        });
        std::fs::write(root.join("agent-exit.kdl.lock"), "legacy lock").unwrap();
        let previous = access_root(root, None).unwrap().source;
        access_root(root, Some(("", previous.as_deref()))).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("agent-exit.kdl.lock")).unwrap(),
            "legacy lock"
        );
        std::fs::remove_file(root.join("agent-exit.kdl.lock")).unwrap();
    }
}

#[test]
fn default_branch_defaults_and_custom_values() {
    for source in ["", "agent_exit {}"] {
        assert_eq!(
            parse(source).unwrap().default_branch.as_deref(),
            Some("main")
        );
    }
    for branch in ["trunk", "release/stable"] {
        assert_eq!(
            parse(&format!("agent_exit {{ default_branch \"{branch}\"; }}"))
                .unwrap()
                .default_branch
                .as_deref(),
            Some(branch)
        );
    }
    let missing = tempfile::tempdir().unwrap();
    assert_eq!(
        access_root(missing.path(), None)
            .unwrap()
            .config
            .default_branch
            .as_deref(),
        Some("main")
    );
}

#[test]
fn malformed_settings_and_unsafe_branches_are_refused() {
    for source in [
        "agent_exit",
        "agent_exit {",
        "hooks {}",
        "agent_exit {}\nagent_exit {}",
        "agent_exit \"arg\" {}",
        "(typed)agent_exit {}",
        "agent_exit { workflow \"removed\"; }",
        "agent_exit { default_branch; }",
        "agent_exit { default_branch 1; }",
        "agent_exit { default_branch \"main\" \"trunk\"; }",
        "agent_exit { default_branch branch=\"main\"; }",
        "agent_exit { default_branch (text)\"main\"; }",
        "agent_exit { default_branch \"main\" {}; }",
        "agent_exit { default_branch \"main\"; default_branch \"trunk\"; }",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
    for branch in [
        "", "HEAD", "--bad", "@", "a..b", "a@{b", "/main", "main/", ".main", "main.", "a//b",
        "a/.b", "a.lock", "a b", "a\\b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\0b", "a\nb",
    ] {
        let source = format!(
            "agent_exit {{ default_branch {}; }}",
            serde_json::to_string(branch).unwrap()
        );
        assert!(parse(&source).is_err(), "{branch:?}");
    }
}
