use super::*;

#[test]
fn only_confirmed_ssh_failures_are_permanent() {
    for diagnostic in [
        "ssh exited (255): Connection refused",
        "ssh exited (255): Connection timed out",
        "ssh exited (255): Connection reset by peer",
        "ssh exited (255): unknown proxy failure",
        "Permission denied opening a local log",
        "forwarding permissions and server responsiveness",
        "YINZ_REMOTE_ERROR \"unresponsive\"",
        "YINZ_REMOTE_ERROR \"other\"",
    ] {
        assert!(!permanent_diagnostic(diagnostic), "{diagnostic}");
    }
    for diagnostic in [
        "user@host: Permission denied (publickey).",
        "Host key verification failed.",
        "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!",
        "open failed: administratively prohibited: open failed",
        "sh: 1: yinz: not found",
        "bash: yinz: command not found",
        "Yinz compatibility mismatch",
        "YINZ_REMOTE_ERROR \"session_missing\"",
        "YINZ_REMOTE_ERROR \"permission\"",
        "YINZ_REMOTE_ERROR \"configuration\"",
    ] {
        assert!(permanent_diagnostic(diagnostic), "{diagnostic}");
    }
}

#[test]
fn cancellation_wins_over_permanent_diagnostics_and_unknown_diagnostics_are_retained() {
    let stop = AtomicBool::new(true);
    let failure =
        ConnectionFailure::classify(anyhow::anyhow!("Host key verification failed"), &stop);
    assert_eq!(failure.kind, FailureKind::Cancelled);
    let failure = ConnectionFailure::classify(
        anyhow::anyhow!("unknown SSH 255 failure"),
        &AtomicBool::new(false),
    );
    assert_eq!(failure.kind, FailureKind::Retryable);
    assert_eq!(failure.diagnostic, "unknown SSH 255 failure");
}

#[test]
fn bootstrap_categories_survive_the_ssh_error_context_prefix() {
    assert!(permanent_diagnostic("SSH bootstrap failed: YINZ_REMOTE_ERROR \"session_missing\"\nError: session does not exist"));
}
