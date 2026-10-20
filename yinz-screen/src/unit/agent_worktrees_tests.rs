use super::*;
use std::process::Command;
use yinz_runtime::channels;

fn git(cwd: &Path, args: &[&str]) {
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
}

#[test]
fn same_worktree_resolution_matches_subdirectories_and_aliases_but_not_linked_worktrees() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    std::fs::create_dir_all(main.join("nested/deep")).unwrap();
    git(&main, &["init"]);
    git(
        &main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let linked = temp.path().join("linked");
    git(
        &main,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&main, &alias).unwrap();
    crate::global_async_runtime::get_tokio_runtime().block_on(async {
        let root = resolve_worktree(&main).await.unwrap();
        assert_eq!(root, main.canonicalize().unwrap());
        assert_eq!(
            resolve_worktree(&main.join("nested/deep")).await.unwrap(),
            root
        );
        assert_eq!(resolve_worktree(&alias.join("nested")).await.unwrap(), root);
        assert_ne!(resolve_worktree(&linked).await.unwrap(), root);
        assert!(resolve_worktree(temp.path()).await.is_err());
        assert!(resolve_worktree(&temp.path().join("missing"))
            .await
            .is_err());
    });
}

#[test]
fn same_worktree_cache_deduplicates_pending_and_failed_lookups_and_rejects_stale_results() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_path_buf();
    let (sender, _receiver) = channels::unbounded();
    let sender = SenderWithContext::new(sender);
    let mut cache = Worktrees::default();
    let directories = HashSet::from([cwd.clone()]);
    cache.refresh(directories.clone(), Some(sender.clone()));
    let first = cache.entries[&cwd].generation;
    assert!(cache.entries[&cwd].root.is_none());
    assert!(cache.entries[&cwd].task.is_some());
    cache.refresh(directories.clone(), Some(sender.clone()));
    assert_eq!(cache.generation, first);
    assert!(cache.complete(&cwd, first, None));
    cache.refresh(directories.clone(), Some(sender.clone()));
    assert_eq!(cache.generation, first, "failed lookups stay cached");
    cache.refresh(HashSet::new(), Some(sender.clone()));
    assert!(cache.entries.is_empty());
    assert!(!cache.complete(&cwd, first, Some(cwd.clone())));
    cache.refresh(directories, Some(sender));
    assert!(!cache.complete(&cwd, first, Some(cwd.clone())));
    let second = cache.entries[&cwd].generation;
    assert!(second > first);
    assert!(cache.complete(&cwd, second, Some(cwd.clone())));
    assert_eq!(cache.entries[&cwd].root.as_ref(), Some(&cwd));
}

#[test]
fn same_worktree_lookup_times_out_a_stalled_git_process() {
    let temp = tempfile::tempdir().unwrap();
    yinz_test_support::write_warm_script(&temp.path().join("git"), "exec /bin/sleep 60");
    let previous_path = std::env::var_os("PATH");
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("PATH", temp.path()) };
    let (sender, receiver) = channels::unbounded();
    let mut cache = Worktrees::default();
    cache.refresh(
        HashSet::from([temp.path().to_path_buf()]),
        Some(SenderWithContext::new(sender)),
    );
    let completion = receiver.recv_timeout(Duration::from_secs(15));
    if let Some(previous_path) = previous_path {
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("PATH", previous_path) };
    } else {
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("PATH") };
    }
    assert!(matches!(
        completion.unwrap().0,
        ScreenInstruction::AgentWorktreeResolved { root: None, .. }
    ));
}
