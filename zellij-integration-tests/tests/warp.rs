#![cfg(unix)]

//! Moving a running pane from one session to another.
//!
//! Everything here needs two sessions, which is what nothing below the whole-app
//! layer can have: `Screen`'s own tests can ask whether a warp would be allowed
//! and what it would serialize, but the pane leaving one session and arriving in
//! another goes out over a socket to a second server, and that only happens here.
//!
//! Both sessions run in this process, with a real server thread each and a real
//! IPC socket each in the shared per-process socket dir — which is the same dir
//! `warp.rs::connect_to_session` resolves a session name against, so one session
//! reaches the other exactly as `zellij action` would.
//!
//! The supervisor handover is faked (see `FakeServerOsApi::release_terminal_for_warp`):
//! the pane's pty state is moved between the two sessions' registries rather than
//! a real process being re-parented. What that still proves is the part with no
//! owner while it is in flight — that the pane leaves one session, arrives in the
//! other keeping everything it had, and is never told to exit.

use zellij_cli::args::CliAction;
use zellij_integration_tests::{
    keys, FakePtyHandle, LayoutInfo, TestRunner, TestSession, WarpFailure, PROMPT, TERMINAL_SIZE,
};

/// Keep each session to its shell pane: the default background plugins would
/// otherwise be spawning panes of their own while the test watches for one.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

fn start(layout: Option<&str>) -> TestSession {
    let runner = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        // Two sessions to a test should not cost two slots: the pair is one
        // test's worth of work, and the runner's concurrency budget is sized in
        // tests rather than in servers.
        .skip_concurrency_slot();
    match layout {
        Some(layout) => runner
            .with_layout(LayoutInfo::Stringified(layout.to_owned()))
            .start(),
        None => runner.start(),
    }
}

/// A session with one shell pane sitting at a prompt.
fn start_with_a_shell() -> (TestSession, FakePtyHandle) {
    let zellij = start(None);
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_for_app_load();
    (zellij, shell)
}

/// Move the focus onto a pane and confirm it landed, by what the pane is sent.
///
/// Focus is not something a grid snapshot states, and for these tests it is the
/// whole subject — "the focused pane" is what a warp with no `--pane-id` moves.
/// So it is established the only way it can be observed: type something and see
/// which pane's process receives it.
/// The key is repeated rather than sent once: how many panes away the target is
/// depends on what the test split off first, and moving left from the leftmost
/// pane of a one-tab session goes nowhere.
fn focus_and_confirm(zellij: &TestSession, focus_key: &[u8], expected: &FakePtyHandle) {
    let probe = b'z';
    for _ in 0..50 {
        zellij.send_stdin(focus_key);
        zellij.send_stdin(&[probe]);
        if expected.stdin_bytes().contains(&probe) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("the pane never took the keyboard focus");
}

/// The pair a warp needs, **receiving session started first**.
///
/// A warp refuses to move a pane into the session it is already in, and the
/// server answers "which session am I" out of `ZELLIJ_SESSION_NAME` — which is
/// per *process*, and correct in production, where a process is a server. Both
/// of these servers are in this one, so the name belongs to whichever started
/// last. Starting the receiver first is what makes that the sender, which is the
/// answer the check needs to be right about.
fn sender_and_receiver() -> ((TestSession, FakePtyHandle), (TestSession, FakePtyHandle)) {
    let receiver = start_with_a_shell();
    let sender = start_with_a_shell();
    (sender, receiver)
}

/// Split off a second pane and give it something of its own to say.
///
/// A warp needs a pane the session can spare — the session's last pane is
/// refused — and the test needs to be able to tell that pane from the shell it
/// was split off, at both ends of the trip.
fn split_off_a_pane(zellij: &TestSession, says: &str) -> FakePtyHandle {
    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(&keys::key('r'));
    let pane = zellij.expect_pty_spawn();
    pane.output(format!("{says}\r\n").as_bytes());
    zellij.wait_until("the new pane to show its output", |grid_snapshot| {
        grid_snapshot.contains(says)
    });
    pane
}

/// Warp the focused pane, and report what the user was told about it.
fn warp_focused_pane_to(
    sender: &TestSession,
    receiver: &TestSession,
    floating: bool,
) -> (i32, String) {
    sender.run_cli_action_reporting(CliAction::WarpPane {
        to_session: receiver.session_name().to_owned(),
        pane_id: None,
        focused_tab: !floating,
        floating,
    })
}

/// Warp a named pane, which is the only way to reach one that is not on screen.
///
/// A pane hidden behind a scrollback editor is suppressed: the editor holds its
/// place, and "the focused pane" is the editor rather than the pane it is editing.
fn warp_pane_by_id(
    sender: &TestSession,
    receiver: &TestSession,
    terminal_id: u32,
) -> (i32, String) {
    sender.run_cli_action_reporting(CliAction::WarpPane {
        to_session: receiver.session_name().to_owned(),
        pane_id: Some(format!("terminal_{terminal_id}")),
        focused_tab: true,
        floating: false,
    })
}

/// Warp the focused pane, insisting it worked.
fn warp_focused_pane_expecting_success(
    sender: &TestSession,
    receiver: &TestSession,
    floating: bool,
) {
    let (exit_code, reported) = warp_focused_pane_to(sender, receiver, floating);
    assert_eq!(exit_code, 0, "the warp was refused: {reported}");
}

/// The run id a reported agent carries here.
///
/// A codex or a claude derives one from its own session id, but the pane in these tests is a
/// shell being reported *for*, so it names the run itself - which is precisely the case that
/// has nothing but this field to be found by after a warp.
const AGENT_RUN_ID: &str = "codex:warped-run";

fn report_waiting_agent(zellij: &TestSession, terminal_id: u32) {
    report_waiting_agent_with_run_id(zellij, terminal_id, Some(AGENT_RUN_ID));
}

fn report_waiting_agent_with_run_id(zellij: &TestSession, terminal_id: u32, run_id: Option<&str>) {
    let (exit_code, reported) = zellij.run_cli_action_reporting(CliAction::AgentReport {
        pane_id: Some(format!("terminal_{terminal_id}")),
        kind: Some("codex".to_owned()),
        state: Some("waiting".to_owned()),
        detail: Some("permission prompt".to_owned()),
        cwd: None,
        hook_json: false,
        clear: false,
        run_id: run_id.map(str::to_owned),
    });
    assert_eq!(exit_code, 0, "the agent report was refused: {reported}");
}

fn assert_waiting_agent(zellij: &TestSession, terminal_id: Option<u32>) {
    let (exit_code, listed) = zellij.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(exit_code, 0, "list-panes failed: {listed}");
    let panes: serde_json::Value = serde_json::from_str(&listed)
        .unwrap_or_else(|e| panic!("list-panes --json did not answer with JSON: {e}\n{listed}"));
    let matching: Vec<_> = panes
        .as_array()
        .expect("list-panes should return an array")
        .iter()
        .filter(|pane| {
            pane["agent"]["kind"] == "codex"
                && terminal_id.is_none_or(|terminal_id| pane["id"] == terminal_id)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "the warped agent pane should be listed exactly once: {panes}"
    );
    let pane = matching[0];
    assert_eq!(pane["agent"]["state"], "Waiting", "{pane}");
    assert_eq!(pane["agent"]["source"], "Reported", "{pane}");
    assert_eq!(pane["agent"]["detail"], "permission prompt", "{pane}");
    // The run id travels with the record, which is what lets the adopting session find the
    // history the pane's agent has been building. Without it a warped run is unrecoverable
    // for an agent that supplies no session id of its own.
    assert_eq!(pane["agent"]["run_id"], AGENT_RUN_ID, "{pane}");
}

/// The stacked pane list's entries, top to bottom, as they are drawn.
///
/// A stack list keeps one member on screen and parks the rest, so what is on
/// screen for the hidden ones is a single row each in a column of titles - which
/// makes the column the only place the *shape* of the list can be read back from.
/// The open member is the one its row is marked with a caret.
fn stack_list_entries(zellij: &TestSession) -> Vec<String> {
    zellij
        .snapshot()
        .lines()
        .into_iter()
        .filter(|line| line.contains("Pane #"))
        .map(|line| line.trim().to_owned())
        .collect()
}

#[test]
fn a_pane_keeps_its_process_when_it_moves_between_sessions() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let travelling = split_off_a_pane(&sender, "here before the warp");
    report_waiting_agent(&sender, travelling.terminal_id());
    assert_waiting_agent(&sender, Some(travelling.terminal_id()));
    // A hook says everything the agent knows about itself, and the agent does not know its
    // run id. So a report that does not name one leaves the pane's alone rather than erasing
    // it - otherwise the very next `PreToolUse` would undo the plugin that set it.
    report_waiting_agent_with_run_id(&sender, travelling.terminal_id(), None);
    assert_waiting_agent(&sender, Some(travelling.terminal_id()));

    warp_focused_pane_expecting_success(&sender, &receiver, false);

    let arrived = receiver.expect_adopted_pty();
    assert!(
        !arrived.is_exited(),
        "the pane's process was killed rather than handed over",
    );
    assert_waiting_agent(&receiver, Some(arrived.terminal_id()));

    sender.wait_until("the pane to leave the sending session", |grid_snapshot| {
        !grid_snapshot.contains("here before the warp")
    });
    receiver.wait_until("the pane to arrive with its scrollback", |grid_snapshot| {
        grid_snapshot.contains("here before the warp")
    });

    // The pane is the receiving session's now: what its process writes is
    // rendered there, and nowhere else.
    arrived.output(b"still running after the warp\r\n");
    receiver.wait_until("the pane to go on writing", |grid_snapshot| {
        grid_snapshot.contains("still running after the warp")
    });
    assert!(
        !sender.snapshot().contains("still running after the warp"),
        "the sending session is still rendering a pane it gave away",
    );
    // Deliberately not asked of `travelling`: that handle names a terminal id in
    // the *sending* session's registry, which the release emptied. It is dangling
    // by design — the pane is not that session's to have an opinion about any
    // more, which is the point.
    drop(travelling);
}

/// Open a pane whose command names another machine.
///
/// The fake runs nothing anywhere, let alone over ssh — what makes this pane
/// "remote" is the `remote_host` on its `RunCommand`, which is also exactly what
/// the real backend branches on. So this exercises the whole warp path for a
/// remote pane without a broker: which address the release builds, that it
/// survives the protobuf trip between two servers, and that the receiving
/// session follows it back to the same process.
fn split_off_a_remote_pane(zellij: &TestSession, says: &str) -> FakePtyHandle {
    zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["cargo".to_string(), "watch".to_string()],
        remote_host: Some("build-box".to_string()),
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    });
    let pane = zellij.expect_pty_spawn();
    pane.output(format!("{says}\r\n").as_bytes());
    zellij.wait_until("the remote pane to show its output", |grid_snapshot| {
        grid_snapshot.contains(says)
    });
    pane
}

/// The failure this whole change is about: before it, releasing a remote pane
/// answered "pane N has no pty-host supervisor", which is true of this machine
/// and misleading about the pane — its supervisor is on the other one.
#[test]
fn a_remote_pane_keeps_its_process_when_it_moves_between_sessions() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let travelling = split_off_a_remote_pane(&sender, "running on the far machine");

    // By id rather than by focus: a pane opened through `run_cli_action` does not
    // take the interactive client's focus, and "the focused pane" is resolved
    // per client.
    let (exit_code, reported) = warp_pane_by_id(&sender, &receiver, travelling.terminal_id());
    assert_eq!(exit_code, 0, "the warp was refused: {reported}");

    let arrived = receiver.expect_adopted_pty();
    assert!(
        !arrived.is_exited(),
        "the remote pane's process was killed rather than handed over",
    );

    sender.wait_until("the pane to leave the sending session", |grid_snapshot| {
        !grid_snapshot.contains("running on the far machine")
    });
    receiver.wait_until("the pane to arrive with its scrollback", |grid_snapshot| {
        grid_snapshot.contains("running on the far machine")
    });

    // The adopted pane is still a remote pane: it knows which machine it is on,
    // which is what a later re-run and the pane's own title read.
    let arrived_command = match arrived.terminal_action() {
        Some(zellij_types::input::command::TerminalAction::RunCommand(run_command)) => run_command,
        other => panic!("an adopted remote pane should carry a command, got {other:?}"),
    };
    assert_eq!(
        arrived_command.target.ssh_host(),
        Some("build-box"),
        "the command travelled without its host, so the pane no longer knows where it runs — a \
         resume carries no command, so this is the only copy there was"
    );
    // Which road was taken, asked at both ends. Nothing above can tell: the
    // pane's state travels either way in this harness, so a remote pane sent
    // down the local road would arrive looking exactly like this one.
    assert_eq!(
        (
            sender.remote_warp_counts().0,
            receiver.remote_warp_counts().1
        ),
        (1, 1),
        "the pane went by a supervisor socket rather than by its broker",
    );

    arrived.output(b"still running after the warp\r\n");
    receiver.wait_until("the pane to go on writing", |grid_snapshot| {
        grid_snapshot.contains("still running after the warp")
    });
    assert!(
        !sender.snapshot().contains("still running after the warp"),
        "the sending session is still rendering a pane it gave away",
    );
    drop(travelling);
}

/// A tab is released whole, so one remote pane in it puts both addressing shapes
/// through the same message — and a mistake in either loses the whole tab.
#[test]
fn a_tab_of_local_and_remote_panes_warps_whole() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let local = split_off_a_pane(&sender, "local pane");
    let remote = split_off_a_remote_pane(&sender, "remote pane");
    report_waiting_agent(&sender, local.terminal_id());
    assert_waiting_agent(&sender, Some(local.terminal_id()));

    let (exit_code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_index: None,
    });
    assert_eq!(exit_code, 0, "the tab warp was refused: {reported}");

    receiver.wait_until("both panes to arrive", |grid_snapshot| {
        grid_snapshot.contains("local pane") && grid_snapshot.contains("remote pane")
    });
    sender.wait_until("both panes to leave", |grid_snapshot| {
        !grid_snapshot.contains("local pane") && !grid_snapshot.contains("remote pane")
    });
    assert_waiting_agent(&receiver, None);

    // Exactly one of the tab's panes went by the broker and the rest did not,
    // which is the whole point of the mixed tab: one released set, two roads.
    // Without this the tab passes even if every pane took the local one.
    assert_eq!(
        (
            sender.remote_warp_counts().0,
            receiver.remote_warp_counts().1
        ),
        (1, 1),
        "the tab's panes did not take one road each as their processes required",
    );

    drop((local, remote));
}

#[test]
fn the_sessions_last_pane_is_refused_and_the_session_survives() {
    // Moving a pane should not be a way to end a session by accident. A tab's
    // last pane is fine — the render pass sweeps a tab with nothing left in it —
    // but the session's last one is not.
    let ((sender, sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    // Every other test here focuses a pane by splitting one off. This one has
    // nothing to split - one pane is the subject - so the focus is established on
    // its own, or the warp is refused for having no pane to move rather than for
    // the reason being tested.
    focus_and_confirm(&sender, &keys::alt('h'), &sender_shell);

    let (exit_code, reported) = warp_focused_pane_to(&sender, &receiver, false);
    assert_ne!(
        exit_code, 0,
        "warping the session's only pane should have been refused",
    );
    assert!(
        reported.contains("only pane"),
        "refused, but for the wrong reason: {reported}",
    );
    assert!(
        !sender_shell.is_exited(),
        "the session's only pane was moved",
    );

    sender_shell.output(b"the session is still here\r\n");
    sender.wait_until("the session to still be running", |grid_snapshot| {
        grid_snapshot.contains("the session is still here")
    });
}

#[test]
fn a_pane_that_cannot_be_warped_is_still_a_working_pane() {
    // The bug this pins: a refused warp used to leave the pane on screen with
    // its reader already aborted, so it took no input and never updated again.
    // A refusal has to cost the user an error message and nothing else, which
    // means it has to be decided before anything is let go.
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let stubborn = split_off_a_pane(&sender, "STUBBORN_MARKER");
    // Already the focused pane: `split_off_a_pane` leaves the new one focused,
    // which is what `warp_focused_pane_to` will move.
    stubborn.fail_warp(WarpFailure::Unwarpable);

    let (exit_code, reported) = warp_focused_pane_to(&sender, &receiver, false);

    assert_ne!(exit_code, 0, "a pane with no supervisor cannot be moved");
    assert!(
        reported.contains("no pty-host supervisor"),
        "refused, but for the wrong reason: {reported}",
    );
    assert!(!stubborn.is_exited(), "a refused warp killed the pane");

    // The two halves of "still a working pane": it can still be typed into, and
    // what it says still reaches the screen.
    let probe = b'q';
    sender.send_stdin(&[probe]);
    sender.wait_until("the pane to receive input after a refused warp", |_| {
        stubborn.stdin_bytes().contains(&probe)
    });
    stubborn.output(b"still alive\r\n");
    sender.wait_until("the pane to still be showing its output", |grid_snapshot| {
        grid_snapshot.contains("still alive")
    });
}

#[test]
fn a_pane_cut_loose_by_a_failed_warp_says_so_instead_of_freezing() {
    // The residual case the check cannot rule out: the supervisor agrees to be
    // asked and then does not confirm the handoff. The pane's reader is gone by
    // then and there is no way to put it back — so the pane is held and told
    // what happened to it, rather than left on screen looking live.
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let doomed = split_off_a_pane(&sender, "DOOMED_MARKER");
    doomed.fail_warp(WarpFailure::Unreleasable);

    let (exit_code, reported) = warp_focused_pane_to(&sender, &receiver, false);

    assert_ne!(exit_code, 0, "a release that did not confirm is not a move");
    assert!(
        reported.contains("did not confirm"),
        "refused, but for the wrong reason: {reported}",
    );
    sender.wait_until("the pane to say it could not be moved", |grid_snapshot| {
        grid_snapshot.contains("could not move this pane")
    });
}

#[test]
fn one_unmovable_pane_stops_a_whole_tab_and_leaves_it_working() {
    // The tab-sized version. Releasing pane by pane and stopping at the first
    // refusal would leave the panes ahead of it handed away with nothing to give
    // them back to; the whole tab is refused while it is still whole.
    let ((sender, sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let travelling = split_off_a_pane(&sender, "TRAVELLING_MARKER");
    // The pane that cannot go is the *second* one asked about, so a release that
    // walked the tab in order would already have let the first one go.
    travelling.fail_warp(WarpFailure::Unwarpable);

    let (exit_code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_index: None,
    });

    assert_ne!(exit_code, 0, "a tab with an unmovable pane cannot be moved");
    assert!(
        reported.contains("no pty-host supervisor"),
        "refused, but for the wrong reason: {reported}",
    );
    assert!(
        !sender_shell.is_exited() && !travelling.is_exited(),
        "a refused tab warp killed a pane",
    );
    // Every pane in the tab is still a pane, including the one that could have
    // gone.
    sender_shell.output(b"shell still alive\r\n");
    travelling.output(b"split still alive\r\n");
    sender.wait_until("both panes to still be showing output", |grid_snapshot| {
        grid_snapshot.contains("shell still alive") && grid_snapshot.contains("split still alive")
    });
}

#[test]
fn warping_a_pane_being_edited_takes_the_editor_down_with_it() {
    // A scrollback editor stands in for the pane it is editing, so the pane is
    // suppressed behind it and neither half is on its own. The pane can still
    // travel — the editor is closed rather than carried, because an editor whose
    // subject is in another session now is showing a dump of a pane that is not
    // there and would restore nothing when closed.
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let being_edited = split_off_a_pane(&sender, "SCROLLBACK_MARKER");

    sender.send_stdin(&keys::ctrl('s'));
    sender.send_stdin(&keys::key('e'));
    let editor = sender.expect_pty_spawn();
    sender.wait_until("the scrollback editor to open", |grid_snapshot| {
        grid_snapshot.contains("EDITING SCROLLBACK") && grid_snapshot.contains("SCROLLBACK_MARKER")
    });

    let (exit_code, reported) = warp_pane_by_id(&sender, &receiver, being_edited.terminal_id());
    assert_eq!(exit_code, 0, "the warp was refused: {reported}");

    let arrived = receiver.expect_adopted_pty();
    assert!(
        !arrived.is_exited(),
        "the pane being edited was killed rather than handed over",
    );
    sender.wait_until(
        "the editor to be closed with its subject",
        |grid_snapshot| !grid_snapshot.contains("EDITING SCROLLBACK"),
    );
    assert!(
        !receiver.snapshot().contains("EDITING SCROLLBACK"),
        "the editor travelled with the pane instead of being closed",
    );
    // Closed on the pty thread too, rather than left as a process nothing owns.
    for _ in 0..50 {
        if editor.is_exited() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        editor.is_exited(),
        "the editor's process was left running with no pane to close it",
    );

    arrived.output(b"edited and then moved\r\n");
    receiver.wait_until(
        "the pane to be running in its new session",
        |grid_snapshot| grid_snapshot.contains("edited and then moved"),
    );
}

#[test]
fn a_tab_holding_a_stacked_pane_list_warps_whole() {
    // A stacked pane list parks all but one of its members, so most of the tab is
    // suppressed. A tab warp has to take those with it: they are travelling, and
    // being re-homed into another tab would leave them there with their
    // supervisors handed away.
    let receiver = start_with_a_shell();
    let (receiver, _receiver_shell) = receiver;
    let sender = start(None);
    let first = sender.expect_pty_spawn();
    first.output(b"first-member\r\n$ ");
    sender.wait_for_app_load();

    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let second = sender.expect_pty_spawn();
    second.output(b"second-member\r\n$ ");
    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let third = sender.expect_pty_spawn();
    third.output(b"third-member\r\n$ ");
    sender.wait_until("the stack to be built", |grid_snapshot| {
        grid_snapshot.contains("third-member")
    });

    let (exit_code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_index: None,
    });
    assert_eq!(exit_code, 0, "the tab warp was refused: {reported}");

    // Every member travels, hidden ones included: three panes were in the tab and
    // three supervisors have to be taken over, or one is stranded.
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    for member in &adopted {
        assert!(
            !member.is_exited(),
            "a stack-list member was killed rather than handed over",
        );
    }
    receiver.wait_until("the stack to arrive", |grid_snapshot| {
        grid_snapshot.contains("third-member")
    });
    // Three ptys were taken over, which says nothing about what the user got:
    // three loose panes would satisfy it just as well. The tab has to have
    // arrived as a *list* - an entry per member, in the order they were stacked,
    // with the one that was open still the open one.
    let entries = stack_list_entries(&receiver);
    assert_eq!(
        entries.len(),
        3,
        "the stack should have arrived whole, but the list reads {entries:?}",
    );
    for (index, entry) in entries.iter().enumerate() {
        let expected = format!("Pane #{}", index + 1);
        assert!(
            entry.contains(&expected),
            "entry {index} should be {expected}, but reads {entry:?}",
        );
        assert_eq!(
            entry.contains('>'),
            index == 2,
            "the member that was open here should be the open one there: {entries:?}",
        );
    }

    // And it is a list rather than a picture of one: selecting a member that
    // travelled hidden brings its own scrollback forward, so its process and its
    // slot in the list are the same pane.
    let mut selected = false;
    for _ in 0..50 {
        receiver.send_stdin(&keys::alt('k'));
        if receiver.snapshot().contains("second-member") {
            selected = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        selected,
        "a member that travelled hidden could not be selected in its new session",
    );
    // Deliberately nothing asked of `first`/`second`/`third`: those handles name
    // terminal ids in the *sending* session's registry, which releasing them
    // emptied. What became of the processes is a question only the receiving
    // session can answer now, and it has just answered it.
    drop((first, second, third));
}
