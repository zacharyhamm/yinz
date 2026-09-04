#![cfg(unix)]

//! The VM tools end to end: a real session, a real subprocess, a stub `zellij`.
//!
//! The unit tests cover both halves and both pass with the wiring cut. They build
//! an argv without running it, and they render a command result without having
//! asked for one — so a tool that never reaches the host, or reaches it with a cwd
//! the host rejects, looks exactly like a working one. Only a session that actually
//! shells out tells them apart.
//!
//! The `zellij` it shells out to is a stub rather than the real binary, because the
//! real one would want `/dev/kvm`, a provider and a booted guest. What is under test
//! here is hocus's half: that the tool reaches a process at all, with the argv the
//! unit tests describe, and that what the process says comes back as the answer. The
//! provider protocol itself is tested in `zellij-server`, and the whole stack against
//! a real VM in `cargo x firecracker-test`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// A `zellij` that answers the four `firecracker` subcommands and records the argv
/// it was called with, one call per line.
const STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/calls"
case "$2" in
  list)
    printf '%s' '{"vms":[{"session":"work","name":"review","state":"running","image":"rust-dev","worktree":"/src/api","branch":"api"},{"session":"other","name":"bare","state":"provisioning"}],"recoveries":[]}'
    ;;
  changes) printf '3\n' ;;
  sync)
    echo 'warning: /src/api/a.rs: changed on both sides; kept the host copy, the guest copy is /src/api/a.rs.guest' >&2
    ;;
  stop) ;;
esac
"#;

fn write_stub(path: &Path) {
    let mut file = fs::File::create(path).expect("the stub zellij");
    file.write_all(STUB.as_bytes()).expect("the stub body");
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the stub is executable");
}

/// Put a line into hocus and send it. Slash commands run the tool inline — no API
/// key involved, and no y/n either — which is what makes the result readable from
/// the screen. See `hocus_file_tools`, which this borrows wholesale.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(20)..])
    });
    zellij.send_stdin(&keys::ENTER);
}

fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]);
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

const HOCUS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="hocus"
    }
}
"#;

#[test]
fn the_vm_tools_reach_the_zellij_cli_and_answer_with_what_it_said() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path().canonicalize().expect("a real path");
    let stub = root.join("zellij");
    write_stub(&stub);

    // `hocus-agentd` is loaded because it is what names the session: every per-VM
    // tool sends `--session`, and hocus takes that name from the daemon's poll reply
    // the way the rest of its per-pane state comes from there. That is the production
    // path, so it is the one under test.
    let config = format!(
        r#"plugins {{
  hocus location="zellij:hocus" {{
    shared true
    zellij_path "{}"
  }}
  hocus-agentd location="zellij:hocus-agentd"
  hocus-store location="zellij:hocus-store"
}}
load_plugins clear-defaults=true {{
  "hocus-agentd"
  "hocus-store"
}}
"#,
        stub.display()
    );

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The listing comes back as a table, not as the JSON the CLI printed, and it
    // carries the worktree and branch the VM is working on.
    submit(&zellij, "/vm_list");
    zellij.wait_until("the listing names the branch", |grid_snapshot| {
        grid_snapshot.contains("validate") || grid_snapshot.contains("api")
    });
    zellij.wait_until("and the VM it belongs to", |grid_snapshot| {
        grid_snapshot.contains("review")
    });
    clear(&zellij, "review");

    submit(&zellij, r#"/vm_changes {"vm": "review"}"#);
    zellij.wait_until("the count is a sentence", |grid_snapshot| {
        grid_snapshot.contains("3 guest paths changed")
    });
    clear(&zellij, "3 guest paths changed");

    // The regression this file is worth most for: exit 0 with `warning:` on stderr
    // is a *successful* merge, and the warning names a file to go and look at.
    submit(&zellij, r#"/vm_sync {"vm": "review"}"#);
    zellij.wait_until("the sync succeeded", |grid_snapshot| {
        grid_snapshot.contains("synced")
    });
    zellij.wait_until("and said what it could not decide", |grid_snapshot| {
        grid_snapshot.contains(".guest")
    });
    clear(&zellij, "synced");

    // A VM name Zellij would refuse never reaches a process.
    submit(&zellij, r#"/vm_changes {"vm": "a/b"}"#);
    zellij.wait_until("a bad name is refused here", |grid_snapshot| {
        grid_snapshot.contains("must") || grid_snapshot.contains("invalid")
    });

    zellij.quit();

    // Each tool reached the CLI with the argv the unit tests describe. Read after
    // quitting so every call has certainly been written.
    let calls = fs::read_to_string(root.join("calls")).expect("the stub was called");
    let calls: Vec<&str> = calls.lines().collect();
    // The session name is generated per test run, so it is checked for being there
    // and non-empty rather than compared. Everything else is exact - the refused VM
    // name is absent because it never got as far as a process.
    assert_eq!(calls.len(), 3, "the stub was called with {calls:?}");
    assert_eq!(calls[0], "firecracker list --json");
    for (call, verb) in calls[1..].iter().zip(["changes", "sync"]) {
        let expected = format!("firecracker {verb} review --session ");
        let session = call
            .strip_prefix(&expected)
            .unwrap_or_else(|| panic!("{call:?} is not {expected:?} plus a session"));
        assert!(!session.is_empty(), "{call:?} names no session");
    }
}
