use super::*;

use hegel::generators;
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use yinz_runtime::shared::ensure_secure_dir;

#[test]
fn process_ids_cannot_signal_a_process_group_by_wrapping() {
    use yinz_types::internal_ids::ProcessId;
    for raw in [0, 1, 2147483648, u32::MAX] {
        assert!(process_pid(ProcessId::new(raw)).is_err());
    }
    assert_eq!(process_pid(ProcessId::new(2)).unwrap().as_raw(), 2);
    assert_eq!(
        process_pid(ProcessId::new(2147483647)).unwrap().as_raw(),
        i32::MAX
    );
}

#[test]
fn kernel_process_ids_preserve_the_positive_signed_range() {
    for raw in [2, std::process::id(), i32::MAX as u32] {
        assert_eq!(kernel_pid(ProcessId::new(raw)).as_raw() as u32, raw);
    }
}

/// Pure: nothing here reaches `kill(2)`. A test that sent a real signal to the group of pid 1
/// would, the day this guard regressed, SIGKILL every process of whoever ran the suite.
#[test]
fn a_process_group_selector_is_never_every_process_or_our_own_group() {
    use nix::unistd::Pid;
    for raw in [i32::MIN, -2, -1, 0, 1] {
        assert!(
            process_group(Pid::from_raw(raw)).is_err(),
            "{raw} became a group selector"
        );
    }
    assert_eq!(process_group(Pid::from_raw(2)).unwrap().as_raw(), -2);
    assert_eq!(
        process_group(Pid::from_raw(i32::MAX)).unwrap().as_raw(),
        -i32::MAX
    );
}

#[hegel::test]
fn no_process_id_off_the_wire_selects_every_process(tc: hegel::TestCase) {
    let raw = tc.draw(generators::integers::<u32>());
    let selector = process_pid(ProcessId::new(raw)).and_then(process_group);
    if let Ok(group) = selector {
        assert!(
            group.as_raw() < -1,
            "process ID {raw} became selector {group}"
        );
    }
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

/// The supervisor branch runs before clap, the logger and the cache-folder
/// setup, so it has to be sure: a `--pty-host` anywhere but immediately after
/// the binary name belongs to an ordinary session. `main` carries a guard for
/// exactly that case and nothing verified the rule it relies on.
#[test]
fn only_a_leading_pty_host_flag_starts_a_supervisor() {
    assert_eq!(
        invocation_from_args(argv(&["yinz", PTY_HOST_FLAG, "session/3"])),
        Some("session/3".to_owned())
    );
    assert_eq!(
        invocation_from_args(argv(&["yinz", "-s", "mysession", PTY_HOST_FLAG, "x"])),
        None,
        "a flag after another argument is not a supervisor invocation"
    );
    assert_eq!(invocation_from_args(argv(&["yinz"])), None);
    assert_eq!(invocation_from_args(argv(&[])), None);
}

/// The label is only there to make `ps` readable, so a missing one is an
/// empty label rather than a refusal — refusing would strand a pane.
#[test]
fn a_pty_host_flag_with_no_label_still_starts_a_supervisor() {
    assert_eq!(
        invocation_from_args(argv(&["yinz", PTY_HOST_FLAG])),
        Some(String::new())
    );
}

/// `--pty-host` is drawn far more often than random text would produce it, so
/// that it lands at every position — including the one that counts.
#[hegel::composite]
fn argvs(tc: &hegel::TestCase) -> Vec<String> {
    let count = tc.draw(generators::integers::<usize>().max_value(5));
    (0..count)
        .map(|_| {
            if tc.draw(generators::booleans()) {
                PTY_HOST_FLAG.to_owned()
            } else {
                tc.draw(generators::text())
            }
        })
        .collect()
}

#[hegel::test]
fn the_flag_is_recognised_at_the_second_position_and_nowhere_else(tc: hegel::TestCase) {
    let args = tc.draw(argvs());
    let expected = if args.get(1).map(String::as_str) == Some(PTY_HOST_FLAG) {
        Some(args.get(2).cloned().unwrap_or_default())
    } else {
        None
    };
    assert_eq!(
        invocation_from_args(args.clone()),
        expected,
        "argv {args:?} was classified wrongly"
    );
}

#[test]
fn privileged_socket_paths_get_owner_only_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("sockets");
    ensure_secure_dir(&dir).unwrap();
    assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);

    for (name, mode) in [("supervisor", 0o1700), ("broker", 0o600)] {
        let path = dir.join(name);
        let _listener = UnixListener::bind(&path).unwrap();
        secure_socket(&path, mode).unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o7777, mode);
    }
}

#[test]
fn failed_socket_permission_finalization_removes_the_socket() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("socket");
    let _listener = UnixListener::bind(&path).unwrap();

    let error = secure_socket_with(&path, 0o600, |_, _| {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();

    assert!(error.to_string().contains("failed to set permissions 0600"));
    assert!(!path.exists());
    assert!(std::os::unix::net::UnixStream::connect(path).is_err());
}
