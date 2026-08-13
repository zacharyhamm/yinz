//! Tests for the ordering guarantees of a warp.
//!
//! The thing worth pinning down here is not the happy path — that needs two real
//! servers and lives in the manual checks — but *what happens before anything is
//! released*. Between releasing a supervisor and the far session adopting it, the
//! pane belongs to nobody; every check that can be made before that window opens
//! must actually be made before it, or a typo in a session name costs a user a
//! pane instead of an error message.

use super::*;

use std::path::PathBuf;
use zellij_utils::channels::{unbounded, SenderWithContext};
use zellij_utils::errors::ErrorContext;

/// A short budget for the timeout paths. Long enough that a loaded CI machine
/// will not trip it by accident, short enough that the suite stays quick.
const TEST_TIMEOUT: Duration = Duration::from_millis(200);

struct Harness {
    senders: ThreadSenders,
    screen: zellij_utils::channels::Receiver<(ScreenInstruction, ErrorContext)>,
    pty: zellij_utils::channels::Receiver<(PtyInstruction, ErrorContext)>,
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
fn warping_to_the_current_session_is_refused_before_anything_is_asked_of_screen() {
    let harness = harness();

    let result = warp_pane_to_session(
        &harness.senders,
        1,
        Some(PaneId::Terminal(3)),
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
        1,
        Some(PaneId::Terminal(3)),
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
        PaneId::Terminal(3),
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
        PaneId::Terminal(3),
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
        PaneId::Terminal(3),
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
        1,
        Box::new(WarpedPane {
            host_id: "abc123".to_owned(),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: Some("top".to_owned()),
            contents: None,
            borderless: false,
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
        1,
        Box::new(WarpedPane {
            host_id: "abc123".to_owned(),
            control_socket: PathBuf::from("/nonexistent.sock"),
            title: None,
            contents: None,
            borderless: false,
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

    let result = warp_tab_to_session(&harness.senders, 1, Some(0), "here", "here", TEST_TIMEOUT);

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
        1,
        Some(0),
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
    let result = warp_tab_from_session("here", "here", 0, TEST_TIMEOUT);

    assert_eq!(result, Err("'here' is this session".to_owned()));
}

#[test]
fn bringing_a_tab_into_a_nameless_session_is_refused() {
    // A session with no name has no address to give the source, so asking would
    // send a tab to nowhere.
    let result = warp_tab_from_session("", &unreachable_session("nameless-tab"), 0, TEST_TIMEOUT);

    assert_eq!(
        result,
        Err("this session has no name to bring the tab to".to_owned())
    );
}

#[test]
fn bringing_a_tab_from_an_unreachable_session_reports_which_one() {
    let missing = unreachable_session("tab-source");

    let result = warp_tab_from_session("here", &missing, 0, TEST_TIMEOUT);

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
        1,
        Box::new(WarpedTab::default()),
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
        1,
        Box::new(WarpedTab::default()),
        Duration::from_secs(5),
    );
    responder.join().expect("the responder should not panic");

    assert_eq!(
        result,
        Err("recover them with `zellij pty-host adopt abc123`".to_owned())
    );
}
