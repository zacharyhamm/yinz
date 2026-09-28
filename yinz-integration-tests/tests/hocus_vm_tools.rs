#![cfg(unix)]

//! The VM tools end to end: a real session, a real subprocess, a stub `yinz`.
//!
//! The unit tests cover both halves and both pass with the wiring cut. They build
//! an argv without running it, and they render a command result without having
//! asked for one — so a tool that never reaches the host, or reaches it with a cwd
//! the host rejects, looks exactly like a working one. Only a session that actually
//! shells out tells them apart.
//!
//! The `yinz` it shells out to is a stub rather than the real binary, because the
//! real one would want `/dev/kvm`, a provider and a booted guest. What is under test
//! here is hocus's half: that the tool reaches a process at all, with the argv the
//! unit tests describe, and that what the process says comes back as the answer. The
//! provider protocol itself is tested in `yinz-server`, and the whole stack against
//! a real VM in `cargo x firecracker-test`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use yinz_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// A `yinz` that answers the four `firecracker` subcommands and records the argv
/// it was called with, one call per line.
const STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/calls"
printf '%s\n' "$YINZ_CONFIG_FILE" "$YINZ_CONFIG_DIR" >> "$(dirname "$0")/config-env"
case "$2" in
  list)
    printf '%s' '{"vms":[{"session":"work","name":"review","state":"running","image":"rust-dev","worktree":"/src/api","branch":"vm-branch-proof"},{"session":"other","name":"bare","state":"provisioning"}],"recoveries":[]}'
    ;;
  changes) printf '3\n' ;;
  sync)
    echo 'warning: /src/api/a.rs: changed on both sides; kept the host copy, the guest copy is /src/api/a.rs.guest' >&2
    ;;
  close) echo "Lease released; merge and VM termination are reported separately." ;;
esac
"#;

fn write_stub(path: &Path) {
    let mut file = fs::File::create(path).expect("the stub yinz");
    file.write_all(STUB.as_bytes()).expect("the stub body");
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the stub is executable");
}

/// Put a line into hocus and send it. Slash commands run the tool inline — no API
/// key involved, and no y/n either — which is what makes the result readable from
/// the screen. See `hocus_file_tools`, which this borrows wholesale.
fn submit(yinz: &TestSession, line: &str) {
    yinz.send_stdin(&keys::BRACKETED_PASTE_START);
    yinz.send_stdin(line.as_bytes());
    yinz.send_stdin(&keys::BRACKETED_PASTE_END);
    yinz.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(20)..])
    });
    yinz.send_stdin(&keys::ENTER);
}

fn clear(yinz: &TestSession, gone: &str) {
    yinz.send_stdin(&[0x0c]);
    yinz.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

const HOCUS_LAYOUT: &str = r#"layout {
    pane focus=true size="80%" {
        plugin location="hocus"
    }
    pane split_direction="vertical" {
        pane command="sh" firecracker_vm="review"
        pane command="sh" firecracker_vm="other-vm"
        pane command="sh"
    }
}
"#;

#[test]
fn the_vm_tools_reach_the_yinz_cli_and_answer_with_what_it_said() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path().canonicalize().expect("a real path");
    let stub = root.join("yinz");
    write_stub(&stub);

    // No agent daemon: VM targeting must come directly from the host snapshot.
    let config = format!(
        r#"plugins {{
  hocus location="yinz:hocus" {{
    shared true
    yinz_path "{}"
  }}
  hocus-store location="yinz:hocus-store"
}}
load_plugins clear-defaults=true {{
  "hocus-store"
}}
"#,
        stub.display()
    );

    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&config)
        .with_config_dir(root.clone())
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    yinz.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    let panes: Vec<_> = (0..3).map(|_| yinz.expect_pty_spawn()).collect();
    let review = panes.iter().find(|pane| {
        matches!(pane.terminal_action(), Some(yinz_types::input::command::TerminalAction::RunCommand(command))
            if command.target.firecracker_vm() == Some("review"))
    }).expect("the review VM pane");
    review.output(b"review-pane-live");

    // The listing comes back as a table, not as the JSON the CLI printed, and it
    // carries the worktree and branch the VM is working on.
    submit(&yinz, "/vm_list");
    yinz.wait_until("the listing names the branch", |grid_snapshot| {
        grid_snapshot.contains("vm-branch-proof")
    });
    yinz.wait_until("and the VM it belongs to", |grid_snapshot| {
        grid_snapshot.contains("review")
    });
    clear(&yinz, "vm-branch-proof");

    submit(&yinz, r#"/vm_changes {"vm": "review"}"#);
    yinz.wait_until("the count is a sentence", |grid_snapshot| {
        grid_snapshot.contains("3 guest paths changed")
    });
    clear(&yinz, "3 guest paths changed");

    // The regression this file is worth most for: exit 0 with `warning:` on stderr
    // is a *successful* merge, and the warning names a file to go and look at.
    submit(&yinz, r#"/vm_sync {"vm": "review"}"#);
    yinz.wait_until("the sync succeeded", |grid_snapshot| {
        grid_snapshot.contains("synced")
    });
    yinz.wait_until("and said what it could not decide", |grid_snapshot| {
        grid_snapshot.contains(".guest")
    });
    clear(&yinz, "synced");

    submit(
        &yinz,
        r#"/vm_close {"vm":"review","vm_session":"other-session"}"#,
    );
    yinz.wait_until("another session's lease was released", |grid_snapshot| {
        grid_snapshot.contains("Lease released") && grid_snapshot.contains("review-pane-live")
    });
    assert!(panes.iter().all(|pane| !pane.is_exited()));
    clear(&yinz, "Lease released");

    submit(&yinz, r#"/vm_close {"vm": "  review  "}"#);
    yinz.wait_until(
        "the shared close command completed without a daemon",
        |grid_snapshot| {
            grid_snapshot.contains("Lease released") && grid_snapshot.contains("review-pane-live")
        },
    );
    for pane in &panes {
        if pane.terminal_id() != review.terminal_id() {
            assert!(!pane.is_exited(), "an unrelated pane was closed");
        }
    }
    clear(&yinz, "Lease released");

    // A VM name Yinz would refuse never reaches a process.
    submit(&yinz, r#"/vm_changes {"vm": "a/b"}"#);
    yinz.wait_until("a bad name is refused here", |grid_snapshot| {
        grid_snapshot.contains("must") || grid_snapshot.contains("invalid")
    });

    yinz.quit();

    // Each tool reached the CLI with the argv the unit tests describe. Read after
    // quitting so every call has certainly been written.
    let calls = fs::read_to_string(root.join("calls")).expect("the stub was called");
    let calls: Vec<&str> = calls.lines().collect();
    // The refused VM name is absent because it never got as far as a process.
    assert_eq!(calls.len(), 5, "the stub was called with {calls:?}");
    assert_eq!(calls[0], "firecracker list --json");
    for (call, verb) in [calls[1], calls[2], calls[4]]
        .iter()
        .zip(["changes", "sync", "close"])
    {
        let expected = format!("firecracker {verb} review --session ");
        let session = call
            .strip_prefix(&expected)
            .unwrap_or_else(|| panic!("{call:?} is not {expected:?} plus a session"));
        assert_eq!(session, yinz.session_name());
    }
    assert_eq!(calls[3], "firecracker close review --session other-session");

    let config_env =
        fs::read_to_string(root.join("config-env")).expect("the CLI config environment");
    let config_env: Vec<&str> = config_env.lines().collect();
    assert_eq!(config_env.len(), calls.len() * 2);
    for paths in config_env.chunks_exact(2) {
        let config_path = Path::new(paths[0]);
        assert!(config_path.is_absolute(), "{config_path:?}");
        assert!(fs::read_to_string(config_path).unwrap().contains(&config));
        assert_eq!(Path::new(paths[1]), root);
    }
}
