//! Tests for the pure formatting half of `zellij pty-host list`, and for the
//! directory scan underneath it.
//!
//! `list`, `kill` and `kill_all` are deliberately **not** tested here: they all
//! enumerate the real `$ZELLIJ_SOCK_DIR/pty`, which is shared with whatever
//! zellij sessions the developer running the suite happens to have open, and
//! `kill_all()` would terminate their live detached panes. `ZELLIJ_SOCK_DIR` is
//! a `lazy_static` resolved once per process, so there is no way to point them
//! somewhere safe from inside a test. `collect_from` takes its directory as an
//! argument precisely so the scan itself can be covered without that risk.

use super::*;
use zellij_types::internal_ids::ProcessId;
use zellij_types::internal_ids::SupervisorId;

use crate::pty_host::remote::protocol::RemotePaneEntry;
use hegel::generators;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `humanize_age` reads the clock itself, so between building a `started_at` and
/// the function sampling `SystemTime::now()` the true age can advance. Two
/// seconds is far more than the gap and small enough that the assertions still
/// bite.
const CLOCK_SLACK_SECS: u64 = 2;

/// Splits `"1023K"` into `(1023, 1024)` — the rendered magnitude and the size of
/// the unit it was rendered in.
fn parse_bytes(rendered: &str) -> (u128, u128) {
    let (digits, suffix) = rendered.split_at(rendered.len() - 1);
    let unit: u128 = match suffix {
        "B" => 1,
        "K" => 1024,
        "M" => 1024 * 1024,
        other => panic!("human_bytes produced an unknown unit {other:?} in {rendered:?}"),
    };
    let value = digits
        .parse()
        .unwrap_or_else(|e| panic!("human_bytes produced an unparseable {rendered:?}: {e}"));
    (value, unit)
}

/// Splits `"3h"` into `(3, 3600)`.
fn parse_age(rendered: &str) -> (u64, u64) {
    let (digits, suffix) = rendered.split_at(rendered.len() - 1);
    let unit: u64 = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        other => panic!("humanize_age produced an unknown unit {other:?} in {rendered:?}"),
    };
    let value = digits
        .parse()
        .unwrap_or_else(|e| panic!("humanize_age produced an unparseable {rendered:?}: {e}"));
    (value, unit)
}

/// A real round trip rather than a restatement of the `if` chain: whatever unit
/// was chosen, multiplying the rendered magnitude back out has to bracket the
/// true byte count. Drawn unbounded — `usize::MAX` is exactly where an
/// intermediate multiplication would overflow, so the arithmetic here is done in
/// `u128`.
#[hegel::test]
fn human_bytes_never_loses_more_than_its_unit(tc: hegel::TestCase) {
    let bytes = tc.draw(generators::integers::<usize>());
    let rendered = human_bytes(bytes);
    let (value, unit) = parse_bytes(&rendered);
    let bytes = bytes as u128;

    assert!(
        value * unit <= bytes,
        "human_bytes({bytes}) rendered {rendered:?}, which overstates the size"
    );
    assert!(
        bytes < (value + 1) * unit,
        "human_bytes({bytes}) rendered {rendered:?}, which understates it by a whole unit"
    );
}

#[test]
fn human_bytes_switches_units_at_each_power_of_two_boundary() {
    assert_eq!(human_bytes(0), "0B");
    assert_eq!(human_bytes(1023), "1023B");
    assert_eq!(human_bytes(1024), "1K");
    assert_eq!(human_bytes(1024 * 1024 - 1), "1023K");
    assert_eq!(human_bytes(1024 * 1024), "1M");
}

/// The same round-trip shape as `human_bytes`, with one small tolerance for the
/// clock the function reads on its own.
#[hegel::test]
fn humanize_age_reports_an_age_the_rendered_unit_can_account_for(tc: hegel::TestCase) {
    // A little over four days, so the day arm is reachable as well as the other
    // three. There is no upper bound in the code — this is purely to keep every
    // branch reachable in a hundred cases.
    let age = tc.draw(generators::integers::<u64>().max_value(400_000));
    let rendered = humanize_age(now_unix().saturating_sub(age));
    let (value, unit) = parse_age(&rendered);

    assert!(
        value * unit <= age + CLOCK_SLACK_SECS,
        "an age of {age}s rendered as {rendered:?}, which is further in the past than it happened"
    );
    assert!(
        age < (value + 1) * unit,
        "an age of {age}s rendered as {rendered:?}, which is a whole unit short"
    );
}

#[test]
fn humanize_age_uses_the_largest_unit_that_fits() {
    // Every value sits at the bottom of its band, so the couple of seconds the
    // clock may advance mid-call cannot push any of them into the next one.
    let now = now_unix();
    for (age, expected) in [
        (0, "0s"),
        (30, "30s"),
        (60, "1m"),
        (90, "1m"),
        (3600, "1h"),
        (7200, "2h"),
        (86400, "1d"),
        (200_000, "2d"),
    ] {
        assert_eq!(
            humanize_age(now - age),
            expected,
            "an age of {age}s should render as {expected:?}"
        );
    }
}

/// A supervisor restored from a suspended machine, or one that outlived an NTP
/// step, can legitimately report a start time in the future. Without the
/// `saturating_sub` that would underflow.
#[test]
fn an_age_from_the_future_is_reported_as_zero_rather_than_underflowing() {
    assert_eq!(humanize_age(now_unix() + 3600), "0s");
    assert_eq!(humanize_age(u64::MAX), "0s");
}

/// `buffered_bytes` is drawn from a mix that includes a plain zero so the
/// "nothing buffered" branch comes up about half the time rather than almost
/// never; `child_exit_status` is a nested `Option` on purpose, because the three
/// shapes it can take are the whole subject of these tests.
#[hegel::composite]
fn host_statuses(tc: &hegel::TestCase) -> HostStatus {
    HostStatus {
        host_id: SupervisorId::new(tc.draw(generators::text())),
        session_name: tc.draw(generators::optional(generators::text())),
        terminal_id: tc.draw(generators::integers::<u32>()),
        command: tc.draw(generators::text()),
        cwd: tc
            .draw(generators::optional(generators::text()))
            .map(PathBuf::from),
        child_pid: tc
            .draw(generators::optional(generators::integers::<u32>()))
            .map(ProcessId::new),
        host_pid: ProcessId::new(tc.draw(generators::integers::<u32>())),
        // Recent enough that the age is a small number of seconds, which keeps
        // these tests about the classification rather than about the clock.
        started_at_unix: now_unix(),
        attached: tc.draw(generators::booleans()),
        child_exit_status: tc.draw(generators::optional(generators::optional(
            generators::integers::<i32>(),
        ))),
        buffered_bytes: if tc.draw(generators::booleans()) {
            0
        } else {
            tc.draw(generators::integers::<usize>())
        },
    }
}

/// An exit status wins over attachment, deliberately: a pane whose child is gone
/// is finished whether or not a server happens to be looking at it. The three
/// states are also mutually exclusive — a listing that called something both
/// attached and exited would be worse than useless.
#[hegel::test]
fn a_pane_that_has_exited_is_described_as_exited_whether_or_not_a_server_is_attached(
    tc: hegel::TestCase,
) {
    let status = tc.draw(host_statuses());
    let described = describe_state(&status);

    let expected_prefix = match (status.attached, status.child_exit_status) {
        (_, Some(_)) => "exited",
        (true, None) => "attached",
        (false, None) => "orphaned",
    };
    assert!(
        described.starts_with(expected_prefix),
        "a status that is attached={} and exited={:?} was described as {described:?}, \
         which does not start with {expected_prefix:?}",
        status.attached,
        status.child_exit_status
    );

    let claimed: Vec<&str> = ["exited", "attached", "orphaned"]
        .into_iter()
        .filter(|state| described.starts_with(state))
        .collect();
    assert_eq!(
        claimed.len(),
        1,
        "{described:?} claims {claimed:?} rather than exactly one state"
    );
}

/// The exit code is the one piece of a `HostStatus` a user actually acts on, and
/// it is only knowable for a child that exited normally — a signalled child
/// arrives as `Some(None)` and must not be rendered as some invented code.
#[hegel::test]
fn an_exit_code_is_shown_only_when_there_is_one(tc: hegel::TestCase) {
    let status = tc.draw(host_statuses());
    let described = describe_state(&status);

    match status.child_exit_status {
        Some(Some(code)) => assert!(
            described.starts_with(&format!("exited({code})")),
            "a child that exited with {code} was described as {described:?}"
        ),
        Some(None) => assert!(
            described.starts_with("exited "),
            "a child that exited without a code was described as {described:?}"
        ),
        None => assert!(
            !described.starts_with("exited"),
            "a running child was described as {described:?}"
        ),
    }
}

/// Buffered output is only meaningful for a pane nobody is reading: while a
/// server is attached it is the reader, so the ring is empty by construction and
/// mentioning it would be noise.
#[hegel::test]
fn only_an_orphan_with_buffered_output_mentions_buffering(tc: hegel::TestCase) {
    let status = tc.draw(host_statuses());
    let described = describe_state(&status);

    let should_mention =
        !status.attached && status.child_exit_status.is_none() && status.buffered_bytes > 0;
    assert_eq!(
        described.contains("buffered"),
        should_mention,
        "attached={}, exited={:?}, buffered={} produced {described:?}",
        status.attached,
        status.child_exit_status,
        status.buffered_bytes
    );
}

// ---------------------------------------------------------------------------
// The directory scan
// ---------------------------------------------------------------------------

#[test]
fn a_directory_that_does_not_exist_is_not_an_error() {
    // The normal state of a machine that has never run a detachable pane. There
    // being nothing to list is not a failure to list.
    let dir = tempfile::tempdir().expect("TEST");
    let missing = dir.path().join("never-created");

    let entries = collect_from(&missing).expect("a missing socket dir means no supervisors");
    assert!(entries.is_empty());
}

#[test]
fn an_empty_directory_yields_no_supervisors() {
    let dir = tempfile::tempdir().expect("TEST");

    let entries = collect_from(dir.path()).expect("TEST");
    assert!(entries.is_empty());
}

#[test]
fn a_socket_nobody_is_listening_on_is_garbage_collected() {
    // The residue of a SIGKILL'd supervisor. Reporting it would be worse than
    // useless: it names a pane that no longer exists.
    let dir = tempfile::tempdir().expect("TEST");
    let stale = dir.path().join("deadbeef.sock");
    // A real bound socket with nobody listening, not a plain file standing in for
    // one: `UnixListener` leaves the node behind on drop, and only an actual
    // socket refuses the connect. Connecting to a regular file is ECONNREFUSED on
    // Linux but ENOTSOCK on macOS, which the scan would rightly not collect.
    drop(std::os::unix::net::UnixListener::bind(&stale).expect("TEST"));

    let entries = collect_from(dir.path()).expect("TEST");

    assert!(entries.is_empty());
    assert!(
        !stale.exists(),
        "a socket whose owner is gone should have been unlinked"
    );
}

#[test]
fn files_that_are_not_sockets_are_left_alone() {
    // Supervisors keep a sibling `<host_id>.json` beside each socket, so the
    // scan has to be able to walk past a file it did not put there.
    let dir = tempfile::tempdir().expect("TEST");
    let metadata = dir.path().join("deadbeef.json");
    std::fs::write(&metadata, b"{}").expect("TEST");

    let entries = collect_from(dir.path()).expect("TEST");

    assert!(entries.is_empty());
    assert!(metadata.exists(), "a non-socket file must not be unlinked");
}

// --- the remote half of `pty-host list` and `pty-host adopt` ---
//
// Pure by construction: everything here is a judgement about one entry a broker
// reported, and the broker is the part that cannot be reached without a machine
// on the far end of an ssh.

fn remote_entry(command: &str, attached: bool) -> RemotePaneEntry {
    RemotePaneEntry {
        pane_token: "d6a7b8c9e0f1".to_owned(),
        command: command.to_owned(),
        attached,
        unattached_secs: if attached { None } else { Some(90) },
    }
}

#[test]
fn a_duration_reads_in_the_largest_unit_that_fits() {
    // The boundaries, which are where a column this narrow goes wrong: 59
    // seconds is not a minute, and 59 minutes is not an hour.
    assert_eq!(human_duration(0), "0s");
    assert_eq!(human_duration(59), "59s");
    assert_eq!(human_duration(60), "1m");
    assert_eq!(human_duration(3599), "59m");
    assert_eq!(human_duration(3600), "1h");
    assert_eq!(human_duration(86_400), "24h");
}

#[test]
fn a_pane_someone_is_already_watching_is_not_offered_for_recovery() {
    // `adopt` is for panes a failed handoff stranded. Taking one that still has
    // a session speaking for it would be moving a live pane out from under it,
    // which is a warp — and warps do not go through this command.
    let refused = warped_pane_from_entry("build-box", remote_entry("/bin/sh", true));
    let message = format!(
        "{:#}",
        refused.expect_err("an attached pane was offered up")
    );
    assert!(
        message.contains("already has a session attached"),
        "refused, but for the wrong reason: {message}",
    );
}

#[test]
fn a_stranded_pane_is_addressed_by_its_broker_and_nothing_local() {
    let pane = warped_pane_from_entry("build-box", remote_entry("/usr/bin/cargo watch", false))
        .expect("an unattached pane is exactly what this command recovers");

    assert_eq!(
        pane.host_id,
        SupervisorId::new(""),
        "there is no local supervisor to name"
    );
    assert_eq!(pane.control_socket, PathBuf::new());
    let remote = pane.remote.expect("the pane's only address is its broker");
    assert_eq!(remote.host, "build-box");
    assert_eq!(remote.pane_token, "d6a7b8c9e0f1");
    assert_eq!(
        remote.received, 0,
        "this side has taken delivery of nothing, so the replay starts from \
         whatever the ring still holds",
    );
    assert_eq!(
        remote.cmd.target.ssh_host(),
        Some("build-box"),
        "the host is the part that has to be right: it is what a later re-run reads",
    );
    assert_eq!(remote.cmd.command, PathBuf::from("/usr/bin/cargo"));
    assert_eq!(remote.cmd.args, vec!["watch".to_owned()]);
    assert_eq!(
        pane.contents, None,
        "a survivor brings no screen: the server that held its scrollback is what died",
    );
}

#[test]
fn a_recovered_panes_command_is_reconstructed_by_splitting_on_spaces() {
    // Pinned as a known limitation rather than as a good outcome. The session
    // that knew the real `RunCommand` is the one that is gone, and the broker
    // reports a command already flattened to a string, so a quoted argument
    // cannot be put back together. The host survives, which is the part a later
    // re-run depends on.
    let pane = warped_pane_from_entry("build-box", remote_entry("sh -c 'echo one two'", false))
        .expect("TEST");
    let cmd = pane.remote.expect("TEST").cmd;
    assert_eq!(
        cmd.args,
        vec!["-c", "'echo", "one", "two'"],
        "a quoted argument comes back split, which is what flattening it lost",
    );
    assert_eq!(cmd.target.ssh_host(), Some("build-box"));
}
