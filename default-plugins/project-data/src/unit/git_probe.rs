use super::*;

/// The checkout out of an outcome, or a panic naming what came back instead.
fn checkout(outcome: ProbeOutcome) -> Repository {
    match outcome {
        ProbeOutcome::Checkout(repository) => repository,
        other => panic!("expected a checkout, got {other:?}"),
    }
}

#[test]
fn repository_prefers_push_default_and_detects_a_linked_worktree() {
    let repository = checkout(repository(
        Some(0),
        b"/worktrees/topic\n/repos/agent/.git/worktrees/topic\n/repos/agent/.git\ntopic\n",
        b"remote.origin.url\nhttps://example/origin\0remote.fork.url\nhttps://example/fork\0remote.pushdefault\nfork\0branch.topic.remote\norigin\0",
    ));

    assert_eq!(repository.root, "/worktrees/topic");
    assert_eq!(repository.remote.as_deref(), Some("https://example/fork"));
    assert_eq!(repository.worktree_status, Some(WorktreeStatus::Linked));
    assert_eq!(repository.branch.as_deref(), Some("topic"));
}

/// A detached HEAD is on no branch, and the word Git answers with is not the
/// name of one.
#[test]
fn repository_reports_a_detached_head_as_no_branch() {
    let repository = checkout(repository(
        Some(0),
        b"/repos/agent\n/repos/agent/.git\n/repos/agent/.git\nHEAD\n",
        b"remote.origin.url\nhttps://example/origin\0",
    ));

    assert_eq!(repository.worktree_status, Some(WorktreeStatus::Primary));
    assert_eq!(repository.branch, None);
}

/// The case that silently denied a whole repository its project: `git
/// rev-parse` writes the identity and *then* fails on the unborn ref, so the
/// exit code alone says "not a repository" about a repository.
#[test]
fn a_checkout_with_no_commits_keeps_its_identity() {
    let repository = checkout(repository(
        Some(128),
        b"/work/fresh\n/work/fresh/.git\n/work/fresh/.git\nHEAD\n",
        b"",
    ));

    assert_eq!(repository.root, "/work/fresh");
    assert_eq!(repository.worktree_status, Some(WorktreeStatus::Primary));
    assert_eq!(repository.common_dir.as_deref(), Some("/work/fresh/.git"));
    assert_eq!(repository.branch, None);
    assert!(repository.is_checkout(), "a project can be made from it");
}

/// Git ran and answered. The folder is not a repository, and saying so is
/// what stops the daemon probing it again on every event.
#[test]
fn a_folder_outside_a_repository_is_conclusively_not_one() {
    assert_eq!(
        repository(Some(128), b"", b""),
        ProbeOutcome::NotARepository
    );
}

/// Nothing was learned. Recording this as a folder would be a guess that
/// outlives the reason for it.
#[test]
fn a_probe_that_could_not_run_is_not_recorded() {
    assert_eq!(repository(None, b"", b""), ProbeOutcome::Failed);
    assert_eq!(repository(Some(2), b"", b""), ProbeOutcome::Failed);
}

/// Output that is not what we asked for is not something to guess at: a
/// relative root would name a project after whatever its last part happened
/// to be.
#[test]
fn output_that_is_not_the_shape_we_asked_for_is_a_failure() {
    assert_eq!(
        repository(Some(0), b"not/absolute\n.git\n.git\nmain\n", b""),
        ProbeOutcome::Failed
    );
}
