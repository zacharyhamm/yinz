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

use zellij_integration_tests::{
    keys, FakePtyHandle, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};
use zellij_utils::cli::CliAction;

/// Keep each session to its shell pane: the default background plugins would
/// otherwise be spawning panes of their own while the test watches for one.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

/// A left-edge global pane in front of an ordinary tab pane.
///
/// A global pane is docked to the session rather than held by a tab, which makes
/// it the wrong answer to both questions a warp asks: which pane the user means
/// by "this one", and where a pane arriving from elsewhere should land.
const GLOBAL_AND_SHELL: &str = r#"layout {
    global_panes {
        pane edge="left" size="30%"
    }
    pane
}
"#;

/// What the global pane writes, so it can be told apart from everything else and
/// found on screen.
const GLOBAL_MARKER: &str = "GLOBAL";

/// How wide `GLOBAL_AND_SHELL`'s strip is, in columns. Anything at a smaller
/// column than this is inside the global pane rather than in the tab.
const GLOBAL_STRIP_COLS: usize = TERMINAL_SIZE.cols * 30 / 100;

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

/// A session with a left-edge global pane as well as its shell pane.
///
/// The global pane spawns first, so it is the first pty the session asks for.
fn start_with_a_global_pane() -> (TestSession, FakePtyHandle, FakePtyHandle) {
    let zellij = start(Some(GLOBAL_AND_SHELL));
    let global = zellij.expect_pty_spawn();
    global.output(GLOBAL_MARKER.as_bytes());
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_until("the global pane and the shell to load", |grid_snapshot| {
        grid_snapshot.contains(GLOBAL_MARKER) && grid_snapshot.contains("$")
    });
    (zellij, global, shell)
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

/// The column a piece of text is rendered at, in characters.
fn column_of(zellij: &TestSession, needle: &str) -> usize {
    zellij
        .snapshot()
        .lines()
        .into_iter()
        .find_map(|line| {
            line.find(needle)
                .map(|byte_index| line[..byte_index].chars().count())
        })
        .unwrap_or_else(|| panic!("{needle:?} is not on screen"))
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

    warp_focused_pane_expecting_success(&sender, &receiver, false);

    let arrived = receiver.expect_adopted_pty();
    assert!(
        !arrived.is_exited(),
        "the pane's process was killed rather than handed over",
    );

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

#[test]
fn a_focused_global_pane_is_refused_and_nothing_moves() {
    // The dangerous case, end to end. Asked for "the focused pane" with a global
    // pane focused, a warp that resolved focus through the tab would answer with
    // a pane the user is not even looking at — and hand it to another session for
    // them. Refusing needs no supervisor handover, so this is the one test here
    // that would pass on a backend that cannot move panes at all.
    let (receiver, _receiver_shell) = start_with_a_shell();
    let (sender, global, _sender_shell) = start_with_a_global_pane();
    let tab_pane = split_off_a_pane(&sender, "not going anywhere");

    focus_and_confirm(&sender, &keys::alt('h'), &global);

    let (exit_code, reported) = warp_focused_pane_to(&sender, &receiver, false);
    assert_ne!(
        exit_code, 0,
        "warping a global pane should have been refused"
    );
    assert!(
        reported.contains("this is a global pane"),
        "refused, but for the wrong reason: {reported}",
    );
    assert!(
        !tab_pane.is_exited(),
        "a pane the user was not even looking at was moved",
    );
    assert!(
        !global.is_exited(),
        "the global pane itself was moved out of the session",
    );
    sender.wait_until("the sending session to be unchanged", |grid_snapshot| {
        grid_snapshot.contains("not going anywhere") && grid_snapshot.contains(GLOBAL_MARKER)
    });
    assert!(
        !receiver.snapshot().contains("not going anywhere"),
        "a pane was handed over by a warp that was supposed to be refused",
    );
}

#[test]
fn an_adopted_pane_lands_in_the_tab_not_in_the_global_strip() {
    // `NewPane` reads "wherever the client is", and for a client focused on a
    // global pane that means *inside* the global pane — a strip belonging to the
    // whole session, which a pane from another session has no business joining.
    // The unit tests assert the placement that is asked for; this asserts where
    // the pane actually ends up.
    let (receiver, global, receiver_shell) = start_with_a_global_pane();
    let (sender, _sender_shell) = start_with_a_shell();
    let _travelling = split_off_a_pane(&sender, "arriving from elsewhere");

    focus_and_confirm(&receiver, &keys::alt('h'), &global);

    warp_focused_pane_expecting_success(&sender, &receiver, false);
    let _arrived = receiver.expect_adopted_pty();
    receiver.wait_until("the warped pane to arrive", |grid_snapshot| {
        grid_snapshot.contains("arriving from elsewhere")
    });

    let landed_at = column_of(&receiver, "arriving from elsewhere");
    assert!(
        landed_at >= GLOBAL_STRIP_COLS,
        "the pane landed at column {landed_at}, inside the {GLOBAL_STRIP_COLS}-column global strip",
    );
    // The strip is still one pane wide rather than split in two, and the pane
    // that was in the tab is still in the tab beside the new arrival.
    assert_eq!(
        column_of(&receiver, GLOBAL_MARKER),
        0,
        "the global pane was moved over to make room for an arriving pane",
    );
    assert!(!receiver_shell.is_exited());
}

#[test]
fn an_adopted_floating_pane_lands_in_the_tab_not_in_the_global_strip() {
    // Floating panes escape the global strip today only because
    // `split_focused_global_pane` has no arm for them, so it is worth pinning
    // that they stay out of it. Asked of the strip rather than of the pane's own
    // column: a floating pane is drawn over the tab and may overlap the strip on
    // screen, which is a different thing from *belonging* to it.
    let (receiver, global, _receiver_shell) = start_with_a_global_pane();
    let (sender, _sender_shell) = start_with_a_shell();
    let _travelling = split_off_a_pane(&sender, "floating in from elsewhere");

    focus_and_confirm(&receiver, &keys::alt('h'), &global);

    warp_focused_pane_expecting_success(&sender, &receiver, true);
    let _arrived = receiver.expect_adopted_pty();
    receiver.wait_until("the warped pane to arrive", |grid_snapshot| {
        grid_snapshot.contains("floating in from elsewhere")
    });

    assert!(
        !global.is_exited(),
        "the global pane was taken down by a pane arriving in the tab",
    );
    assert_eq!(
        column_of(&receiver, GLOBAL_MARKER),
        0,
        "the global pane was split or moved to make room for an arriving pane",
    );
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
