use super::*;

#[test]
fn worker_transport_rejects_abnormal_exit_and_invalid_output() {
    for script in [
        "printf garbage",
        "printf '{\"Ok\":{\"Emitted\":{\"text\":\"ok\",\"source_map\":null}}}'; exit 7",
        "kill -KILL $$",
    ] {
        assert!(exchange(Command::new("/bin/sh").args(["-c", script]), b"", DEADLINE).is_err());
    }
    assert!(exchange(&mut Command::new("/does/not/exist"), b"", DEADLINE).is_err());
}

#[test]
fn worker_timeout_reaps_the_child_even_when_input_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let error = exchange(
        Command::new("/bin/sh")
            .args(["-c", "echo $$ > \"$1\"; exec sleep 60", "worker"])
            .arg(&pid_file),
        &vec![b' '; 1024 * 1024],
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error:#}");
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[cfg(target_os = "linux")]
#[test]
fn worker_timeout_kills_descendants_that_hold_its_pipes_open() {
    // Act as init for this test so we can inspect the orphan's exit status without
    // depending on the machine's init process to reap it promptly.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("descendant");
    let result = supervise(
        Command::new("/bin/sh")
            .args(["-c", "sleep 60 & echo $! > \"$1\"; exit 0", "worker"])
            .arg(&pid_file),
        &[],
        Duration::from_secs(2),
    );
    let pid: i32 = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 0, 0, 0, 0) },
        0
    );
    assert!(result.unwrap_err().to_string().contains("deadline"));
    assert!(libc::WIFSIGNALED(status));
    assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
}

#[test]
fn worker_drains_both_pipes_before_accepting_a_response() {
    let output = exchange(Command::new("/bin/sh").args(["-c", "head -c 1000000 /dev/zero >&2; cat >/dev/null; printf '{\"Ok\":{\"Emitted\":{\"text\":\"ok\",\"source_map\":null}}}'"]), &vec![b' '; 1024 * 1024], DEADLINE).unwrap();
    assert!(matches!(output, Output::Emitted { text, .. } if text == "ok"));
}

#[test]
fn worker_frame_limits_cover_requests_and_both_output_streams() {
    assert!(exchange(
        &mut Command::new("/does/not/exist"),
        &vec![0; MAX + 1],
        DEADLINE
    )
    .unwrap_err()
    .to_string()
    .contains("request exceeds"));
    // stdout and stderr use the same bounded reader.
    assert!(drain(&mut std::io::Cursor::new([1]), &mut vec![0; MAX])
        .unwrap_err()
        .to_string()
        .contains("output exceeds"));
}
