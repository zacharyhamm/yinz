use super::*;
use std::time::Duration;

async fn assert_reporter_times_out(read_input: bool) {
    let root = tempfile::tempdir().unwrap();
    let reporter = root.path().join("reporter");
    let pid_file = root.path().join("pid");
    let received = root.path().join("received");
    yinz_test_support::write_warm_script(
        &reporter,
        &format!(
            "printf '%s' \"$$\" > \"$YINZ_TEST_REPORT_PID\"\n{}\nexec sleep 60\n",
            if read_input {
                "cat > \"$YINZ_TEST_REPORT_OUTPUT\""
            } else {
                ":"
            }
        ),
    );
    let env = HashMap::from([
        (
            "YINZ_TEST_REPORT_PID".into(),
            pid_file.to_str().unwrap().into(),
        ),
        (
            "YINZ_TEST_REPORT_OUTPUT".into(),
            received.to_str().unwrap().into(),
        ),
    ]);
    // Exceed pipe capacity so a reporter ignoring stdin stalls write_all, not just wait.
    let raw = vec![b'x'; if read_input { 128 } else { 4 * 1024 * 1024 }];
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        crate::forward(&reporter, &env, &raw),
    )
    .await;
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let reaped = nix::sys::wait::waitpid(
        nix::unistd::Pid::from_raw(pid as i32),
        Some(nix::sys::wait::WaitPidFlag::WNOHANG),
    );
    if reaped == Ok(nix::sys::wait::WaitStatus::StillAlive) {
        assert_bridge_child_stopped(pid).await;
    }
    let error = result
        .expect("hook forwarding remained blocked")
        .unwrap_err();
    assert!(
        error.to_string().contains("timed out forwarding"),
        "{error:#}"
    );
    assert_eq!(
        reaped,
        Err(nix::errno::Errno::ECHILD),
        "the timed-out reporter must be reaped"
    );
    if read_input {
        assert_eq!(std::fs::read(received).unwrap(), raw);
    }
}

#[tokio::test]
async fn hook_forward_timeout_covers_a_reporter_ignoring_stdin() {
    assert_reporter_times_out(false).await;
}

#[tokio::test]
async fn hook_forward_timeout_covers_a_reporter_that_never_exits() {
    assert_reporter_times_out(true).await;
}

#[tokio::test]
async fn hook_forward_preserves_input_and_success() {
    let root = tempfile::tempdir().unwrap();
    let reporter = root.path().join("reporter");
    let received = root.path().join("received");
    yinz_test_support::write_warm_script(&reporter, "cat > \"$YINZ_TEST_REPORT_OUTPUT\"");
    let env = HashMap::from([(
        "YINZ_TEST_REPORT_OUTPUT".into(),
        received.to_str().unwrap().into(),
    )]);
    let raw = br#"{"hook_event_name":"StopFailure","session_id":"selected"}"#;
    crate::forward(&reporter, &env, raw).await.unwrap();
    assert_eq!(std::fs::read(received).unwrap(), raw);
}
