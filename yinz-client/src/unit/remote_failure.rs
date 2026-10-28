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
        "YINZ_REMOTE_ERROR \"session_exists\"",
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
    // Any permanent category counts, not only the first one quoted.
    assert!(permanent_diagnostic(
        "YINZ_REMOTE_ERROR \"unresponsive\"\nYINZ_REMOTE_ERROR \"session_missing\""
    ));
}

#[test]
fn the_reported_error_is_the_line_after_the_category() {
    assert_eq!(
        reported_error(
            "Error: from a login script\nYINZ_REMOTE_ERROR \"session_exists\"\nError: taken\n"
        ),
        Some((BootstrapCategory::SessionExists, "taken"))
    );
    assert_eq!(
        reported_error("Error: from a login script\nYINZ_REMOTE_ERROR \"session_exists\"\n"),
        None
    );
}

#[test]
fn markerless_bootstrap_diagnostics_from_compatible_peers_are_permanent() {
    let name = yinz_types::internal_ids::SessionName::new("work session");
    for diagnostic in [
        format!("remote session {name:?} does not exist; use --create to create or resurrect it"),
        format!("permission denied probing remote session {name:?}"),
        "remote session \"work\" does not exist; use --create to create or resurrect it".into(),
        "permission denied probing remote session \"work\"".into(),
    ] {
        for prefix in ["", "SSH bootstrap failed: ssh exited (1): Error: "] {
            let diagnostic = format!("{prefix}{diagnostic}");
            let failure = ConnectionFailure::classify(
                anyhow::anyhow!(diagnostic.clone()),
                &AtomicBool::new(false),
            );
            assert_eq!(failure.kind, FailureKind::Permanent, "{diagnostic}");
            assert_eq!(failure.diagnostic, diagnostic);
            assert_eq!(
                ConnectionFailure::classify(anyhow::anyhow!(diagnostic), &AtomicBool::new(true))
                    .kind,
                FailureKind::Cancelled
            );
        }
    }
    for diagnostic in [
        format!("remote session {name:?} is unresponsive; refusing to replace it"),
        "remote session is not ready: Connection refused".into(),
        "configuration file does not exist".into(),
        "remote session log file does not exist".into(),
        "remote session \"work\" does not exist on this proxy".into(),
    ] {
        assert!(!permanent_diagnostic(&diagnostic), "{diagnostic}");
    }
}
