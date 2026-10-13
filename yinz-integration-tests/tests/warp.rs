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
//! reaches the other exactly as `yinz action` would.
//!
//! The supervisor handover is faked (see `FakeServerOsApi::release_terminal_for_warp`):
//! the pane's pty state is moved between the two sessions' registries rather than
//! a real process being re-parented. What that still proves is the part with no
//! owner while it is in flight — that the pane leaves one session, arrives in the
//! other keeping everything it had, and is never told to exit.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    keys, FakePtyHandle, GridSnapshot, LayoutInfo, TestRunner, TestSession, WarpFailure, PROMPT,
    TERMINAL_SIZE,
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
    let yinz = start(None);
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_for_app_load();
    (yinz, shell)
}

/// Move the focus onto a pane and confirm it landed, by what the pane is sent.
///
/// Focus is not something a grid snapshot states, and for these tests it is the
/// whole subject — "the focused pane" is what a warp with no `--pane-id` moves.
/// So it is established the only way it can be observed: type something and see
/// which pane's process receives it.
fn focus_and_confirm(yinz: &TestSession, focus_key: &[u8], expected: &FakePtyHandle) {
    yinz.send_stdin(focus_key);
    type_and_confirm(yinz, expected, b"focus-probe");
}

/// Record process identity before transfer, and observe keyboard routing afterwards.
fn type_and_confirm(session: &TestSession, pane: &FakePtyHandle, marker: &[u8]) {
    session.send_stdin(marker);
    session.wait_until("input to reach the expected process", |_| {
        pane.stdin_bytes()
            .windows(marker.len())
            .any(|bytes| bytes == marker)
    });
}

fn assert_redrawn(session: &TestSession, pane: &FakePtyHandle) {
    session.wait_until(
        "adopted process to receive a redraw nudge and its final size",
        |_| {
            pane.size_history().windows(3).any(|sizes| {
                let (cols, rows) = sizes[0];
                sizes[1] == (if cols == 1 { 2 } else { cols - 1 }, rows)
                    && sizes[2] == sizes[0]
                    && pane.size() == Some(sizes[2])
            })
        },
    );
}

fn adopted_with_marker<'a>(panes: &'a [FakePtyHandle], marker: &[u8]) -> &'a FakePtyHandle {
    let matching: Vec<_> = panes
        .iter()
        .filter(|pane| {
            pane.stdin_bytes()
                .windows(marker.len())
                .any(|bytes| bytes == marker)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one process must retain {marker:?}"
    );
    matching[0]
}

fn assert_departed(session: &TestSession, panes: &[&FakePtyHandle]) {
    let (code, listed) = session.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{listed}");
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    for pane in panes {
        assert!(
            !listed.as_array().unwrap().iter().any(|item| {
                item["is_plugin"] == false && item["id"] == pane.terminal_id().get()
            }),
            "transferred pane remains in source: {listed}"
        );
    }
}

/// The pair a warp needs, **receiving session started first**.
///
/// A warp refuses to move a pane into the session it is already in, and the
/// server answers "which session am I" out of `YINZ_SESSION_NAME` — which is
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
/// The test needs to distinguish this pane from the shell it was split off,
/// at both ends of the trip.
fn split_off_a_pane(yinz: &TestSession, says: &str) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let pane = yinz.expect_pty_spawn();
    pane.output(format!("{says}\r\n").as_bytes());
    yinz.wait_until("the new pane to show its output", |grid_snapshot| {
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
    terminal_id: yinz_types::internal_ids::TerminalId,
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

fn report_waiting_agent(yinz: &TestSession, terminal_id: yinz_types::internal_ids::TerminalId) {
    report_waiting_agent_with_run_id(yinz, terminal_id, Some(AGENT_RUN_ID));
}

fn report_waiting_agent_with_run_id(
    yinz: &TestSession,
    terminal_id: yinz_types::internal_ids::TerminalId,
    run_id: Option<&str>,
) {
    let (exit_code, reported) = yinz.run_cli_action_reporting(CliAction::AgentReport {
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

fn assert_waiting_agent(
    yinz: &TestSession,
    terminal_id: Option<yinz_types::internal_ids::TerminalId>,
) {
    let (exit_code, listed) = yinz.run_cli_action_reporting(CliAction::ListPanes {
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
                && terminal_id.is_none_or(|terminal_id| pane["id"] == terminal_id.get())
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
/// The open member is the one its row is marked with a caret. Local numeric IDs
/// and padding are excluded: both can change at the destination.
fn stack_list_entries(snapshot: &GridSnapshot) -> Vec<String> {
    snapshot
        .lines()
        .into_iter()
        .filter(|line| {
            line.contains("Pane #") || (line.contains("│ [") && line.contains("EDITING SCROLLBACK"))
        })
        .map(|line| {
            let title = if let Some((_, suffix)) = line.split_once("Pane #") {
                format!("Pane #{}", suffix.trim_end_matches([' ', '│']))
            } else {
                "EDITING SCROLLBACK".to_owned()
            };
            format!("{}{title}", if line.contains('>') { "> " } else { "" })
        })
        .collect()
}

#[test]
fn an_individual_warp_to_a_new_tab_preserves_the_terminal_header() {
    let receiver = start(Some(
        r#"
        layout {
            pane size=1 borderless=true { plugin location="tab-bar"; }
            pane
            pane size=1 borderless=true { plugin location="status-bar"; }
            swap_tiled_layout name="ui" {
                tab {
                    pane size=1 borderless=true { plugin location="tab-bar"; }
                    pane split_direction="vertical" { children; }
                    pane size=1 borderless=true { plugin location="status-bar"; }
                }
            }
        }
        "#,
    ));
    receiver.expect_pty_spawn().output(PROMPT);
    receiver.wait_for_app_load();
    let (sender, _sender_shell) = start_with_a_shell();
    let travelling = split_off_a_pane(&sender, "before transfer");
    let title = "travelling terminal";
    let (code, reported) = sender.run_cli_action_reporting(CliAction::RenamePane {
        name: title.into(),
        pane_id: Some(format!("terminal_{}", travelling.terminal_id())),
    });
    assert_eq!(code, 0, "{reported}");

    let (code, reported) = sender.run_cli_action_reporting(CliAction::WarpPane {
        to_session: receiver.session_name().to_owned(),
        pane_id: Some(format!("terminal_{}", travelling.terminal_id())),
        focused_tab: false,
        floating: false,
    });
    assert_eq!(code, 0, "the warp was refused: {reported}");
    let arrived = receiver.expect_adopted_pty();
    assert_redrawn(&receiver, &arrived);
    receiver.wait_until("the terminal to arrive in its new tab", |grid| {
        grid.contains("before transfer") && grid.tab_bar_appears() && grid.status_bar_appears()
    });

    // Title-style frames show a lone terminal's title in the tab bar; a second
    // terminal makes the transferred terminal's own header visible.
    split_off_a_pane(&receiver, "beside the transferred terminal");
    receiver.wait_until("the transferred title in its pane header", |grid| {
        grid.lines().iter().skip(1).any(|line| line.contains(title))
    });
    assert!(
        !arrived.is_exited(),
        "the transfer killed the terminal process"
    );
    arrived.output(b"still writing after transfer\r\n");
    receiver.wait_until(
        "the transferred terminal to keep producing output",
        |grid| {
            grid.contains("still writing after transfer")
                && grid.lines().iter().skip(1).any(|line| line.contains(title))
        },
    );
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
    assert_redrawn(&receiver, &arrived);
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
fn split_off_a_remote_pane(yinz: &TestSession, says: &str) -> FakePtyHandle {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["cargo".to_string(), "watch".to_string()],
        remote_host: Some("build-box".to_string()),

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
        configuration_json: None,
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
    let pane = yinz.expect_pty_spawn();
    pane.output(format!("{says}\r\n").as_bytes());
    yinz.wait_until("the remote pane to show its output", |grid_snapshot| {
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
    assert_redrawn(&receiver, &arrived);
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
        Some(yinz_types::input::command::TerminalAction::RunCommand(run_command)) => run_command,
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

    let original_sizes = [_sender_shell.size(), local.size(), remote.size()];
    let (exit_code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_id: None,
    });
    assert_eq!(exit_code, 0, "the tab warp was refused: {reported}");

    receiver.wait_until("both panes to arrive", |grid_snapshot| {
        grid_snapshot.contains("local pane") && grid_snapshot.contains("remote pane")
    });
    sender.wait_until("both panes to leave", |grid_snapshot| {
        !grid_snapshot.contains("local pane") && !grid_snapshot.contains("remote pane")
    });
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    for pane in &adopted {
        assert_redrawn(&receiver, pane);
    }
    let mut arrived_sizes: Vec<_> = adopted.iter().map(FakePtyHandle::size).collect();
    let mut original_sizes = original_sizes.to_vec();
    arrived_sizes.sort();
    original_sizes.sort();
    assert_eq!(
        arrived_sizes, original_sizes,
        "same-size tab adoption must still redraw"
    );
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

/// Silent hook records must survive both the handoff and the old server's teardown.
#[test]
fn two_idle_agents_keep_their_records_after_a_whole_tab_warp_and_source_shutdown() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    use yinz_types::input::actions::Action;
    use yinz_types::pane_id::PaneId;

    let ((mut sender, _source_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    // Leave room for the idle count beside the status bar's key hints.
    receiver.resize(yinz_types::pane_size::Size {
        rows: 24,
        cols: 200,
    });
    sender.send_stdin(&keys::ctrl('t'));
    sender.send_stdin(&keys::key('n'));
    let shell = sender.expect_pty_spawn();
    shell.output(b"first idle agent\r\n");
    sender.wait_until("the travelling tab to open", |grid| {
        grid.contains("first idle agent")
    });
    let second = split_off_a_pane(&sender, "second idle agent");
    let records: Vec<_> = ["codex", "claude"]
        .into_iter()
        .map(|kind| AgentInfo {
            kind: kind.to_owned(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: Some(format!("{kind} finished")),
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: Some(format!("/work/{kind}")),
            session_id: Some(format!("{kind}-session")),
            run_id: Some(format!("{kind}:transferred-run")),
        })
        .collect();
    for (pane, record) in [&shell, &second].into_iter().zip(&records) {
        let (code, output) = sender.run_actions_reporting(vec![Action::SetPaneAgent {
            pane_id: PaneId::Terminal(pane.terminal_id()),
            agent: Some(record.clone()),
        }]);
        assert_eq!(code, 0, "{output}");
    }
    let list = |session: &TestSession| -> Vec<serde_json::Value> {
        let (code, output) = session.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        serde_json::from_str(&output).unwrap()
    };
    let assert_records = || {
        let panes = list(&receiver);
        records
            .iter()
            .map(|record| {
                let matching: Vec<_> = panes
                    .iter()
                    .filter(|pane| pane["agent"]["run_id"] == serde_json::json!(record.run_id))
                    .collect();
                assert_eq!(matching.len(), 1, "missing or duplicated record: {panes:?}");
                assert_eq!(matching[0]["agent"], serde_json::to_value(record).unwrap());
                matching[0]["id"].as_u64().unwrap() as u32
            })
            .collect::<Vec<_>>()
    };
    let (code, output) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_id: None,
    });
    assert_eq!(code, 0, "{output}");
    receiver.wait_until("the transferred tab to arrive", |grid| {
        grid.contains("second idle agent")
    });
    let destination_ids = assert_records();
    receiver.wait_until("the status bar to count both idle agents", |grid| {
        grid.contains("·2") || grid.contains("2i")
    });
    sender.quit();
    // Each CLI client attaches and detaches, causing fresh manifest publications. No hook
    // or timer is needed to keep the records alive, even after their old owner has gone.
    for _ in 0..3 {
        assert_eq!(assert_records(), destination_ids);
    }
    receiver.wait_until(
        "both idle agents to remain counted after source shutdown",
        |grid| grid.contains("·2") || grid.contains("2i"),
    );
    let mut busy = records[0].clone();
    busy.state = AgentState::Busy;
    let (code, output) = receiver.run_actions_reporting(vec![Action::SetPaneAgent {
        pane_id: PaneId::from_legacy(destination_ids[0], false),
        agent: Some(busy.clone()),
    }]);
    assert_eq!(code, 0, "{output}");
    assert!(list(&receiver)
        .iter()
        .any(|pane| pane["agent"] == serde_json::to_value(&busy).unwrap()));
    let event =
        serde_json::from_str(r#"{"hook_event_name":"SessionEnd","session_id":"codex-session"}"#)
            .unwrap();
    let (code, output) =
        receiver.run_actions_reporting(yinz_types::input::actions::hook_report_actions(
            "codex",
            PaneId::from_legacy(destination_ids[0], false),
            &event,
            String::new(),
        ));
    assert_eq!(code, 0, "{output}");
    let panes = list(&receiver);
    assert!(panes
        .iter()
        .find(|pane| pane["id"] == destination_ids[0] && pane["is_plugin"] == false)
        .unwrap()["agent"]
        .is_null());
    assert!(panes
        .iter()
        .any(|pane| pane["agent"] == serde_json::to_value(&records[1]).unwrap()));
}

#[test]
fn the_sessions_last_pane_warps_and_the_source_session_closes() {
    let ((mut sender, sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    focus_and_confirm(&sender, &keys::alt('h'), &sender_shell);

    warp_focused_pane_expecting_success(&sender, &receiver, false);
    let arrived = receiver.expect_adopted_pty();
    sender.wait_for_exit();

    assert!(
        !arrived.is_exited(),
        "source shutdown killed the warped pane"
    );
    type_and_confirm(&receiver, &arrived, b"after-warp");
    arrived.output(b"still running after source shutdown\r\n");
    receiver.wait_until("the transferred process to keep writing", |grid| {
        grid.contains("still running after source shutdown")
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
        tab_id: None,
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
fn warping_a_visible_editor_carries_its_subject_and_restores_it_on_close() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let being_edited = split_off_a_pane(&sender, "SCROLLBACK_MARKER");
    type_and_confirm(&sender, &being_edited, b"subject-before");
    sender.send_stdin(&keys::ctrl('s'));
    sender.send_stdin(&keys::key('e'));
    let editor = sender.expect_pty_spawn();
    sender.wait_until("the scrollback editor to open", |grid_snapshot| {
        grid_snapshot.contains("EDITING SCROLLBACK") && grid_snapshot.contains("SCROLLBACK_MARKER")
    });
    let (exit_code, reported) = warp_pane_by_id(&sender, &receiver, being_edited.terminal_id());
    assert_ne!(exit_code, 0, "hidden selection must be refused");
    assert!(reported.contains("visible"), "{reported}");
    assert!(!being_edited.is_exited() && !editor.is_exited());
    type_and_confirm(&sender, &editor, b"editor-before");
    let (exit_code, reported) = warp_pane_by_id(&sender, &receiver, editor.terminal_id());
    assert_eq!(exit_code, 0, "the warp was refused: {reported}");
    let adopted = [receiver.expect_adopted_pty(), receiver.expect_adopted_pty()];
    for pane in &adopted {
        assert_redrawn(&receiver, pane);
    }
    let arrived_editor = adopted_with_marker(&adopted, b"editor-before");
    let arrived_subject = adopted_with_marker(&adopted, b"subject-before");
    assert_departed(&sender, &[&editor, &being_edited]);
    receiver.wait_until("the editor to arrive", |grid| {
        grid.contains("EDITING SCROLLBACK")
    });
    assert!(!arrived_editor.is_exited() && !arrived_subject.is_exited());
    receiver.send_stdin(&keys::alt('l'));
    type_and_confirm(&receiver, arrived_editor, b"editor-focused-after");
    arrived_editor.exit(Some(0));
    arrived_subject.output(b"edited and then moved\r\n");
    receiver.wait_until("closing the editor restores its running subject", |grid| {
        grid.contains("edited and then moved") && !grid.contains("EDITING SCROLLBACK")
    });
    type_and_confirm(&receiver, arrived_subject, b"restored-input");
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
    type_and_confirm(&sender, &first, b"tab-first-before");

    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let second = sender.expect_pty_spawn();
    second.output(b"second-member\r\n$ ");
    sender.wait_until("second member ready", |grid| grid.contains("second-member"));
    type_and_confirm(&sender, &second, b"tab-second-before");
    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let third = sender.expect_pty_spawn();
    third.output(b"third-member\r\n$ ");
    sender.wait_until("the stack to be built", |grid_snapshot| {
        grid_snapshot.contains("third-member")
    });

    type_and_confirm(&sender, &third, b"tab-third-before");
    let (exit_code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_id: None,
    });
    assert_eq!(exit_code, 0, "the tab warp was refused: {reported}");

    // Every member travels, hidden ones included: three panes were in the tab and
    // three supervisors have to be taken over, or one is stranded.
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    let _first = adopted_with_marker(&adopted, b"tab-first-before");
    let second = adopted_with_marker(&adopted, b"tab-second-before");
    let third = adopted_with_marker(&adopted, b"tab-third-before");
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
    type_and_confirm(&receiver, third, b"tab-selected-after");
    let entries = stack_list_entries(&receiver.snapshot());
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
    receiver.send_stdin(&keys::alt('k'));
    receiver.wait_until("select the hidden second member", |grid| {
        grid.contains("second-member")
    });
    type_and_confirm(&receiver, second, b"tab-hidden-after");
}

#[test]
fn warp_adoption_failure_returns_all_editor_companions_and_their_relationships() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let subject = split_off_a_pane(&sender, "ROLLBACK_SUBJECT");
    roll_back_a_pane_with_two_editors(&sender, &receiver, &subject);
}

#[test]
fn warp_adoption_failure_returns_the_sessions_only_pane_and_its_editors() {
    let ((sender, subject), (receiver, _receiver_shell)) = sender_and_receiver();
    focus_and_confirm(&sender, &keys::alt('h'), &subject);
    roll_back_a_pane_with_two_editors(&sender, &receiver, &subject);
}

/// Open two scrollback editors on the focused `subject`, fail its warp, and check
/// that all three come back with the editors still covering it in order.
fn roll_back_a_pane_with_two_editors(
    sender: &TestSession,
    receiver: &TestSession,
    subject: &FakePtyHandle,
) {
    type_and_confirm(sender, subject, b"rollback-subject-before");
    for index in 0..2 {
        sender.send_stdin(&keys::ctrl('s'));
        sender.send_stdin(&keys::key('e'));
        let editor = sender.expect_pty_spawn();
        let marker = format!("EDITOR_READY_{index}");
        editor.output(format!("{marker}\r\n").as_bytes());
        sender.wait_until("editor to open", |grid| grid.contains(&marker));
        type_and_confirm(sender, &editor, marker.as_bytes());
    }
    subject.fail_warp(WarpFailure::UnadoptableOnce);
    let (exit_code, reported) = warp_focused_pane_to(sender, receiver, false);
    assert_ne!(exit_code, 0);
    assert!(reported.contains("back in this session"), "{reported}");
    let adopted: Vec<_> = (0..3).map(|_| sender.expect_adopted_pty()).collect();
    let newest_editor = adopted_with_marker(&adopted, b"EDITOR_READY_1");
    let older_editor = adopted_with_marker(&adopted, b"EDITOR_READY_0");
    let restored_subject = adopted_with_marker(&adopted, b"rollback-subject-before");
    sender.wait_until("rollback editor to appear", |grid| {
        grid.contains("EDITING SCROLLBACK")
    });
    newest_editor.exit(Some(0));
    older_editor.output(b"OLDER_EDITOR_RESTORED\r\n");
    sender.wait_until("first cover to close", |grid| {
        grid.contains("OLDER_EDITOR_RESTORED")
    });
    older_editor.exit(Some(0));
    restored_subject.output(b"SUBJECT_RESTORED\r\n");
    sender.wait_until("second cover to close", |grid| {
        grid.contains("SUBJECT_RESTORED")
    });
    type_and_confirm(sender, restored_subject, b"rollback-restored-input");
    assert!(!restored_subject.is_exited());
}

#[test]
fn a_visible_stack_warps_to_the_floating_layer_with_selectable_members() {
    let ((sender, _sender_shell), (receiver, _receiver_shell)) = sender_and_receiver();
    let first = split_off_a_pane(&sender, "FIRST_FLOATING_MEMBER");
    type_and_confirm(&sender, &first, b"first-before");
    let mut source = vec![first];
    for marker in ["SECOND_FLOATING_MEMBER", "THIRD_FLOATING_MEMBER"] {
        sender.send_stdin(&keys::ctrl('p'));
        sender.send_stdin(&keys::key('s'));
        let pane = sender.expect_pty_spawn();
        pane.output(format!("{marker}\r\n").as_bytes());
        sender.wait_until("stack member ready", |grid| grid.contains(marker));
        type_and_confirm(&sender, &pane, marker.as_bytes());
        source.push(pane);
    }
    warp_focused_pane_expecting_success(&sender, &receiver, true);
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    for pane in &adopted {
        assert_redrawn(&receiver, pane);
    }
    let first = adopted_with_marker(&adopted, b"first-before");
    let second = adopted_with_marker(&adopted, b"SECOND_FLOATING_MEMBER");
    let third = adopted_with_marker(&adopted, b"THIRD_FLOATING_MEMBER");
    assert_departed(&sender, &source.iter().collect::<Vec<_>>());
    receiver.wait_until("floating stack arrives", |grid| {
        grid.contains("THIRD_FLOATING_MEMBER")
    });
    let (code, listed) = receiver.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{listed}");
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let selected = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["is_plugin"] == false && pane["id"] == third.terminal_id().get())
        .expect("selected stack member in manifest");
    assert_eq!(selected["is_floating"], true, "{selected}");
    // Hiding the floating layer must hide the transferred stack too.
    receiver.send_stdin(&keys::ctrl('p'));
    receiver.send_stdin(&keys::key('w'));
    receiver.wait_until("floating stack hides", |grid| {
        !grid.contains("THIRD_FLOATING_MEMBER") && !grid.contains("SECOND_FLOATING_MEMBER")
    });
    receiver.send_stdin(&keys::ctrl('p'));
    receiver.send_stdin(&keys::key('w'));
    receiver.wait_until("floating stack shows", |grid| {
        grid.contains("THIRD_FLOATING_MEMBER")
    });
    type_and_confirm(&receiver, third, b"selected-after");
    receiver.send_stdin(&keys::alt('k'));
    receiver.wait_until("select hidden floating member", |grid| {
        grid.contains("SECOND_FLOATING_MEMBER")
    });
    assert_eq!(stack_list_entries(&receiver.snapshot()).len(), 3);
    type_and_confirm(&receiver, second, b"hidden-after");
    second.exit(Some(0));
    receiver.wait_until("closing selected member promotes a survivor", |grid| {
        grid.contains("THIRD_FLOATING_MEMBER")
    });
    assert!(!first.is_exited() && !third.is_exited());
}

#[test]
fn a_stack_with_a_hidden_editor_restores_after_id_remapping_and_source_shutdown() {
    let (receiver, _) = start_with_a_shell();
    // Allocate destination IDs before starting the source (session names are process-global).
    let _ = split_off_a_pane(&receiver, "destination filler");
    let _ = split_off_a_pane(&receiver, "another filler");
    let (mut sender, _) = start_with_a_shell();
    sender.send_stdin(&keys::ctrl('t'));
    sender.send_stdin(&keys::key('n'));
    let first = sender.expect_pty_spawn();
    first.output(b"COMBINED_FIRST\r\n");
    sender.wait_until("travelling tab opens", |grid| {
        grid.contains("COMBINED_FIRST")
    });
    type_and_confirm(&sender, &first, b"combined-first-before");
    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let subject = sender.expect_pty_spawn();
    subject.output(b"COMBINED_SUBJECT\r\n");
    sender.wait_until("subject opens", |grid| grid.contains("COMBINED_SUBJECT"));
    type_and_confirm(&sender, &subject, b"combined-subject-before");
    sender.send_stdin(&keys::ctrl('s'));
    sender.send_stdin(&keys::key('e'));
    let editor = sender.expect_pty_spawn();
    sender.wait_until("editor opens", |grid| grid.contains("EDITING SCROLLBACK"));
    type_and_confirm(&sender, &editor, b"combined-editor-before");
    sender.send_stdin(&keys::alt('k'));
    sender.wait_until("select first and hide editor", |grid| {
        grid.contains("COMBINED_FIRST")
    });
    type_and_confirm(&sender, &first, b"first-selected-before");
    let entries = stack_list_entries(&sender.snapshot());
    assert_eq!(entries, vec!["> Pane #1", "EDITING SCROLLBACK"]);

    let (code, reported) = sender.run_cli_action_reporting(CliAction::WarpTab {
        to_session: Some(receiver.session_name().to_owned()),
        from_session: None,
        tab_id: None,
    });
    assert_eq!(code, 0, "{reported}");
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    let arrived_first = adopted_with_marker(&adopted, b"combined-first-before");
    let arrived_subject = adopted_with_marker(&adopted, b"combined-subject-before");
    let arrived_editor = adopted_with_marker(&adopted, b"combined-editor-before");
    for (before, after) in [
        (&first, arrived_first),
        (&subject, arrived_subject),
        (&editor, arrived_editor),
    ] {
        assert_ne!(before.terminal_id(), after.terminal_id());
    }
    assert_departed(&sender, &[&first, &subject, &editor]);
    receiver.wait_until("selected member arrives", |grid| {
        grid.contains("COMBINED_FIRST")
    });
    assert_eq!(stack_list_entries(&receiver.snapshot()), entries);
    type_and_confirm(&receiver, arrived_first, b"first-selected-after");
    sender.quit();
    receiver.send_stdin(&keys::alt('j'));
    receiver.wait_until("select transferred editor", |grid| {
        grid.contains("EDITING SCROLLBACK")
    });
    type_and_confirm(&receiver, arrived_editor, b"editor-selected-after");
    arrived_editor.exit(Some(0));
    receiver.wait_until("editor restores its own subject", |grid| {
        grid.contains("COMBINED_SUBJECT") && !grid.contains("EDITING SCROLLBACK")
    });
    type_and_confirm(&receiver, arrived_subject, b"subject-restored-after");
    arrived_subject.output(b"LIVE_AFTER_SOURCE_SHUTDOWN\r\n");
    receiver.wait_until("live output after source shutdown", |grid| {
        grid.contains("LIVE_AFTER_SOURCE_SHUTDOWN")
    });
    assert!(!arrived_first.is_exited() && !arrived_subject.is_exited());
}

#[test]
fn held_warp_preserves_state_until_explicit_restart() {
    for first_run in [true, false] {
        for mode in ["pane", "tab", "rollback"] {
            let whole_tab = mode != "pane";
            let (receiver, _receiver_shell) = start_with_a_shell();
            let layout = format!(
                r#"layout {{
                tab focus=true {{
                pane
                pane name="saved held task" command="codex" cwd="/tmp" start_suspended={first_run}
                pane size=1 borderless=true {{ plugin location="yinz:status-bar"; }}
                }}
                tab {{ pane; }}
            }}"#
            );
            let sender = start(Some(&layout));
            let live = sender.expect_pty_spawn();
            live.output(PROMPT);
            sender.wait_until("custom layout to load", |grid| {
                grid.contains("saved held task")
            });
            let list = |session: &TestSession| {
                let (code, output) = session.run_cli_action_reporting(CliAction::ListPanes {
                    tab: false,
                    command: false,
                    state: true,
                    geometry: true,
                    all: true,
                    json: true,
                });
                assert_eq!(code, 0, "{output}");
                serde_json::from_str::<Vec<serde_json::Value>>(&output).unwrap()
            };
            if !first_run {
                let terminal = sender.expect_pty_spawn();
                terminal.output(b"KEPT SCROLLBACK\r\n");
                sender.wait_until("command output before exit", |grid| {
                    grid.contains("KEPT SCROLLBACK")
                });
                terminal.exit(Some(17));
            }
            let spare = sender.expect_pty_spawn();
            spare.output(PROMPT);
            sender.wait_until("held command ready to transfer", |_| {
                list(&sender)
                    .iter()
                    .any(|pane| pane["title"] == "saved held task" && pane["is_held"] == true)
            });
            let before = list(&sender)
                .into_iter()
                .find(|pane| pane["title"] == "saved held task")
                .unwrap();
            let id =
                yinz_types::internal_ids::TerminalId::new(before["id"].as_u64().unwrap() as u32);
            report_waiting_agent(&sender, id);
            let before = list(&sender)
                .into_iter()
                .find(|pane| pane["title"] == "saved held task")
                .unwrap();
            if mode == "rollback" {
                live.fail_warp(WarpFailure::UnadoptableOnce);
            }
            let (code, output) = if whole_tab {
                sender.run_cli_action_reporting(CliAction::WarpTab {
                    to_session: Some(receiver.session_name().to_owned()),
                    from_session: None,
                    tab_id: Some(yinz_types::internal_ids::TabId::new(0)),
                })
            } else {
                warp_pane_by_id(&sender, &receiver, id)
            };
            if mode == "rollback" {
                assert_ne!(code, 0, "{output}");
                assert!(output.contains("back in this session"), "{output}");
            } else {
                assert_eq!(code, 0, "{mode}, first_run={first_run}: {output}");
            }
            let receiver = if mode == "rollback" {
                &sender
            } else {
                &receiver
            };
            receiver.wait_until("held command to arrive", |_| {
                list(receiver)
                    .iter()
                    .any(|pane| pane["title"] == "saved held task" && pane["is_held"] == true)
            });
            let arrived = list(receiver)
                .into_iter()
                .find(|pane| pane["title"] == "saved held task")
                .unwrap();
            for field in ["is_held", "exited", "exit_status", "agent"] {
                assert_eq!(arrived[field], before[field], "{field}: {arrived}");
            }
            if !first_run {
                receiver.wait_until("held scrollback to arrive", |grid| {
                    grid.contains("KEPT SCROLLBACK")
                });
            }
            if whole_tab {
                let adopted = receiver.expect_adopted_pty();
                assert!(!adopted.is_exited());
            }
            let focused = receiver.attach_client_with_focus(
                TERMINAL_SIZE,
                Some(yinz_types::data::PaneId::Terminal(
                    yinz_types::internal_ids::TerminalId::new(
                        arrived["id"].as_u64().unwrap() as u32
                    ),
                )),
            );
            focused.wait_until("held pane focused for explicit restart", |grid| {
                grid.contains("saved held task")
            });
            focused.send_stdin(b"\r");
            let started = receiver.expect_pty_spawn();
            started.wait_for_command("codex");
            assert_eq!(started.run_command().cwd, Some("/tmp".into()));
            started.output(b"EXPLICIT RESTART\r\n");
            focused.wait_until("explicit restart output", |grid| {
                grid.contains("EXPLICIT RESTART")
            });
        }
    }
}

#[test]
fn adopted_local_and_remote_panes_redraw_in_each_destination_layer() {
    for remote in [false, true] {
        for placement in ["new tab", "focused tab", "floating"] {
            let receiver = TestRunner::new(yinz_types::pane_size::Size {
                cols: 140,
                rows: 40,
            })
            .with_config(NO_BACKGROUND_PLUGINS)
            .skip_concurrency_slot()
            .start();
            receiver.expect_pty_spawn().output(PROMPT);
            receiver.wait_for_app_load();
            let (sender, _shell) = start_with_a_shell();
            let travelling = if remote {
                split_off_a_remote_pane(&sender, "preparing transfer")
            } else {
                split_off_a_pane(&sender, "preparing transfer")
            };
            // Keep the marker at the bottom so a shorter floating destination
            // retains it in the viewport instead of moving it into scrollback.
            travelling.output(b"\x1b[2J\x1b[999;1HREDRAW_TRANSFER");
            sender.wait_until("source contents ready", |grid| {
                grid.contains("REDRAW_TRANSFER")
            });
            let old_size = travelling.size();
            let (code, reported) = sender.run_cli_action_reporting(CliAction::WarpPane {
                to_session: receiver.session_name().to_owned(),
                pane_id: Some(format!("terminal_{}", travelling.terminal_id())),
                focused_tab: placement == "focused tab",
                floating: placement == "floating",
            });
            assert_eq!(code, 0, "{reported}");
            let arrived = receiver.expect_adopted_pty();
            assert_redrawn(&receiver, &arrived);
            assert_ne!(
                arrived.size(),
                old_size,
                "different-size destination: {placement}"
            );
            receiver.wait_until("transferred contents to render", |grid| {
                grid.contains("REDRAW_TRANSFER")
            });
            arrived.output(b"\r\nREDRAW_LIVE_OUTPUT\r\n");
            receiver.wait_until("live contents to render", |grid| {
                grid.contains("REDRAW_LIVE_OUTPUT")
            });
        }
    }
}

fn palette_sessions() -> ((TestSession, FakePtyHandle), (TestSession, FakePtyHandle)) {
    let start = || {
        let session = TestRunner::new(TERMINAL_SIZE)
            .with_config(&format!(
                "{NO_BACKGROUND_PLUGINS}disable_session_metadata false"
            ))
            .skip_concurrency_slot()
            .start();
        let shell = session.expect_pty_spawn();
        shell.output(PROMPT);
        session.wait_for_app_load();
        (session, shell)
    };
    let receiver = start();
    let sender = start();
    (sender, receiver)
}

fn palette_pull(receiver: &TestSession, sender: &TestSession, command: &str, target: &str) {
    receiver.send_stdin(b"\x1b[112;7u");
    receiver.wait_until("palette opens", |grid| {
        grid.contains("Switch current project")
    });
    receiver.send_stdin(command.as_bytes());
    receiver.send_stdin(&keys::ENTER);
    receiver.wait_until("source picker opens", |grid| {
        grid.contains("Source session")
    });
    receiver.send_stdin(sender.session_name().as_bytes());
    receiver.wait_until("source discovered", |grid| {
        grid.contains(&format!("› {}", sender.session_name()))
    });
    receiver.send_stdin(&keys::ENTER);
    receiver.send_stdin(target.as_bytes());
    receiver.wait_until("warp target discovered", |grid| {
        !grid.contains("Source session") && grid.contains(target) && !grid.contains("No matches")
    });
    receiver.send_stdin(&keys::ENTER);
}

#[test]
fn palette_pulls_one_tab_and_all_tabs_through_source_shutdown() {
    for all in [false, true] {
        let ((mut sender, first), (receiver, _)) = palette_sessions();
        type_and_confirm(&sender, &first, b"first-before-palette");
        sender.send_stdin(&keys::ctrl('t'));
        sender.send_stdin(&keys::key('n'));
        let second = sender.expect_pty_spawn();
        second.output(b"PALETTE_SECOND_TAB\r\n");
        sender.wait_until("second tab ready", |grid| {
            grid.contains("PALETTE_SECOND_TAB")
        });
        type_and_confirm(&sender, &second, b"second-before-palette");
        palette_pull(
            &receiver,
            &sender,
            "Warp tab",
            if all { "All tabs" } else { "Tab #2" },
        );
        let adopted: Vec<_> = (0..if all { 2 } else { 1 })
            .map(|_| receiver.expect_adopted_pty())
            .collect();
        let second = adopted_with_marker(&adopted, b"second-before-palette");
        receiver.wait_until("last pulled tab focused", |grid| {
            grid.contains("PALETTE_SECOND_TAB")
        });
        type_and_confirm(&receiver, second, b"second-after-palette");
        if all {
            let first = adopted_with_marker(&adopted, b"first-before-palette");
            assert!(!first.is_exited());
            sender.wait_for_exit();
        }
        assert!(!second.is_exited());
    }
}

#[test]
fn palette_pulls_a_visible_stack_with_its_hidden_editor_subject() {
    let ((sender, _), (receiver, _)) = palette_sessions();
    let first = split_off_a_pane(&sender, "PALETTE_STACK_FIRST");
    type_and_confirm(&sender, &first, b"stack-first-before");
    sender.send_stdin(&keys::ctrl('p'));
    sender.send_stdin(&keys::key('s'));
    let subject = sender.expect_pty_spawn();
    subject.output(b"PALETTE_STACK_SUBJECT\r\n");
    sender.wait_until("stack subject ready", |grid| {
        grid.contains("PALETTE_STACK_SUBJECT")
    });
    type_and_confirm(&sender, &subject, b"stack-subject-before");
    sender.send_stdin(&keys::ctrl('s'));
    sender.send_stdin(&keys::key('e'));
    let editor = sender.expect_pty_spawn();
    sender.wait_until("editor ready", |grid| grid.contains("EDITING SCROLLBACK"));
    type_and_confirm(&sender, &editor, b"stack-editor-before");
    let entries = stack_list_entries(&sender.snapshot());
    palette_pull(&receiver, &sender, "Warp pane", "EDITING SCROLLBACK");
    let adopted: Vec<_> = (0..3).map(|_| receiver.expect_adopted_pty()).collect();
    let first = adopted_with_marker(&adopted, b"stack-first-before");
    let subject = adopted_with_marker(&adopted, b"stack-subject-before");
    let editor = adopted_with_marker(&adopted, b"stack-editor-before");
    let arrived = receiver.wait_until("editor and its stack list arrive", |grid| {
        let rendered = stack_list_entries(grid);
        rendered.len() == entries.len()
            && rendered
                .last()
                .is_some_and(|entry| entry == "> EDITING SCROLLBACK")
    });
    let arrived_entries = stack_list_entries(&arrived);
    assert_eq!(arrived_entries.len(), entries.len());
    assert_eq!(arrived_entries.last().unwrap(), "> EDITING SCROLLBACK");
    type_and_confirm(&receiver, editor, b"stack-editor-after");
    editor.exit(Some(0));
    receiver.wait_until("hidden subject restored", |grid| {
        grid.contains("PALETTE_STACK_SUBJECT")
    });
    type_and_confirm(&receiver, subject, b"stack-subject-after");
    receiver.send_stdin(&keys::alt('k'));
    receiver.wait_until("hidden stack member restored", |grid| {
        grid.contains("PALETTE_STACK_FIRST")
    });
    type_and_confirm(&receiver, first, b"stack-first-after");
}

#[test]
fn palette_all_tabs_failure_retains_completed_and_unattempted_tabs() {
    let ((sender, first), (receiver, _)) = palette_sessions();
    type_and_confirm(&sender, &first, b"batch-first-before");
    let mut panes = Vec::new();
    for marker in ["BATCH_FAILING", "BATCH_UNATTEMPTED"] {
        sender.send_stdin(&keys::ctrl('t'));
        sender.send_stdin(&keys::key('n'));
        let pane = sender.expect_pty_spawn();
        pane.output(format!("{marker}\r\n").as_bytes());
        sender.wait_until("batch tab ready", |grid| grid.contains(marker));
        panes.push(pane);
    }
    panes[0].fail_warp(WarpFailure::UnadoptableOnce);
    palette_pull(&receiver, &sender, "Warp tab", "All tabs");
    let arrived = receiver.expect_adopted_pty();
    assert!(arrived
        .stdin_bytes()
        .windows(b"batch-first-before".len())
        .any(|bytes| bytes == b"batch-first-before"));
    receiver.wait_until("batch failure reported", |grid| {
        grid.contains("1/3 completed")
    });
    assert_departed(&sender, &[&first]);
    let (code, listed) = sender.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{listed}");
    let listed: Vec<serde_json::Value> = serde_json::from_str(&listed).unwrap();
    assert_eq!(
        listed
            .iter()
            .filter(|pane| pane["is_plugin"] == false)
            .count(),
        2
    );
    assert!(listed
        .iter()
        .any(|pane| pane["id"] == panes[1].terminal_id().get()));
    receiver.send_stdin(&keys::ESC);
    type_and_confirm(&receiver, &arrived, b"batch-first-after");
    assert!(!panes[1].is_exited());
}

fn agent_connection(
    session: &TestSession,
) -> (
    yinz_ipc::ipc::IpcSenderWithContext<yinz_ipc::ipc::ClientToServerMsg>,
    yinz_ipc::ipc::IpcReceiverWithContext<yinz_ipc::ipc::ServerToClientMsg>,
) {
    let socket = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(session.session_name()),
    )
    .unwrap();
    let sender = yinz_ipc::ipc::IpcSenderWithContext::new(socket);
    let receiver = sender.get_receiver().unwrap();
    receiver
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    (sender, receiver)
}

fn agent_response(
    receiver: &mut yinz_ipc::ipc::IpcReceiverWithContext<yinz_ipc::ipc::ServerToClientMsg>,
) -> yinz_types::agent::control::AgentResponse {
    use yinz_ipc::ipc::{IpcRecv, ServerToClientMsg};
    loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => return response,
            IpcRecv::Msg(_, _) => continue,
            _ => panic!("agent response disconnected or timed out"),
        }
    }
}

fn codex_report(session: &TestSession, pane: &FakePtyHandle, name: &str, turn: &str, prompt: &str) {
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, ServerToClientMsg};
    use yinz_types::agent::{
        hook_outcome,
        report::{AgentReport, AgentReportState},
        AgentEvent, HookEvent, HookOutcome,
    };
    let hook = HookEvent {
        hook_event_name: name.into(),
        session_id: "warp-conversation".into(),
        source: (name == "SessionStart").then(|| "startup".into()),
        turn_id: Some(turn.into()),
        prompt: (name == "UserPromptSubmit").then(|| prompt.into()),
        ..Default::default()
    };
    let trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
    let (mut sender, mut receiver) = agent_connection(session);
    sender
        .send_client_msg(ClientToServerMsg::AgentReport(AgentReport {
            report_id: trace.report_id,
            pane_id: yinz_types::pane_id::PaneId::Terminal(pane.terminal_id()),
            state: match hook_outcome("codex", &hook) {
                HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                HookOutcome::Ended => AgentReportState::Clear,
                HookOutcome::Nothing => AgentReportState::Unchanged,
            },
            event: Some(AgentEvent::from_hook(
                "codex",
                pane.terminal_id(),
                &hook,
                serde_json::to_string(&hook).unwrap(),
            )),
            trace: false,
            process_id: trace.process_id,
            kind: "codex".into(),
        }))
        .unwrap();
    assert!(matches!(
        receiver.recv_server_msg(),
        IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
    ));
}

#[test]
fn codex_control_survives_pane_tab_and_failed_adoption_without_reinitialization() {
    use yinz_ipc::ipc::ClientToServerMsg;
    use yinz_types::agent::{
        control::{AgentOutcome, AgentPlacement, AgentRequest},
        AgentSpawnSpec,
    };
    use yinz_types::pane_id::PaneId;
    for (whole_tab, enabled, rollback) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, true, false),
        (false, true, true),
        (true, true, true),
    ] {
        let (receiver, _receiver_shell) = start_with_a_shell();
        let sender =
            start(whole_tab.then_some("layout { tab focus=true { pane; }; tab { pane; }; }"));
        sender.expect_pty_spawn().output(PROMPT);
        if whole_tab {
            sender.expect_pty_spawn().output(PROMPT);
        }
        sender.wait_until("source shell is rendered", |grid| grid.contains("$"));
        let (mut control, mut replies) = agent_connection(&sender);
        let cwd = tempfile::tempdir().unwrap();
        control
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Spawn {
                placement: AgentPlacement::Tab(yinz_types::internal_ids::TabId::new(0)),
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    allow_outside_worktree: Some(true),
                    ..Default::default()
                },
                cwd: Some(cwd.path().into()),
                project: None,
                remote_host: None,
            }))
            .unwrap();
        let spawned = agent_response(&mut replies);
        assert_eq!(spawned.outcome, AgentOutcome::Ok, "{spawned:?}");
        let travelling = sender.expect_pty_spawn();
        let command = travelling.run_command();
        let pane_id = PaneId::Terminal(travelling.terminal_id());
        sender.run_cli_action(CliAction::FocusPaneId {
            pane_id: pane_id.to_string(),
        });
        travelling.output(if enabled {
            b"\x1b[?2004hCodex working"
        } else {
            b"\x1b[?2004lCodex working"
        });
        sender.wait_until("Codex mode output is parsed", |grid| {
            grid.contains("Codex working")
        });
        codex_report(&sender, &travelling, "SessionStart", "", "");
        control
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
                pane_id,
                expected_session_id: Some("warp-conversation".into()),
            }))
            .unwrap();
        assert_eq!(agent_response(&mut replies).agents[0].ready, enabled);
        if enabled {
            control
                .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id,
                    expected_session_id: Some("warp-conversation".into()),
                    prompt: "review".into(),
                    wait: false,
                    timeout_ms: 30_000,
                }))
                .unwrap();
            travelling.wait_for_stdin("first prompt is delivered", |bytes| {
                bytes
                    .windows(b"\x1b[200~review\x1b[201~\r".len())
                    .any(|bytes| bytes == b"\x1b[200~review\x1b[201~\r")
            });
            codex_report(&sender, &travelling, "UserPromptSubmit", "turn-1", "review");
            assert_eq!(
                agent_response(&mut replies).outcome,
                AgentOutcome::Submitted
            );
        }
        if rollback {
            travelling.fail_warp(WarpFailure::UnadoptableOnce);
        }
        let (code, message) = if whole_tab {
            sender.run_cli_action_reporting(CliAction::WarpTab {
                to_session: Some(receiver.session_name().into()),
                from_session: None,
                tab_id: spawned.spawned.unwrap().tab_id,
            })
        } else {
            warp_pane_by_id(&sender, &receiver, travelling.terminal_id())
        };
        assert_eq!(code == 0, !rollback, "{message}");
        let destination = if rollback { &sender } else { &receiver };
        let adopted: Vec<_> = (0..if whole_tab { 2 } else { 1 })
            .map(|_| destination.expect_adopted_pty())
            .collect();
        let arrived = adopted
            .into_iter()
            .find(|pane| pane.run_command() == command)
            .unwrap();
        assert_eq!(arrived.run_command(), command);
        let pane_id = PaneId::Terminal(arrived.terminal_id());
        destination.wait_until("the adopted agent record to be installed", |_| {
            let (mut control, mut replies) = agent_connection(destination);
            control
                .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
                    pane_id,
                    expected_session_id: Some("warp-conversation".into()),
                }))
                .unwrap();
            agent_response(&mut replies)
                .agents
                .first()
                .is_some_and(|status| {
                    status.agent.session_id.as_deref() == Some("warp-conversation")
                })
        });
        let (mut control, mut replies) = agent_connection(destination);
        control
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
                pane_id,
                expected_session_id: Some("warp-conversation".into()),
            }))
            .unwrap();
        assert!(!agent_response(&mut replies).agents[0].status_available);
        // No repeated SessionStart or terminal initialization: only completion arrives.
        codex_report(destination, &arrived, "Stop", "turn-1", "");
        control
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
                pane_id,
                expected_session_id: Some("warp-conversation".into()),
            }))
            .unwrap();
        let status = agent_response(&mut replies);
        assert_eq!(status.agents[0].ready, enabled, "{status:?}");
        assert_eq!(
            status.agents[0].agent.session_id.as_deref(),
            Some("warp-conversation")
        );
        if enabled {
            control
                .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id,
                    expected_session_id: Some("warp-conversation".into()),
                    prompt: "next".into(),
                    wait: false,
                    timeout_ms: 30_000,
                }))
                .unwrap();
            arrived.wait_for_stdin("next prompt reaches the same process", |bytes| {
                bytes
                    .windows(b"\x1b[200~next\x1b[201~\r".len())
                    .any(|bytes| bytes == b"\x1b[200~next\x1b[201~\r")
            });
            codex_report(destination, &arrived, "UserPromptSubmit", "turn-2", "next");
            assert_eq!(
                agent_response(&mut replies).outcome,
                AgentOutcome::Submitted
            );
        }
    }
}

#[test]
fn warp_adoption_failure_restores_the_sessions_only_live_pane() {
    let ((sender, subject), (receiver, _)) = sender_and_receiver();
    focus_and_confirm(&sender, &keys::alt('h'), &subject);
    subject.fail_warp(WarpFailure::UnadoptableOnce);
    let (code, output) = warp_focused_pane_to(&sender, &receiver, false);
    assert_ne!(code, 0);
    assert!(output.contains("back in this session"), "{output}");
    let restored = sender.expect_adopted_pty();
    restored.output(b"SOLE PANE RESTORED\r\n");
    sender.wait_until("rollback output", |grid| {
        grid.contains("SOLE PANE RESTORED")
    });
    type_and_confirm(&sender, &restored, b"rollback-input");
    assert!(!restored.is_exited());
}

#[test]
fn warp_disconnected_destination_restores_the_only_held_pane() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::{
        ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg,
    };

    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let name = format!("warp-disconnect-{}", std::process::id());
    std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
    let _socket = RemoveOnDrop(path.clone());
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let destination = std::thread::spawn(move || {
        let probe = listener.accept().unwrap();
        let mut reply: IpcSenderWithContext<ServerToClientMsg> = IpcSenderWithContext::new(probe);
        let mut request: IpcReceiverWithContext<ClientToServerMsg> = reply.get_receiver().unwrap();
        assert!(matches!(
            request.recv_client_msg().msg(),
            Some((ClientToServerMsg::ConnStatus, _))
        ));
        reply
            .send_server_msg(ServerToClientMsg::Connected {
                warp_companions: true,
                warp_held_terminals: true,
            })
            .unwrap();
        let mut adoption: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(listener.accept().unwrap());
        assert!(matches!(
            adoption.recv_client_msg().msg(),
            Some((
                ClientToServerMsg::Action {
                    action: yinz_types::input::actions::Action::AdoptPane { .. },
                    ..
                },
                _
            ))
        ));
        // Drop the adoption connection after accepting the request, without a reply.
    });
    let sender = start(Some(
        r#"layout { pane name="sole held task" command="codex" cwd="/tmp"; }"#,
    ));
    let subject = sender.expect_pty_spawn();
    subject.output(b"SOLE HELD SCROLLBACK\r\n");
    sender.wait_until("held output before exit", |grid| {
        grid.contains("SOLE HELD SCROLLBACK")
    });
    subject.exit(Some(17));
    let held_state = || {
        let (code, output) = sender.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        serde_json::from_str::<Vec<serde_json::Value>>(&output)
            .unwrap()
            .into_iter()
            .find(|pane| pane["is_plugin"] == false)
            .unwrap()
    };
    sender.wait_until("held exit before transfer", |_| {
        held_state()["is_held"] == true
    });
    let before = held_state();
    let (code, output) = sender.run_cli_action_reporting(CliAction::WarpPane {
        to_session: name,
        pane_id: Some(format!("terminal_{}", subject.terminal_id())),
        floating: false,
        focused_tab: true,
    });
    assert_ne!(code, 0);
    assert!(output.contains("back in this session"), "{output}");
    destination.join().unwrap();
    sender.wait_until("held rollback state", |grid| {
        grid.contains("SOLE HELD SCROLLBACK")
    });
    let after = held_state();
    for field in ["is_held", "exited", "exit_status"] {
        assert_eq!(after[field], before[field], "{field}: {after}");
    }
    sender.send_stdin(b"\r");
    let restarted = sender.expect_pty_spawn();
    restarted.wait_for_command("codex");
    assert_eq!(restarted.run_command().cwd, Some("/tmp".into()));
}
