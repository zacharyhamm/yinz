use super::*;
use nix::errno::Errno;
use nix::sys::wait::{waitpid, WaitPidFlag};
use nix::unistd::Pid;
use std::sync::mpsc;
use yinz_test_support::{default_timeout, write_warm_script};

#[test]
fn copy_command_uses_renamed_session() {
    assert_renamed_session(false);
}

#[test]
fn copy_command_uses_session_renamed_while_queued() {
    assert_renamed_session(true);
}

fn assert_renamed_session(queued: bool) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("helper");
    let received = dir.path().join("session");
    write_warm_script(
        &script,
        &format!(
            "cat > /dev/null\nprintf '%s' \"$YINZ_SESSION_NAME\" > '{}'",
            received.display()
        ),
    );
    let session = SessionState::new("old-session".into());
    let command = CopyCommand::new(script.display().to_string(), session.clone());
    let (release, previous) = mpsc::channel();
    if queued {
        // Hold the preceding transfer until the queued command's session is renamed.
        *LAST_TRANSFER.lock().unwrap() = Some(Arc::new(Mutex::new(previous)));
    } else {
        session.set("renamed-session".into());
    }
    let (tx, rx) = mpsc::channel();
    let worker = command
        .set("copied text".into(), move |result| tx.send(result).unwrap())
        .unwrap();
    session.set("renamed-session".into());
    drop(release);
    rx.recv_timeout(default_timeout()).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(received).unwrap(),
        "renamed-session"
    );
}

#[test]
fn copy_command_preserves_transfer_order() {
    assert_transfer_order(false);
}

#[test]
fn copy_command_preserves_transfer_order_across_instances() {
    assert_transfer_order(true);
}

fn open_gate(path: &std::path::Path) -> std::fs::File {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

fn assert_transfer_order(separate_instances: bool) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("helper");
    let clipboard = dir.path().join("clipboard");
    let ready_path = dir.path().join("ready");
    let release_path = dir.path().join("release");
    let committed_path = dir.path().join("committed");
    let owner_path = dir.path().join("owner");
    let mut ready = open_gate(&ready_path);
    let mut release = open_gate(&release_path);
    let mut committed = open_gate(&committed_path);
    let mut owner = open_gate(&owner_path);
    write_warm_script(
        &script,
        &format!(
            "IFS= read -r copy\n\
             if [ \"$copy\" = A ]; then\n\
               echo ready > '{}'\n\
               read release < '{}'\n\
               cat > /dev/null\n\
               printf A > '{}'\n\
               echo committed > '{}'\n\
               read release < '{}'\n\
             else\n\
               cat > /dev/null\n\
               read committed < '{}'\n\
               printf B > '{}'\n\
             fi",
            ready_path.display(),
            release_path.display(),
            clipboard.display(),
            committed_path.display(),
            owner_path.display(),
            committed_path.display(),
            clipboard.display(),
        ),
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = ready.try_clone().unwrap();
    let ready_thread = std::thread::spawn(move || {
        let mut line = String::new();
        std::io::BufReader::new(reader)
            .read_line(&mut line)
            .unwrap();
        let _ = ready_tx.send(());
    });
    let command = CopyCommand::new(script.display().to_string(), SessionState::default());
    let other_command = CopyCommand::new(script.display().to_string(), SessionState::default());
    let (tx, rx) = mpsc::channel();
    let first_tx = tx.clone();
    let first = command
        .set(
            format!("A\n{}", "a".repeat(4 * 1024 * 1024)),
            move |result| {
                first_tx.send(('A', result)).unwrap();
            },
        )
        .unwrap();
    let mut second = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ready_rx.recv_timeout(default_timeout()).unwrap();
        second = Some(
            (if separate_instances {
                &other_command
            } else {
                &command
            })
            .set("B\n".into(), move |result| tx.send(('B', result)).unwrap())
            .unwrap(),
        );
        // Timing is the behavior under test: neither transfer may finish before release.
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_millis(250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        writeln!(release).unwrap();
        for expected in ['A', 'B'] {
            let (copy, result) = rx.recv_timeout(default_timeout()).unwrap();
            assert_eq!(copy, expected);
            result.unwrap();
        }
        second.take().unwrap().join().unwrap();
        assert_eq!(std::fs::read_to_string(&clipboard).unwrap(), "B");
        assert!(
            !first.is_finished(),
            "the earlier clipboard owner must survive"
        );
    }));
    // Unblock all helper and reader waits even when an ordering assertion fails.
    writeln!(release).unwrap();
    writeln!(owner).unwrap();
    writeln!(committed).unwrap();
    writeln!(ready).unwrap();
    ready_thread.join().unwrap();
    first.join().unwrap();
    if let Some(second) = second {
        second.join().unwrap();
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn copy_command_completes_before_its_owner_exits_and_reaps_it() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("helper");
    let received = dir.path().join("received");
    let pid_file = dir.path().join("pid");
    let gate = dir.path().join("gate");
    let survived = dir.path().join("survived");
    assert!(Command::new("mkfifo")
        .arg(&gate)
        .status()
        .unwrap()
        .success());
    let mut release = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&gate)
        .unwrap();
    write_warm_script(
        &script,
        &format!(
            "echo $$ > '{}'\ncat > '{}'\nread release < '{}'\ntouch '{}'",
            pid_file.display(),
            received.display(),
            gate.display(),
            survived.display()
        ),
    );
    let (tx, rx) = mpsc::channel();
    let worker = CopyCommand::new(script.display().to_string(), SessionState::default())
        .set("some copied text".into(), move |result| {
            tx.send(result).unwrap()
        })
        .unwrap();
    let result = rx.recv_timeout(default_timeout());
    // Release even if completion timed out, so a regression cannot leave a blocked child.
    writeln!(release).unwrap();
    worker.join().unwrap();
    result.unwrap().unwrap();
    assert!(rx.try_recv().is_err());
    assert_eq!(
        std::fs::read_to_string(received).unwrap(),
        "some copied text"
    );
    assert!(survived.exists());
    assert_reaped(&pid_file);
}

fn assert_reaped(pid_file: &std::path::Path) {
    let pid = Pid::from_raw(
        std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    assert_eq!(
        waitpid(pid, Some(WaitPidFlag::WNOHANG)).err(),
        Some(Errno::ECHILD)
    );
}

#[test]
fn copy_command_is_reaped_after_a_failed_stdin_write() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("close-stdin");
    let pid_file = dir.path().join("pid");
    write_warm_script(
        &script,
        &format!("echo $$ > '{}'\nexec 0<&-\nexit 0", pid_file.display()),
    );
    let (tx, rx) = mpsc::channel();
    let worker = CopyCommand::new(script.display().to_string(), SessionState::default())
        // Larger than a pipe buffer: the helper closes its read end before this can finish.
        .set("x".repeat(4 * 1024 * 1024), move |result| {
            tx.send(result).unwrap()
        })
        .unwrap();
    let error = rx.recv_timeout(default_timeout()).unwrap().unwrap_err();
    assert_next_copy_completes();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    worker.join().unwrap();
    assert!(rx.try_recv().is_err());
    assert_reaped(&pid_file);
}

#[test]
fn copy_command_reports_spawn_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    CopyCommand::new(
        dir.path().join("missing").display().to_string(),
        SessionState::default(),
    )
    .set(String::new(), move |result| tx.send(result).unwrap())
    .unwrap()
    .join()
    .unwrap();
    let error = rx.recv_timeout(default_timeout()).unwrap().unwrap_err();
    assert_next_copy_completes();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(rx.try_recv().is_err());
}

fn assert_next_copy_completes() {
    let (tx, rx) = mpsc::channel();
    let worker = CopyCommand::new("/usr/bin/true".into(), SessionState::default())
        .set(String::new(), move |result| tx.send(result).unwrap())
        .unwrap();
    rx.recv_timeout(default_timeout()).unwrap().unwrap();
    worker.join().unwrap();
}
