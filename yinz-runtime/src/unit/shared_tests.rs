use super::*;

#[test]
fn terminal_titles_cannot_terminate_the_osc_or_inject_terminal_commands() {
    let title = make_terminal_title("日本語\x07\x1b]52;c;payload\x1b\\\r\n\u{9c}\u{2028}", None);
    let payload = title
        .strip_prefix("\x1b]0;")
        .unwrap()
        .strip_suffix('\x07')
        .unwrap();
    assert!(payload.contains("日本語"));
    assert!(!payload.chars().any(char::is_control));
    assert!(!payload.contains('\u{2028}'));
    assert!(make_terminal_title("plain title", None).ends_with("plain title\x07"));
}

#[test]
fn terminal_titles_lead_with_the_hostname() {
    let osc = |title: &str| format!("\x1b]0;{title}\x07");
    assert_eq!(
        format_terminal_title("box", Some("main"), "vim"),
        osc("box:main | vim")
    );
    assert_eq!(
        format_terminal_title("box", Some("main"), ""),
        osc("box:main")
    );
    assert_eq!(format_terminal_title("box", None, "vim"), osc("box | vim"));
    assert_eq!(format_terminal_title("box", None, ""), osc("box"));
    assert_eq!(
        format_terminal_title("", Some("main"), "vim"),
        osc("main | vim")
    );
}

#[cfg(unix)]
#[test]
fn secure_dirs_are_created_with_missing_parents_and_owner_only_access() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("missing").join("nested").join("sockets");
    ensure_secure_dir(&dir).unwrap();
    assert!(dir.is_dir());
    assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);

    // A second call tightens a directory that already exists with looser access.
    set_permissions(&dir, 0o755).unwrap();
    ensure_secure_dir(&dir).unwrap();
    assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);
}
