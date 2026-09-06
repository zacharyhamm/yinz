//! Tests for the ordering guarantees of a warp.
//!
//! The thing worth pinning down here is not the happy path — that needs two real
//! servers and lives in the manual checks — but *what happens before anything is
//! released*. Between releasing a supervisor and the far session adopting it, the
//! pane belongs to nobody; every check that can be made before that window opens
//! must actually be made before it, or a typo in a session name costs a user a
//! pane instead of an error message.

use super::*;
use zellij_types::internal_ids::SupervisorId;
use zellij_types::internal_ids::TabId;

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use std::path::PathBuf;
use zellij_runtime::channels::{unbounded, SenderWithContext};
use zellij_types::diagnostics::ErrorContext;

/// A short budget for the timeout paths. Long enough that a loaded CI machine
/// will not trip it by accident, short enough that the suite stays quick.
const TEST_TIMEOUT: Duration = Duration::from_millis(200);

struct Harness {
    senders: ThreadSenders,
    screen: zellij_runtime::channels::Receiver<(ScreenInstruction, ErrorContext)>,
    pty: zellij_runtime::channels::Receiver<(PtyInstruction, ErrorContext)>,
}

fn harness() -> Harness {
    let (to_screen, screen) = unbounded();
    let (to_pty, pty) = unbounded();
    Harness {
        senders: ThreadSenders {
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_pty: Some(SenderWithContext::new(to_pty)),
            ..Default::default()
        },
        screen,
        pty,
    }
}

/// A session name nothing else in the suite can collide with, and which is
/// certainly not a running session.
fn unreachable_session(tag: &str) -> String {
    format!("warp-tests-no-such-session-{tag}")
}

#[test]
fn an_input_unblock_before_the_warp_reply_is_ignored() {
    let socket_dir = tempfile::TempDir::new().expect("failed to create temp dir");
    let socket_path = socket_dir.path().join("warp-reply.sock");
    let listener = ListenerOptions::new()
        .name(
            socket_path
                .as_path()
                .to_fs_name::<GenericFilePath>()
                .expect("invalid socket path"),
        )
        .create_sync()
        .expect("failed to bind socket");
    let server = std::thread::spawn(move || {
        let stream = listener
            .incoming()
            .next()
            .expect("listener stopped")
            .expect("failed to accept connection");
        let mut sender: IpcSenderWithContext<ServerToClientMsg> = IpcSenderWithContext::new(stream);
        sender
            .send_server_msg(ServerToClientMsg::UnblockInputThread)
            .expect("failed to send input unblock");
        sender
            .send_server_msg(ServerToClientMsg::Log {
                lines: vec!["Adopted pane".to_owned()],
            })
            .expect("failed to send warp reply");
    });

    let stream = LocalSocketStream::connect(
        socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("invalid socket path"),
    )
    .expect("failed to connect socket");
    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);

    assert_eq!(
        read_cli_reply(&mut sender).expect("the warp reply should follow the input unblock"),
        "Adopted pane"
    );
    server.join().expect("server thread panicked");
}

#[test]
fn warping_to_the_current_session_is_refused_before_anything_is_asked_of_screen() {
    let harness = harness();

    let result = warp_pane_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "here",
        "here",
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    assert_eq!(
        result,
        Err("this pane is already in session 'here'".to_owned())
    );
    assert!(
        harness.screen.try_recv().is_err(),
        "screen should not have been asked to release a pane"
    );
}

#[test]
fn an_unreachable_target_releases_nothing() {
    // The load-bearing invariant: reachability is checked *first*, so a session
    // name that does not resolve costs an error and not a homeless pane.
    let harness = harness();

    let result = warp_pane_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "here",
        &unreachable_session("unreachable"),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    let error = result.expect_err("a session that is not running cannot be warped to");
    assert!(
        error.contains("is not running"),
        "expected the error to name the missing session, got: {error}"
    );
    assert!(
        harness.screen.try_recv().is_err(),
        "screen should not have been asked to release a pane"
    );
}

#[test]
fn bringing_a_pane_from_the_current_session_is_refused() {
    let result = warp_pane_from_session(
        "here",
        "here",
        PaneId::Terminal(crate::TerminalId::new(3)),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    assert_eq!(result, Err("'here' is this session".to_owned()));
}

#[test]
fn bringing_a_pane_into_a_nameless_session_is_refused() {
    // `warp_pane_from_session` names *us* as the destination, so a server with
    // no name has nothing to put in that field and must say so rather than
    // asking the far side to warp to "".
    let result = warp_pane_from_session(
        "",
        &unreachable_session("nameless"),
        PaneId::Terminal(crate::TerminalId::new(3)),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    assert_eq!(
        result,
        Err("this session has no name to bring the pane to".to_owned())
    );
}

#[test]
fn bringing_a_pane_from_an_unreachable_session_reports_which_one() {
    let missing = unreachable_session("source");

    let result = warp_pane_from_session(
        "here",
        &missing,
        PaneId::Terminal(crate::TerminalId::new(3)),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    let error = result.expect_err("a session that is not running has no pane to give");
    assert!(
        error.contains(&missing),
        "expected the error to name the missing session, got: {error}"
    );
}

#[test]
fn adopting_times_out_rather_than_waiting_on_a_pty_thread_that_never_answers() {
    let harness = harness();

    let result = adopt_pane_here(
        &harness.senders,
        crate::ClientId::new(1),
        Box::new(WarpedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: Some("top".to_owned()),
            contents: None,
            borderless: false,
            remote: None,
            agent: None,
        }),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );

    assert_eq!(result, Err("timed out adopting the pane".to_owned()));
    // It did ask, though — the timeout is the pty thread failing to answer, not
    // the request never being made.
    assert!(matches!(
        harness.pty.try_recv(),
        Ok((PtyInstruction::AdoptTerminal { .. }, _))
    ));
}

#[test]
fn a_pty_side_refusal_to_adopt_comes_back_verbatim() {
    // The whole value of returning a string rather than logging one is that the
    // caller — a user at a cli, or a model reading a tool result — sees the real
    // reason. Nothing in this path may reword it.
    let harness = harness();
    let senders = harness.senders.clone();
    let responder = std::thread::spawn(move || {
        let (instruction, _) = harness.pty.recv().expect("the request should arrive");
        match instruction {
            PtyInstruction::AdoptTerminal {
                response_channel, ..
            } => {
                let _ = response_channel.send(Err(anyhow!("the supervisor is already attached")));
            },
            other => panic!("unexpected instruction: {other:?}"),
        }
    });

    let result = adopt_pane_here(
        &senders,
        crate::ClientId::new(1),
        Box::new(WarpedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: None,
            contents: None,
            borderless: false,
            remote: None,
            agent: None,
        }),
        WarpPlacement::NewTab,
        Duration::from_secs(5),
    );
    responder.join().expect("the responder should not panic");

    assert_eq!(result, Err("the supervisor is already attached".to_owned()));
}

// ---------------------------------------------------------------------------
// The same guarantees, a tab at a time
// ---------------------------------------------------------------------------
//
// Everything above matters more here. A tab warp's window of nobody-owns-this
// holds every process in the tab, so a check that happened after the release
// instead of before it would cost a user a tab rather than a pane.

#[test]
fn warping_a_tab_to_the_current_session_is_refused_before_anything_is_asked_of_screen() {
    let harness = harness();

    let result = warp_tab_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(TabId::new(0)),
        "here",
        "here",
        TEST_TIMEOUT,
    );

    assert_eq!(
        result,
        Err("this tab is already in session 'here'".to_owned())
    );
    assert!(
        harness.screen.try_recv().is_err(),
        "screen should not have been asked to release a tab"
    );
}

#[test]
fn an_unreachable_target_releases_no_part_of_a_tab() {
    // The load-bearing invariant, restated for tabs: reachability is checked
    // first, so a session name that does not resolve costs an error rather than
    // a tab's worth of homeless processes.
    let harness = harness();

    let result = warp_tab_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(TabId::new(0)),
        "here",
        &unreachable_session("tab-unreachable"),
        TEST_TIMEOUT,
    );

    let error = result.expect_err("a session that is not running cannot be warped to");
    assert!(
        error.contains("is not running"),
        "expected the error to name the missing session, got: {error}"
    );
    assert!(
        harness.screen.try_recv().is_err(),
        "screen should not have been asked to release a tab"
    );
}

#[test]
fn bringing_a_tab_from_the_current_session_is_refused() {
    let result = warp_tab_from_session("here", "here", TabId::new(0), TEST_TIMEOUT);

    assert_eq!(result, Err("'here' is this session".to_owned()));
}

#[test]
fn bringing_a_tab_into_a_nameless_session_is_refused() {
    // A session with no name has no address to give the source, so asking would
    // send a tab to nowhere.
    let result = warp_tab_from_session(
        "",
        &unreachable_session("nameless-tab"),
        TabId::new(0),
        TEST_TIMEOUT,
    );

    assert_eq!(
        result,
        Err("this session has no name to bring the tab to".to_owned())
    );
}

#[test]
fn bringing_a_tab_from_an_unreachable_session_reports_which_one() {
    let missing = unreachable_session("tab-source");

    let result = warp_tab_from_session("here", &missing, TabId::new(0), TEST_TIMEOUT);

    let error = result.expect_err("a session that is not running has no tab to give");
    assert!(
        error.contains(&missing),
        "expected the error to name the missing session, got: {error}"
    );
}

#[test]
fn adopting_a_tab_times_out_rather_than_waiting_on_a_pty_thread_that_never_answers() {
    let harness = harness();

    let result = adopt_tab_here(
        &harness.senders,
        crate::ClientId::new(1),
        Box::default(),
        TEST_TIMEOUT,
    );

    assert_eq!(result, Err("timed out adopting the tab".to_owned()));
    // It did ask, though — the timeout is the pty thread failing to answer, not
    // the request never being made.
    assert!(matches!(
        harness.pty.try_recv(),
        Ok((PtyInstruction::AdoptTab { .. }, _))
    ));
}

#[test]
fn a_pty_side_refusal_to_adopt_a_tab_comes_back_verbatim() {
    // Doubly true for a tab: the refusal is the only place the list of
    // supervisors left running gets written down.
    let harness = harness();
    let senders = harness.senders.clone();
    let responder = std::thread::spawn(move || {
        let (instruction, _) = harness.pty.recv().expect("the request should arrive");
        match instruction {
            PtyInstruction::AdoptTab {
                response_channel, ..
            } => {
                let _ = response_channel.send(Err(anyhow!(
                    "recover them with `zellij pty-host adopt abc123`"
                )));
            },
            other => panic!("unexpected instruction: {other:?}"),
        }
    });

    let result = adopt_tab_here(
        &senders,
        crate::ClientId::new(1),
        Box::default(),
        Duration::from_secs(5),
    );
    responder.join().expect("the responder should not panic");

    assert_eq!(
        result,
        Err("recover them with `zellij pty-host adopt abc123`".to_owned())
    );
}

// --- what a stranded pane is called ---
//
// Only ever reached when a warp has failed in the one way it cannot undo: the
// pane released, the handover refused, and the rollback refused too. The line
// this produces is all that stands between the user and the work in that pane,
// so it has to be a command that actually applies — and the two kinds of pane
// are not recovered by the same one.

fn remote_pane(host: &str, pane_token: &str) -> WarpedPane {
    WarpedPane {
        host_id: SupervisorId::new(String::new()),
        control_socket: PathBuf::new(),
        title: None,
        contents: None,
        borderless: false,
        remote: Some(zellij_types::data::RemotePaneHandoff {
            host: host.to_owned(),
            pane_token: pane_token.to_owned(),
            received: 0,
            cmd: Default::default(),
        }),
        agent: None,
    }
}

#[test]
fn a_remote_pane_is_recovered_through_the_broker_holding_it() {
    // `host_id` is empty for a remote pane, so the local wording would print
    // `zellij pty-host adopt` with nothing after it — a command that names no
    // pane and silently does nothing useful.
    assert_eq!(
        recovery_command(&remote_pane("build-box", "d6a7b8c9e0f1")),
        "zellij pty-host adopt --host build-box d6a7b8c9e0f1",
    );
}

#[test]
fn a_local_pane_is_recovered_by_its_supervisor_id() {
    assert_eq!(
        recovery_command(&WarpedPane {
            host_id: SupervisorId::new("abc123".to_owned()),
            ..Default::default()
        }),
        "zellij pty-host adopt abc123",
    );
}
