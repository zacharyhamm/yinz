use super::*;

#[test]
fn worker_generation_survives_installed_executable_replacement_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("yinz");
    yinz_test_support::write_warm_script(&binary, "#!/bin/sh\nprintf old-worker\n");
    let pinned = pin_executable(&binary, None).unwrap();
    let retained = pinned.path.clone();
    let replacement = root.path().join("replacement");
    yinz_test_support::write_warm_script(&replacement, "#!/bin/sh\nprintf new-worker\n");
    std::fs::rename(&replacement, &binary).unwrap();
    assert_eq!(
        Command::new(&retained).output().unwrap().stdout,
        b"old-worker"
    );
    assert_eq!(
        Command::new(&binary).output().unwrap().stdout,
        b"new-worker"
    );
    drop(pinned);
    assert!(!retained.exists());
}

#[test]
fn worker_generation_borrows_only_the_runners_matching_snapshot() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let generation = root.path().join("generation");
    yinz_test_support::write_warm_script(&generation, "#!/bin/sh\nprintf shared-worker\n");
    // Multiple test processes (or sequential sessions) must keep using the same
    // inode without allocating another executable or taking ownership of cleanup.
    for _ in 0..16 {
        let pinned = pin_executable(&generation, Some(&generation)).unwrap();
        assert!(pinned._directory.is_none());
        assert_eq!(
            std::fs::metadata(&pinned.path).unwrap().ino(),
            std::fs::metadata(&generation).unwrap().ino(),
        );
        assert_eq!(
            Command::new(&pinned.path).output().unwrap().stdout,
            b"shared-worker"
        );
        drop(pinned);
        assert!(generation.exists());
    }
    // A child that overrides its worker must still retain its own generation,
    // even if it inherited the runner's snapshot metadata.
    let other = root.path().join("other");
    yinz_test_support::write_warm_script(&other, "#!/bin/sh\nprintf other-worker\n");
    let pinned = pin_executable(&other, Some(&generation)).unwrap();
    let retained = pinned.path.clone();
    assert!(pinned._directory.is_some());
    std::fs::remove_file(other).unwrap();
    assert_eq!(
        Command::new(&retained).output().unwrap().stdout,
        b"other-worker"
    );
    drop(pinned);
    assert!(!retained.exists());
}

#[test]
fn worker_transport_rejects_abnormal_exit_and_invalid_output() {
    for script in [
        "printf garbage",
        "printf '{\"Ok\":{\"Analysis\":{\"diagnostics\":[],\"outline\":[],\"permissions\":[],\"warnings\":[]}}}'; exit 7",
        "kill -KILL $$",
    ] {
        assert!(exchange(Command::new("/bin/sh").args(["-c", script]), b"", DEADLINE).is_err());
    }
    assert!(exchange(&mut Command::new("/does/not/exist"), b"", DEADLINE).is_err());
}

#[test]
fn worker_that_exits_without_reading_reports_its_status_and_stderr() {
    // A binary that rejects the worker argument never reads its request. Whether it exits
    // before the first write, mid-write or after it, the error must be the worker's own.
    for (size, fits_pipe) in [(16, true), (256 * 1024, false)] {
        let error = exchange(
            Command::new("/bin/sh")
                .args([
                    "-c",
                    "echo \"unexpected argument $1\" >&2; exit 3",
                    "worker",
                ])
                .arg(ARG),
            &vec![b' '; size],
            DEADLINE,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(&format!("unexpected argument {ARG}")),
            "{message}"
        );
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(!message.contains("Broken pipe"), "{message}");
        // No pipe buffer holds this request, so the worker cannot have taken all of it.
        if !fits_pipe {
            assert!(message.contains("before reading its request"), "{message}");
        }
    }
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
    let output = exchange(Command::new("/bin/sh").args(["-c", "head -c 1000000 /dev/zero >&2; cat >/dev/null; printf '{\"Ok\":{\"Analysis\":{\"diagnostics\":[],\"outline\":[],\"permissions\":[],\"warnings\":[]}}}'"]), &vec![b' '; 1024 * 1024], DEADLINE).unwrap();
    assert!(matches!(output, Output::Analysis(report) if report == PluginCheckReport::default()));
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
