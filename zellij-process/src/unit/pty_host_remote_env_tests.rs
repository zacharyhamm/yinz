//! The policy half of remote-pane environments: what crosses the wire, what is
//! recomputed on the far machine, and what is refused outright.

use super::*;

fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn a_test_login() -> LoginSource {
    LoginSource {
        name: Some("test".to_owned()),
        uid: Some(1000),
        home: Some(PathBuf::from("/home/test")),
        shell: Some(PathBuf::from("/bin/bash")),
    }
}

/// The allowlist, stated as a test: the terminal and the locale describe the
/// asking side and cross; everything else either names a path on the wrong
/// filesystem or a session on the wrong machine.
#[test]
fn only_the_terminal_and_the_locale_cross_the_wire() {
    let carried = select_carried(vars(&[
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("LANG", "en_US.UTF-8"),
        ("LC_TIME", "C"),
        ("PATH", "/home/me/.local/bin"),
        ("HOME", "/home/me"),
        ("SHELL", "/usr/bin/fish"),
        ("PWD", "/home/me/src"),
        ("TMPDIR", "/tmp/me"),
        ("DISPLAY", ":0"),
        ("SSH_AUTH_SOCK", "/tmp/ssh-agent.sock"),
        ("SSH_CONNECTION", "10.0.0.1 22 10.0.0.2 4242"),
        ("ZELLIJ", "0"),
        ("ZELLIJ_SESSION_NAME", "here"),
        ("ZELLIJ_PANE_ID", "7"),
    ]));

    let names: Vec<&str> = carried.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["COLORTERM", "LANG", "LC_TIME", "TERM"]);
}

/// Locale variables are open-ended, so the rule is a prefix rather than a list -
/// a `LC_*` this code has never heard of still describes the person typing.
#[test]
fn a_locale_variable_this_code_has_never_heard_of_still_crosses() {
    let carried = select_carried(vars(&[("LC_FUTURE_THING", "en_GB.UTF-8")]));
    assert_eq!(
        carried,
        vec![("LC_FUTURE_THING".to_owned(), "en_GB.UTF-8".to_owned())]
    );
}

/// A locale name that cannot be spelled in UTF-8 cannot cross as JSON either.
/// Dropping it lets the far side fall back to its default; mangling it would set
/// a locale that does not exist.
#[test]
fn a_value_that_is_not_utf8_is_dropped_rather_than_mangled() {
    use std::os::unix::ffi::OsStringExt;

    let carried = select_carried(vec![
        (OsString::from("LANG"), OsString::from_vec(vec![0xff, 0xfe])),
        (OsString::from("TERM"), OsString::from("xterm")),
    ]);
    assert_eq!(carried, vec![("TERM".to_owned(), "xterm".to_owned())]);
}

/// The far machine's own passwd entry is the only authority for these, because it
/// is the only one that does not depend on how the broker happened to be started.
#[test]
fn the_login_environment_comes_from_the_passwd_entry() {
    let env = login_env(&a_test_login());

    assert_eq!(
        env,
        vec![
            ("HOME".to_owned(), "/home/test".to_owned()),
            ("USER".to_owned(), "test".to_owned()),
            ("LOGNAME".to_owned(), "test".to_owned()),
            ("MAIL".to_owned(), "/var/mail/test".to_owned()),
            ("SHELL".to_owned(), "/bin/bash".to_owned()),
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ]
    );
}

#[test]
fn roots_path_carries_the_sbin_directories() {
    let root = LoginSource {
        name: Some("root".to_owned()),
        uid: Some(0),
        home: Some(PathBuf::from("/root")),
        shell: Some(PathBuf::from("/bin/sh")),
    };
    let env = login_env(&root);
    let path = env.iter().find(|(name, _)| name == "PATH").expect("a PATH");
    assert_eq!(
        path.1,
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    );
}

/// A passwd lookup that found nothing degrades to a bare PATH rather than
/// inventing a home directory or a `/var/mail/` with nothing after it.
#[test]
fn a_passwd_entry_with_nothing_in_it_still_yields_a_path() {
    let env = login_env(&LoginSource::default());
    assert_eq!(
        env,
        vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())]
    );
}

/// A sender that bypasses the sampling helper still cannot replace the remote
/// login environment or reintroduce an unrelated SSH connection.
#[test]
fn the_receiver_rejects_carried_values_outside_the_allowlist() {
    let composed = compose(
        &a_test_login(),
        vec![
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
            ("LC_TIME".to_owned(), "C".to_owned()),
            ("PATH".to_owned(), "/a/local/bin".to_owned()),
            ("HOME".to_owned(), "/a/local/home".to_owned()),
            (
                "SHELL".to_owned(),
                "/a/shell/on/the/wrong/machine".to_owned(),
            ),
            (
                "SSH_CONNECTION".to_owned(),
                "unrelated connection".to_owned(),
            ),
        ],
    );

    let mut expected = login_env(&a_test_login());
    expected.extend([
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
        ("LC_TIME".to_owned(), "C".to_owned()),
    ]);
    assert_eq!(composed, expected);
}

/// Nothing the broker composes claims a connection the pane does not have. See
/// `SCRUBBED_VARS` for why these are absent rather than approximated.
#[test]
fn nothing_composed_for_a_pane_names_an_ssh_connection() {
    let composed = compose(
        &a_test_login(),
        select_carried(vars(&[
            ("TERM", "xterm"),
            ("SSH_CLIENT", "10.0.0.1 4242 22"),
            ("SSH_CONNECTION", "10.0.0.1 4242 10.0.0.2 22"),
        ])),
    );
    assert!(
        composed.iter().all(|(name, _)| !name.starts_with("SSH_")),
        "composed a variable describing an ssh connection: {composed:?}"
    );
}

/// Every scrubbed name is one the allowlist would have refused anyway - the two
/// lists must not drift into disagreeing about a variable.
#[test]
fn nothing_scrubbed_is_also_carried() {
    for name in SCRUBBED_VARS {
        assert!(!is_carried(name), "{name} is both carried and scrubbed");
    }
}

/// The pre-flight command lookup in the supervisor has to search the `PATH` the
/// child will actually get, not its own.
#[test]
fn the_childs_path_is_read_back_from_what_was_composed() {
    let composed = compose(&a_test_login(), vec![]);
    assert_eq!(
        path_in(&composed),
        Some(OsStr::new("/usr/local/bin:/usr/bin:/bin"))
    );
    assert_eq!(path_in(&[]), None);
}
