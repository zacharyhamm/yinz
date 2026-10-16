//! Tests for the ordering guarantees of a warp.
//!
//! The thing worth pinning down here is not the happy path — that needs two real
//! servers and lives in the manual checks — but *what happens before anything is
//! released*. Between releasing a supervisor and the far session adopting it, the
//! pane belongs to nobody; every check that can be made before that window opens
//! must actually be made before it, or a typo in a session name costs a user a
//! pane instead of an error message.

use super::*;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use std::path::PathBuf;
use yinz_runtime::channels::{unbounded, SenderWithContext};
use yinz_types::diagnostics::ErrorContext;

/// A short budget for the timeout paths. Long enough that a loaded CI machine
/// will not trip it by accident, short enough that the suite stays quick.
const TEST_TIMEOUT: Duration = Duration::from_millis(200);

struct Harness {
    senders: ThreadSenders,
    screen: yinz_runtime::channels::Receiver<(ScreenInstruction, ErrorContext)>,
    pty: yinz_runtime::channels::Receiver<(PtyInstruction, ErrorContext)>,
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

fn read_reply_after(message: ServerToClientMsg) -> Result<String> {
    read_reply_from(Duration::from_secs(10), move |mut sender| {
        sender
            .send_server_msg(message)
            .expect("failed to send server message");
    })
}

/// What `read_cli_reply`, waiting up to `timeout`, makes of a connection the server
/// side of which `serve` drives.
fn read_reply_from(
    timeout: Duration,
    serve: impl FnOnce(IpcSenderWithContext<ServerToClientMsg>) + Send + 'static,
) -> Result<String> {
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
        serve(IpcSenderWithContext::new(stream));
    });

    let stream = LocalSocketStream::connect(
        socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("invalid socket path"),
    )
    .expect("failed to connect socket");
    bound_reply_time(&stream, timeout);
    let mut sender: IpcSenderWithContext<ClientToServerMsg> = IpcSenderWithContext::new(stream);

    let reply = read_cli_reply(&mut sender);
    // Closed, so a server still writing fails rather than blocking on a full socket.
    drop(sender);
    server.join().expect("server thread panicked");
    reply
}

#[test]
fn a_successful_warp_reply_returns_its_message() {
    let reply = read_reply_after(ServerToClientMsg::Log {
        lines: vec!["Adopted".to_owned(), "pane".to_owned()],
    });
    assert_eq!(reply.unwrap(), "Adopted pane");
}

#[test]
fn a_warp_refusal_returns_its_reason() {
    let reply = read_reply_after(ServerToClientMsg::LogError {
        lines: vec!["there is no tab 4 in this session".to_owned()],
    });
    assert_eq!(
        format!("{:#}", reply.expect_err("a LogError is a refusal")),
        "there is no tab 4 in this session"
    );
}

/// What a session running an older server sends a warp connection ahead of its answer.
fn older_server_pushes() -> [ServerToClientMsg; 3] {
    [
        ServerToClientMsg::UnblockInputThread,
        ServerToClientMsg::SetMouseMode { enabled: true },
        ServerToClientMsg::ConfigFileUpdated,
    ]
}

#[test]
fn an_unexpected_warp_reply_is_an_error() {
    let message = ServerToClientMsg::QueryTerminalSize;
    let expected = format!("unexpected reply from the other session: {message:?}");
    let reply = read_reply_after(message);
    assert_eq!(format!("{:#}", reply.unwrap_err()), expected);
}

#[test]
fn an_older_servers_pushes_before_the_warp_reply_are_skipped() {
    let reply = read_reply_from(Duration::from_secs(10), |mut sender| {
        for message in older_server_pushes() {
            sender
                .send_server_msg(message)
                .expect("failed to send server push");
        }
        sender
            .send_server_msg(ServerToClientMsg::Log {
                lines: vec!["Adopted pane".to_owned()],
            })
            .expect("failed to send warp reply");
    });
    assert_eq!(reply.unwrap(), "Adopted pane");
}

#[test]
fn an_unanswered_warp_reply_times_out() {
    let reply = read_reply_from(TEST_TIMEOUT, |sender| {
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            sender.get_receiver().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(
            receiver.recv_client_msg().is_eof(),
            "the warp client must give up first"
        );
    });
    reply.expect_err("no reply arrived");
}

#[test]
fn a_closed_warp_connection_without_a_reply_is_an_error() {
    let reply = read_reply_from(Duration::from_secs(10), |_| {});
    assert_eq!(
        format!("{:#}", reply.unwrap_err()),
        "the other session closed the connection without answering"
    );
}

#[test]
fn a_shutdown_warp_reply_is_distinct_from_other_errors() {
    let reply = read_reply_after(ServerToClientMsg::Exit {
        exit_reason: yinz_ipc::ipc::ExitReason::Normal,
    });
    assert!(reply.unwrap_err().downcast_ref::<SessionEnded>().is_some());
}

#[test]
fn the_capability_probe_reads_its_answer_after_an_older_servers_pushes() {
    let session = FakeSession::bind("probe-answer");
    let listener = session.listener.as_ref().unwrap();
    let probe = {
        let name = session.name.clone();
        std::thread::spawn(move || warp_capabilities(&name, Duration::from_secs(10)))
    };
    let mut answer: IpcSenderWithContext<ServerToClientMsg> =
        IpcSenderWithContext::new(listener.accept().unwrap());
    let mut request: IpcReceiverWithContext<ClientToServerMsg> = answer.get_receiver().unwrap();
    assert!(matches!(
        request.recv_client_msg().msg(),
        Some((ClientToServerMsg::ConnStatus, _))
    ));
    for message in older_server_pushes() {
        answer.send_server_msg(message).unwrap();
    }
    answer
        .send_server_msg(ServerToClientMsg::Connected {
            warp_companions: true,
            warp_held_terminals: false,
        })
        .unwrap();
    assert_eq!(
        probe
            .join()
            .unwrap()
            .expect("the probe should receive its answer"),
        (true, false)
    );
}

#[test]
fn an_unanswered_capability_probe_times_out() {
    let session = FakeSession::bind("probe-timeout");
    let listener = session.listener.as_ref().unwrap();
    let probe = {
        let name = session.name.clone();
        std::thread::spawn(move || warp_capabilities(&name, TEST_TIMEOUT))
    };
    let mut request: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(listener.accept().unwrap());
    request
        .set_recv_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    assert!(matches!(
        request.recv_client_msg().msg(),
        Some((ClientToServerMsg::ConnStatus, _))
    ));
    assert!(
        request.recv_client_msg().is_eof(),
        "the probe must give up first"
    );
    assert_eq!(
        format!("{:#}", probe.join().unwrap().unwrap_err()),
        "destination did not answer the warp capability probe"
    );
}

#[test]
fn warping_to_the_current_session_is_refused_before_anything_is_asked_of_screen() {
    let harness = harness();

    let (result, hold) = warp_pane_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new("here"),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );
    assert!(hold.is_none(), "nothing was released");

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

    let (result, hold) = warp_pane_to_session(
        &harness.senders,
        crate::ClientId::new(1),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new(unreachable_session("unreachable")),
        WarpPlacement::NewTab,
        TEST_TIMEOUT,
    );
    assert!(hold.is_none(), "nothing was released");

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
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new("here"),
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
        &yinz_types::internal_ids::SessionName::new(""),
        &yinz_types::internal_ids::SessionName::new(unreachable_session("nameless")),
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
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new(&missing),
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
            agent_theme: None,
            bracketed_paste: false,
            agent_input: None,
            held: None,
            group: None,
            host_id: SupervisorId::new("abc123".to_owned()),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: Some("top".to_owned()),
            caption: None,
            contents: None,
            borderless: false,
            remote: None,
            agent: None,
        }),
        WarpPlacement::NewTab,
        None,
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
            agent_theme: None,
            bracketed_paste: false,
            agent_input: None,
            held: None,
            group: None,
            host_id: SupervisorId::new("abc123".to_owned()),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: None,
            caption: None,
            contents: None,
            borderless: false,
            remote: None,
            agent: None,
        }),
        WarpPlacement::NewTab,
        None,
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
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new("here"),
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
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new(unreachable_session("tab-unreachable")),
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
    let result = warp_tab_from_session(
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new("here"),
        TabId::new(0),
        TEST_TIMEOUT,
    );

    assert_eq!(result, Err("'here' is this session".to_owned()));
}

#[test]
fn bringing_a_tab_into_a_nameless_session_is_refused() {
    // A session with no name has no address to give the source, so asking would
    // send a tab to nowhere.
    let result = warp_tab_from_session(
        &yinz_types::internal_ids::SessionName::new(""),
        &yinz_types::internal_ids::SessionName::new(unreachable_session("nameless-tab")),
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

    let result = warp_tab_from_session(
        &yinz_types::internal_ids::SessionName::new("here"),
        &yinz_types::internal_ids::SessionName::new(&missing),
        TabId::new(0),
        TEST_TIMEOUT,
    );

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
                    "recover them with `yinz pty-host adopt abc123`"
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
        Err("recover them with `yinz pty-host adopt abc123`".to_owned())
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
        agent_theme: None,
        bracketed_paste: false,
        agent_input: None,
        held: None,
        group: None,
        host_id: SupervisorId::new(String::new()),
        control_socket: PathBuf::new(),
        title: None,
        caption: None,
        contents: None,
        borderless: false,
        remote: Some(yinz_types::data::RemotePaneHandoff {
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
    // `yinz pty-host adopt` with nothing after it — a command that names no
    // pane and silently does nothing useful.
    assert_eq!(
        recovery_command(&remote_pane("build-box", "d6a7b8c9e0f1")),
        "yinz pty-host adopt --host build-box d6a7b8c9e0f1",
    );
}

#[test]
fn a_local_pane_is_recovered_by_its_supervisor_id() {
    assert_eq!(
        recovery_command(&WarpedPane {
            agent_theme: None,
            held: None,
            host_id: SupervisorId::new("abc123".to_owned()),
            ..Default::default()
        }),
        "yinz pty-host adopt abc123",
    );
}

#[test]
fn warp_pull_budget_covers_probe_release_adoption_and_rollback() {
    assert_eq!(WARP_PULL_TIMEOUT, WARP_TIMEOUT * 4 + Duration::from_secs(1));
}

/// Binds a fake session socket in the real socket directory, where
/// `connect_to_session` looks, and removes it on drop.
struct FakeSession {
    name: SessionName,
    path: PathBuf,
    listener: Option<interprocess::local_socket::Listener>,
}

impl FakeSession {
    fn bind(tag: &str) -> Self {
        let name = SessionName::new(format!("warp-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
        let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        Self {
            name,
            path,
            listener: Some(listener),
        }
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Answers the capability probe, then reads the handoff and closes it unanswered.
fn refuse_handoff(listener: interprocess::local_socket::Listener) {
    let mut probe: IpcSenderWithContext<ServerToClientMsg> =
        IpcSenderWithContext::new(listener.accept().unwrap());
    let mut request: IpcReceiverWithContext<ClientToServerMsg> = probe.get_receiver().unwrap();
    assert!(matches!(
        request.recv_client_msg().msg(),
        Some((ClientToServerMsg::ConnStatus, _))
    ));
    probe
        .send_server_msg(ServerToClientMsg::Connected {
            warp_companions: true,
            warp_held_terminals: true,
        })
        .unwrap();
    let mut adoption: IpcReceiverWithContext<ClientToServerMsg> =
        IpcReceiverWithContext::new(listener.accept().unwrap());
    let _ = adoption.recv_client_msg();
}

fn start_pane_warp(
    harness: &Harness,
    to_session: &SessionName,
) -> std::thread::JoinHandle<(std::result::Result<String, String>, Option<PaneWarpHold>)> {
    let senders = harness.senders.clone();
    let to_session = to_session.clone();
    std::thread::spawn(move || {
        warp_pane_to_session(
            &senders,
            crate::ClientId::new(1),
            None,
            &SessionName::new("source"),
            &to_session,
            WarpPlacement::FocusedTab,
            Duration::from_secs(5),
        )
    })
}

fn screen_warp_reply(harness: &Harness) -> PaneWarpReply {
    let (
        ScreenInstruction::WarpPane {
            response_channel, ..
        },
        _,
    ) = harness.screen.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("expected warp")
    };
    response_channel
}

#[test]
fn a_refused_pane_warp_never_holds_the_session() {
    let harness = harness();
    let mut session = FakeSession::bind("refused");
    let destination = std::thread::spawn({
        let listener = session.listener.take().unwrap();
        move || refuse_handoff(listener)
    });
    let caller = start_pane_warp(&harness, &session.name);
    assert!(screen_warp_reply(&harness)
        .answer(Err("preparation refused".into()))
        .is_none());
    let (result, hold) = caller.join().unwrap();
    let error = result.unwrap_err();
    assert!(error.contains("preparation refused"), "{error}");
    assert!(hold.is_none());
    assert!(harness.screen.try_recv().is_err());
    destination.join().unwrap();
}

#[test]
fn a_rollback_keeps_the_warp_hold_until_the_adoption_is_done_with_it() {
    let harness = harness();
    let mut session = FakeSession::bind("rollback");
    let destination = std::thread::spawn({
        let listener = session.listener.take().unwrap();
        move || refuse_handoff(listener)
    });
    let caller = start_pane_warp(&harness, &session.name);
    assert!(screen_warp_reply(&harness)
        .answer(Ok((
            Box::default(),
            PaneWarpHold::new(harness.senders.clone()),
        )))
        .is_none());
    let (instruction, _) = harness.pty.recv_timeout(Duration::from_secs(5)).unwrap();
    let PtyInstruction::AdoptTerminal {
        ref response_channel,
        ref hold,
        ..
    } = instruction
    else {
        panic!("expected rollback")
    };
    assert!(hold.is_some(), "the rollback must carry the hold");
    response_channel
        .send(Err(anyhow!("rollback refused")))
        .unwrap();
    let (result, reply_hold) = caller.join().unwrap();
    let error = result.unwrap_err();
    assert!(error.contains("rollback refused"), "{error}");
    assert!(
        reply_hold.is_some(),
        "the caller keeps the session open until it has answered"
    );
    drop(reply_hold);
    assert!(
        harness.screen.try_recv().is_err(),
        "the caller giving up must not finish the warp"
    );
    drop(instruction);
    assert!(matches!(
        harness
            .screen
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .0,
        ScreenInstruction::PaneWarpFinished
    ));
    destination.join().unwrap();
}

#[test]
fn a_source_that_ends_without_answering_a_pane_pull_is_a_failure() {
    let mut session = FakeSession::bind("pull-last");
    let listener = session.listener.take().unwrap();
    let source = std::thread::spawn(move || {
        let mut reply: IpcSenderWithContext<ServerToClientMsg> =
            IpcSenderWithContext::new(listener.accept().unwrap());
        let mut request: IpcReceiverWithContext<ClientToServerMsg> = reply.get_receiver().unwrap();
        assert!(matches!(
            request.recv_client_msg().msg(),
            Some((ClientToServerMsg::Action { .. }, _))
        ));
        reply
            .send_server_msg(ServerToClientMsg::Exit {
                exit_reason: yinz_ipc::ipc::ExitReason::Normal,
            })
            .unwrap();
    });
    let result = warp_pane_from_session(
        &SessionName::new("here"),
        &session.name,
        PaneId::Terminal(crate::TerminalId::new(1)),
        WarpPlacement::FocusedTab,
        Duration::from_secs(5),
    );
    source.join().unwrap();
    let error = result.unwrap_err();
    assert!(
        error.contains("ended before confirming the pane arrived"),
        "{error}"
    );
}

fn adopt_in_thread(
    harness: &Harness,
) -> std::thread::JoinHandle<std::result::Result<String, String>> {
    let senders = harness.senders.clone();
    std::thread::spawn(move || {
        adopt_pane_here(
            &senders,
            crate::ClientId::new(1),
            Box::default(),
            WarpPlacement::NewTab,
            None,
            Duration::from_secs(5),
        )
    })
}

fn attached_adoption(harness: &Harness) -> crossbeam::channel::Sender<Result<()>> {
    let (instruction, _) = harness.pty.recv_timeout(Duration::from_secs(5)).unwrap();
    let PtyInstruction::AdoptTerminal {
        response_channel,
        placed,
        ..
    } = instruction
    else {
        panic!("expected adoption")
    };
    response_channel.send(Ok(())).unwrap();
    placed
}

#[test]
fn an_adoption_that_was_given_back_is_reported_with_its_recovery() {
    let harness = harness();
    let adopter = adopt_in_thread(&harness);
    attached_adoption(&harness)
        .send(Err(anyhow!(
            "given back. Recover it with `yinz pty-host adopt abc`"
        )))
        .unwrap();
    let error = adopter.join().unwrap().unwrap_err();
    assert!(error.contains("yinz pty-host adopt abc"), "{error}");
}

#[test]
fn an_attached_adoption_with_no_placement_answer_is_owned_here() {
    let harness = harness();
    let adopter = adopt_in_thread(&harness);
    drop(attached_adoption(&harness));
    assert_eq!(adopter.join().unwrap(), Ok("Adopted pane".to_owned()));
}

#[test]
fn a_pane_warp_answer_racing_a_timeout_is_received_or_handed_back() {
    let harness = harness();
    let released =
        || -> ReleasedPaneWarp { (Box::default(), PaneWarpHold::new(harness.senders.clone())) };

    // Answered before the waiter gave up, but not read in time.
    let (reply, waiter) = pane_warp_reply();
    assert!(reply.answer(Ok(released())).is_none());
    assert!(matches!(waiter.recv_timeout(Duration::ZERO), Ok(Ok(_))));

    // Answered after it gave up.
    let (reply, waiter) = pane_warp_reply();
    assert!(waiter.recv_timeout(Duration::ZERO).is_err());
    assert!(reply.answer(Ok(released())).is_some());

    // Answered after it was dropped.
    let (reply, waiter) = pane_warp_reply();
    drop(waiter);
    assert!(reply.answer(Ok(released())).is_some());
}
