//! The four VM tools through the loop: what they refuse without running, what
//! they defer on, and what the CLI's answer is turned into.

use super::*;

fn vm_ready_state() -> State {
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|session| {
        *session = Some(zellij_tile::prelude::SessionInfo {
            name: "test-session".to_owned().into(),
            is_current_session: true,
            ..Default::default()
        });
    });
    let mut state = ready_state();
    // VM tools must not rely on the daemon's state, including its session name.
    state.zellij_session_name = Default::default();
    state
}

/// Feed a `RunCommandResult` back to a parked VM call, the way the host would.
fn answer(
    state: &mut State,
    name: &str,
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    extra: &[(&str, &str)],
) {
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: name.into(),
        arguments: r#"{"vm":"review"}"#.into(),
    }]
    .into();
    let mut context = BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]);
    for (key, value) in extra {
        context.insert((*key).to_owned(), (*value).to_owned());
    }
    state.on_command_result(exit_code, stdout, stderr, &context);
}

#[test]
fn every_vm_tool_defers_on_the_cli() {
    for (name, arguments) in [
        ("vm_list", "{}"),
        ("vm_changes", r#"{"vm":"review"}"#),
        ("vm_sync", r#"{"vm":"review"}"#),
        ("vm_close", r#"{"vm":"review"}"#),
    ] {
        let mut state = vm_ready_state();
        assert!(!run_file_tool(&mut state, name, arguments), "{name}");
        assert!(matches!(state.status, Status::AwaitingCommand), "{name}");
    }
}

#[test]
fn a_per_vm_tool_without_a_vm_is_answered_without_running_anything() {
    let mut state = vm_ready_state();
    assert!(run_file_tool(&mut state, "vm_sync", "{}"));
    assert_eq!(
        tool_results(&state)[0].content,
        "vm is required; call vm_list for the names"
    );
}

/// A missing host snapshot must not turn into an empty session or pane list.
#[test]
fn a_per_vm_tool_waits_for_the_session_name_but_the_listing_does_not() {
    let mut state = vm_ready_state();
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|session| *session = None);
    assert!(run_file_tool(&mut state, "vm_sync", r#"{"vm":"review"}"#));
    assert_eq!(
        tool_results(&state)[0].content,
        "the current session is not in the session list"
    );

    // The listing is machine-wide and names no session, so it has nothing to wait for.
    let mut state = vm_ready_state();
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|session| *session = None);
    assert!(!run_file_tool(&mut state, "vm_list", "{}"));

    // An explicit session is the way past the wait.
    let mut state = vm_ready_state();
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|session| *session = None);
    assert!(!run_file_tool(
        &mut state,
        "vm_sync",
        r#"{"vm":"review","vm_session":"work"}"#,
    ));
}

#[test]
fn a_listing_is_answered_as_a_table_rather_than_as_json() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_list",
        Some(0),
        br#"{"vms":[{"session":"work","name":"review","state":"running","image":"rust-dev","worktree":"/src/api","branch":"api"},{"session":"work","name":"bare","state":"provisioning"}],"recoveries":[{"path":"/state/recovery/abc","reason":"sync-failed","retained_at":1,"session":"work","name":"lost","worktree":"/src/api"}]}"#,
        b"",
        &[],
    );
    let result = &tool_results(&state)[0].content;
    assert_eq!(
        result,
        "session\tvm\tstate\timage\tworktree\tbranch\n\
         work\treview\trunning\trust-dev\t/src/api\tapi\n\
         work\tbare\tprovisioning\t-\t-\t-\n\
         \n\
         Retained disks (a VM that crashed or could not be merged back):\n\
         lost\tsync-failed\t/state/recovery/abc"
    );
}

#[test]
fn an_empty_listing_says_so() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_list",
        Some(0),
        br#"{"vms":[],"recoveries":[]}"#,
        b"",
        &[],
    );
    assert_eq!(
        tool_results(&state)[0].content,
        "No Firecracker VMs are running."
    );
}

#[test]
fn a_change_count_is_read_as_a_sentence() {
    for (stdout, expected) in [
        (
            &b"0\n"[..],
            "no guest paths have changed since the last sync",
        ),
        (&b"3\n"[..], "3 guest paths changed since the last sync"),
    ] {
        let mut state = vm_ready_state();
        answer(&mut state, "vm_changes", Some(0), stdout, b"", &[]);
        assert_eq!(tool_results(&state)[0].content, expected);
    }
}

/// The regression this file exists for. A merge that kept a host copy exits 0 and
/// writes `warning:` lines to stderr; the generic `exit code: N` envelope would
/// present that success as a failure with an error attached, which is the one
/// reading the model must not take away from a sync.
#[test]
fn a_sync_that_kept_a_host_copy_is_a_success_with_its_warnings_attached() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_sync",
        Some(0),
        b"",
        b"warning: /src/api/a.rs: changed on both sides; kept the host copy, the guest copy is /src/api/a.rs.guest\n",
        &[],
    );
    let result = &tool_results(&state)[0].content;
    assert!(result.starts_with("synced: "), "{result}");
    assert!(!result.contains("exit code"), "{result}");
    // Passed through whole: each one names a file the user has to go and look at.
    assert!(result.contains("/src/api/a.rs.guest"), "{result}");
}

#[test]
fn a_close_reports_lease_release_separately_from_termination_and_merge() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_close",
        Some(0),
        b"Lease released: api in work.\nVM remains leased by another session.\n",
        b"warning: file.guest retained\n",
        &[],
    );
    let result = &tool_results(&state)[0].content;
    assert!(result.contains("Lease released: api in work."));
    assert!(result.contains("VM remains leased by another session."));
    assert!(result.contains("file.guest retained"));
}

#[test]
fn a_failing_cli_is_reported_with_what_it_said() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_sync",
        Some(1),
        b"",
        b"Error: no Firecracker VM named review\n",
        &[],
    );
    assert_eq!(
        tool_results(&state)[0].content,
        "vm_sync failed: Error: no Firecracker VM named review"
    );
}

#[test]
fn a_missing_zellij_binary_is_named_in_the_tool_result() {
    let mut state = vm_ready_state();
    answer(
        &mut state,
        "vm_list",
        Some(2),
        b"",
        b"No such file or directory (os error 2)",
        &[],
    );
    let result = &tool_results(&state)[0].content;
    assert!(result.starts_with("zellij could not be run"), "{result}");
    assert!(result.contains("zellij_path"), "{result}");
}

#[test]
fn closing_an_explicit_session_uses_the_cli_without_a_local_snapshot() {
    let mut state = vm_ready_state();
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|session| *session = None);
    assert!(!run_file_tool(
        &mut state,
        "vm_close",
        r#"{"vm":"review","vm_session":"other"}"#
    ));
    assert!(tool_results(&state).is_empty());
}

#[test]
fn malformed_vm_responses_are_diagnostics() {
    for stdout in [
        "",
        "  ",
        "null",
        "3",
        "{}",
        r#"{"error":"oops"}"#,
        r#"{"vms":null,"recoveries":[]}"#,
        r#"{"vms":[],"recoveries":{}}"#,
        r#"{"vms":[null],"recoveries":[]}"#,
        r#"{"vms":[{}],"recoveries":[]}"#,
        r#"{"vms":[],"recoveries":[{}]}"#,
        "unexpected output",
    ] {
        let mut state = vm_ready_state();
        answer(&mut state, "vm_list", Some(0), stdout.as_bytes(), b"", &[]);
        let result = &tool_results(&state)[0].content;
        assert!(result.starts_with("vm_list returned"), "{result}");
        assert!(!result.contains("No Firecracker VMs"), "{result}");
    }
    for stdout in ["", "oops", "-1", "1.5", "1\n2", "18446744073709551616"] {
        let mut state = vm_ready_state();
        answer(
            &mut state,
            "vm_changes",
            Some(0),
            stdout.as_bytes(),
            b"",
            &[],
        );
        assert!(tool_results(&state)[0]
            .content
            .starts_with("vm_changes returned an invalid path count:"));
    }
}

#[test]
fn failed_closes_never_claim_the_vm_is_gone() {
    for (code, stderr) in [
        (1, "merge failed"),
        (2, "No such file or directory (os error 2)"),
    ] {
        let mut state = vm_ready_state();
        answer(
            &mut state,
            "vm_close",
            Some(code),
            b"",
            stderr.as_bytes(),
            &[],
        );
        let result = &tool_results(&state)[0].content;
        assert!(!result.contains("Lease released"), "{result}");
        assert!(!result.contains("VM is gone"), "{result}");
    }
}
