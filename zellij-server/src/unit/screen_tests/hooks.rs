//! The hooks the screen fires, from the moment to the plugin thread being asked to run them.
//!
//! `agent_exit` used to be a branch in the terminal emulator - `TerminalPane::hold` painted a
//! fixed question over any pane whose originating plugin carried `hocus_agent_pane`. What the
//! screen does now is fire a hook, and it fires several: `pane_exit` for any command pane,
//! `agent_exit` for an agent's, `agent_start` and `agent_waiting` off the edges of an agent
//! record. These check each moment fires what it should, and nothing else.

use super::*;
use crate::screen::{agent_hook_edges, apply_pane_agent_report};
use std::collections::BTreeMap;
use zellij_utils::data::OriginatingPlugin;
use zellij_utils::data::HOCUS_AGENT_PANE_CONTEXT;
use zellij_utils::data::{AgentInfo, AgentSource, AgentState, BareKey, KeyWithModifier};
use zellij_utils::input::hooks::HookPoint;

type Fired = (HookPoint, Option<PaneId>, BTreeMap<String, String>);

fn agent_command(cwd: &str) -> RunCommand {
    RunCommand {
        command: PathBuf::from("codex"),
        cwd: Some(PathBuf::from(cwd)),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(
            2,
            1,
            BTreeMap::from([(HOCUS_AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
        )),
        ..Default::default()
    }
}

fn echo_command() -> RunCommand {
    RunCommand {
        command: PathBuf::from("echo"),
        args: vec!["hello".to_string()],
        cwd: Some(PathBuf::from("/work")),
        hold_on_close: true,
        ..Default::default()
    }
}

fn report(state: AgentState, session_id: Option<&str>) -> AgentInfo {
    AgentInfo {
        kind: "claude".to_owned(),
        state,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: None,
        // The blinking frame indicator is the only reader; a wait a person must answer is
        // what the common case reports, so it is what the helper reports too.
        wait_needs_a_person: state == AgentState::Waiting,
        cwd: Some("/work".to_owned()),
        session_id: session_id.map(str::to_owned),
        run_id: None,
    }
}

fn new_pane(pane_id: u32, invoked_with: Option<Run>) -> ScreenInstruction {
    ScreenInstruction::NewPane(
        PaneId::Terminal(pane_id),
        Some("codex".to_string()),
        None,
        invoked_with,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(10),
        None,
        false,
    )
}

fn set_agent(pane_id: u32, agent: Option<AgentInfo>) -> ScreenInstruction {
    ScreenInstruction::SetPaneAgent(PaneId::Terminal(pane_id), agent, None)
}

fn hold(pane_id: u32, run_command: RunCommand) -> ScreenInstruction {
    ScreenInstruction::HoldPane(PaneId::Terminal(pane_id), Some(0), run_command)
}

/// Drive the screen through `steps` and return every hook it asked the plugin thread for, in
/// order. The screen drains its instructions in order and `teardown` waits for the last one, so
/// nothing here needs a sleep.
fn hooks_fired_by(steps: Vec<ScreenInstruction>) -> Vec<Fired> {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    for step in steps {
        let _ = mock_screen.to_screen.send(step);
    }
    mock_screen.teardown(vec![screen_thread]);
    plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                ..
            } => Some((point, pane_id, payload)),
            _ => None,
        })
        .collect()
}

fn hooks_fired_by_until(steps: Vec<ScreenInstruction>, expected: HookPoint) -> Vec<Fired> {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    let instructions = Watched::new(vec![]);
    let plugin_thread =
        log_actions_in_thread!(instructions, PluginInstruction::Exit, plugin_receiver);
    for step in steps {
        let _ = mock_screen.to_screen.send(step);
    }
    instructions.wait_until(
        "the buffered agent report to reach its adopted pane",
        zellij_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|instruction| {
                    matches!(instruction, PluginInstruction::RunHook { point, .. } if *point == expected)
                })
                .then_some(())
        },
    );
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let fired = instructions
        .lock()
        .iter()
        .filter_map(|instruction| match instruction {
            PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                ..
            } => Some((*point, *pane_id, payload.clone())),
            _ => None,
        })
        .collect();
    fired
}

/// A hook-driven agent record flips several times a second under a working
/// agent. The pane manifest and the `SessionInfo` that embeds it must agree,
/// without rebuilding or broadcasting unchanged tab state.
#[test]
fn a_changed_agent_record_reports_pane_and_session_state() {
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    new_tab(&mut screen, 1, 0);
    drain_background_jobs(&background_jobs);
    plugin_receiver.try_iter().for_each(drop);

    let pane_id = PaneId::Terminal(1);
    let waiting = report(AgentState::Waiting, Some("s1"));
    assert!(screen
        .tab_containing_pane_mut(pane_id)
        .expect("pane exists")
        .set_pane_agent(pane_id, Some(waiting.clone())));
    screen.report_pane_state().expect("pane state reports");

    let cached = screen
        .peer_sessions_cache
        .get("zellij-test")
        .expect("current session stays cached");
    assert!(cached
        .panes
        .panes
        .values()
        .flatten()
        .any(|pane| { pane.id == 1 && pane.agent.as_ref() == Some(&waiting) }));
    let (_, reported) = match background_jobs.try_recv().expect("session info reported").0 {
        BackgroundJob::ReportSessionInfo(name, session_info) => (name, session_info),
        job => panic!("expected session info report, got {job:?}"),
    };
    assert_eq!(reported.panes, cached.panes);

    let events: Vec<Event> = plugin_receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .map(|(_, _, event)| event)
        .collect();
    assert!(!events
        .iter()
        .any(|event| matches!(event, Event::TabUpdate(..))));
    let session = events
        .iter()
        .find_map(|event| match event {
            Event::SessionUpdate(sessions, _) => sessions.first(),
            _ => None,
        })
        .expect("session subscribers are updated");
    assert_eq!(session.panes, cached.panes);
}

/// The run id is the one field of a record its reporter does not know: a hook says everything
/// the agent knows about itself, and the agent has no idea whose books it is being kept in. So
/// it is carried forward across reports, and only clearing the record clears it — otherwise the
/// next `PreToolUse` would undo the plugin that set it, and a warped pane would arrive naming
/// no run.
#[test]
fn a_report_that_names_no_run_keeps_the_panes() {
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    drain_background_jobs(&background_jobs);
    let pane_id = PaneId::Terminal(1);
    let record_of = |screen: &Screen| {
        screen
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id))
    };

    let mut named = report(AgentState::Busy, Some("s1"));
    named.run_id = Some("claude:s1".to_owned());
    assert!(apply_pane_agent_report(&mut screen, pane_id, Some(named)).unwrap());
    assert_eq!(
        record_of(&screen).and_then(|agent| agent.run_id),
        Some("claude:s1".to_owned())
    );

    // A hook's own record, which never names a run.
    let waiting = report(AgentState::Waiting, Some("s1"));
    assert!(apply_pane_agent_report(&mut screen, pane_id, Some(waiting)).unwrap());
    let carried = record_of(&screen).expect("the record is still there");
    assert_eq!(carried.state, AgentState::Waiting, "the report still lands");
    assert_eq!(carried.run_id, Some("claude:s1".to_owned()));

    // Clearing the record clears the run with it: there is no agent left to keep books for.
    assert!(apply_pane_agent_report(&mut screen, pane_id, None).unwrap());
    assert_eq!(record_of(&screen), None);
}

#[test]
fn agent_reports_wait_for_adopted_panes_and_tabs() {
    let fresh = report(AgentState::Waiting, Some("fresh"));
    let serialized = report(AgentState::Busy, Some("serialized"));

    let pane = hooks_fired_by_until(
        vec![
            set_agent(2, Some(fresh.clone())),
            new_pane(2, None),
            set_agent(2, Some(serialized.clone())),
        ],
        HookPoint::AgentWaiting,
    );
    assert_eq!(
        points(&pane),
        vec![
            HookPoint::AgentStart,
            HookPoint::AgentStart,
            HookPoint::AgentWaiting,
        ],
        "the buffered live report must apply after the serialized pane record: {pane:?}"
    );

    let layout = TiledPaneLayout::default();
    let tab = hooks_fired_by_until(
        vec![
            ScreenInstruction::NewTab(
                None,
                None,
                Some(layout.clone()),
                vec![],
                None,
                (Some(vec![]), Some(vec![])),
                None,
                false,
                true,
                10,
                None,
            ),
            set_agent(42, Some(fresh)),
            ScreenInstruction::ApplyLayout(
                layout,
                vec![],
                vec![(42, None)],
                vec![],
                vec![(42, serialized)],
                HashMap::new(),
                1,
                true,
                10,
                None,
                None,
            ),
        ],
        HookPoint::AgentWaiting,
    );
    assert_eq!(
        points(&tab),
        vec![
            HookPoint::AgentStart,
            HookPoint::AgentStart,
            HookPoint::AgentWaiting,
        ],
        "the buffered live report must apply after the serialized tab record: {tab:?}"
    );
}

#[test]
fn pending_agent_reports_keep_only_the_latest_for_each_pane() {
    let fired = hooks_fired_by_until(
        vec![
            set_agent(2, Some(report(AgentState::Busy, Some("stale")))),
            set_agent(2, Some(report(AgentState::Waiting, Some("fresh")))),
            new_pane(2, None),
        ],
        HookPoint::AgentWaiting,
    );
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart, HookPoint::AgentWaiting],
        "only the latest report for a missing pane should be replayed: {fired:?}"
    );
}

fn points(fired: &[Fired]) -> Vec<HookPoint> {
    fired.iter().map(|(point, _, _)| *point).collect()
}

fn hooks_fired_by_holding(pane_id: u32, run_command: RunCommand) -> Vec<Fired> {
    hooks_fired_by(vec![new_pane(pane_id, None), hold(pane_id, run_command)])
}

#[test]
fn an_exiting_agent_pane_fires_the_agent_exit_hook() {
    let fired = hooks_fired_by_holding(2, agent_command("/work/.worktrees/topic"));
    let (point, pane_id, payload) = fired
        .iter()
        .find(|(point, _, _)| *point == HookPoint::AgentExit)
        .unwrap_or_else(|| panic!("expected an agent_exit hook, got: {fired:?}"));
    assert_eq!(*point, HookPoint::AgentExit);
    assert_eq!(*pane_id, Some(PaneId::Terminal(2)));
    assert_eq!(payload.get("agent_pane_id").map(String::as_str), Some("2"));
    assert_eq!(
        payload.get("agent_cwd").map(String::as_str),
        Some("/work/.worktrees/topic"),
        "the hook is told where the agent was working, which is what it offers to restart in"
    );
    assert_eq!(
        payload.get("agent_exit_code").map(String::as_str),
        Some("0")
    );
}

#[test]
fn an_exiting_agent_pane_fires_pane_exit_then_agent_exit() {
    // The observer hears of the exit before the pane is covered by the hook that replaces it.
    let fired = hooks_fired_by_holding(2, agent_command("/work"));
    assert_eq!(
        points(&fired),
        vec![HookPoint::PaneExit, HookPoint::AgentExit],
        "got: {fired:?}"
    );
}

/// A pane nobody spawned as an agent - `claude` typed at a shell - becomes one the moment its
/// agent reports for itself, and so gets the exit hook like any other agent pane.
#[test]
fn a_pane_whose_agent_reported_for_itself_fires_the_hook_on_exit() {
    let typed_by_hand = RunCommand {
        command: PathBuf::from("claude"),
        cwd: Some(PathBuf::from("/work")),
        hold_on_close: true,
        originating_plugin: None,
        ..Default::default()
    };
    let fired = hooks_fired_by(vec![
        new_pane(3, None),
        set_agent(3, Some(report(AgentState::Busy, Some("s1")))),
        hold(3, typed_by_hand),
    ]);
    assert_eq!(
        points(&fired),
        vec![
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit
        ],
        "got: {fired:?}"
    );
    assert_eq!(fired[2].1, Some(PaneId::Terminal(3)));
}

#[test]
fn an_ordinary_command_pane_fires_no_agent_hook() {
    // The agent hooks are for agent panes. An ordinary command pane still just holds, showing
    // whatever its command left on the screen - and tells `pane_exit`, which is for every
    // command pane.
    let fired = hooks_fired_by_holding(2, echo_command());
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
}

#[test]
fn a_command_pane_from_a_plugin_that_is_not_an_agent_fires_no_agent_hook() {
    // Having an originating plugin is not enough: it is the `hocus_agent_pane` marker in its
    // context that says this pane was an agent.
    let from_a_plugin = RunCommand {
        command: PathBuf::from("echo"),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(2, 1, BTreeMap::new())),
        ..Default::default()
    };
    let fired = hooks_fired_by_holding(2, from_a_plugin);
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
}

#[test]
fn an_exiting_command_pane_fires_pane_exit() {
    let fired = hooks_fired_by_holding(2, echo_command());
    let (_, pane_id, payload) = &fired[0];
    assert_eq!(*pane_id, Some(PaneId::Terminal(2)));
    assert_eq!(payload.get("exit_pane_id").map(String::as_str), Some("2"));
    assert_eq!(payload.get("exit_code").map(String::as_str), Some("0"));
    assert_eq!(payload.get("exit_cwd").map(String::as_str), Some("/work"));
    assert_eq!(
        payload.get("exit_held").map(String::as_str),
        Some("true"),
        "a held pane is still there to be re-run"
    );
    assert!(
        payload
            .get("exit_command")
            .is_some_and(|c| c.contains("echo")),
        "got: {payload:?}"
    );
}

#[test]
fn a_command_pane_closed_by_its_own_exit_fires_pane_exit_without_hold() {
    // No `hold_on_close`: the pty closes the pane with the exit status and no client, which is
    // the one shape of close that means "the command exited".
    let mut echo = echo_command();
    echo.hold_on_close = false;
    let fired = hooks_fired_by(vec![
        new_pane(2, Some(Run::Command(echo))),
        ScreenInstruction::ClosePane(PaneId::Terminal(2), None, None, Some(1)),
    ]);
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
    let payload = &fired[0].2;
    assert_eq!(payload.get("exit_code").map(String::as_str), Some("1"));
    assert_eq!(payload.get("exit_held").map(String::as_str), Some("false"));
}

#[test]
fn a_pane_closed_by_the_user_fires_no_pane_exit() {
    // A close that names a client is the user's doing, not the command's.
    let fired = hooks_fired_by(vec![
        new_pane(2, Some(Run::Command(echo_command()))),
        ScreenInstruction::ClosePane(PaneId::Terminal(2), Some(10), None, None),
    ]);
    assert!(fired.is_empty(), "expected no hook, got: {fired:?}");
}

#[test]
fn a_shell_pane_closing_fires_no_pane_exit() {
    // Not a command pane: a shell that exits is the user leaving, not a command finishing.
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        ScreenInstruction::ClosePane(PaneId::Terminal(2), None, None, Some(0)),
    ]);
    assert!(fired.is_empty(), "expected no hook, got: {fired:?}");
}

#[test]
fn a_pane_born_as_an_agent_fires_agent_start() {
    // The spawn path writes the record as the pane is created, so there is no report to
    // observe: the creation itself is the start.
    let fired = hooks_fired_by(vec![new_pane(
        2,
        Some(Run::Command(agent_command("/work"))),
    )]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart],
        "got: {fired:?}"
    );
    let payload = &fired[0].2;
    assert_eq!(payload.get("agent_pane_id").map(String::as_str), Some("2"));
    assert_eq!(payload.get("agent_kind").map(String::as_str), Some("codex"));
    assert_eq!(payload.get("agent_state").map(String::as_str), Some("busy"));
    assert_eq!(
        payload.get("agent_source").map(String::as_str),
        Some("spawned")
    );
    assert_eq!(payload.get("agent_cwd").map(String::as_str), Some("/work"));
    assert!(payload.get("agent_previous_state").is_none());
    assert!(
        payload
            .get("agent_command")
            .is_some_and(|c| c.contains("codex")),
        "got: {payload:?}"
    );
}

#[test]
fn an_agent_reporting_for_itself_fires_agent_start() {
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, Some(report(AgentState::Busy, Some("s1")))),
    ]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart],
        "got: {fired:?}"
    );
    let payload = &fired[0].2;
    assert_eq!(
        payload.get("agent_kind").map(String::as_str),
        Some("claude")
    );
    assert_eq!(
        payload.get("agent_source").map(String::as_str),
        Some("reported")
    );
    assert_eq!(
        payload.get("agent_session_id").map(String::as_str),
        Some("s1")
    );
}

#[test]
fn an_agent_going_to_waiting_fires_agent_waiting_once() {
    // Busy -> waiting is the edge. A second report of waiting with a new detail is the same
    // wait, and a return to busy fires nothing.
    let mut with_detail = report(AgentState::Waiting, Some("s1"));
    with_detail.detail = Some("Bash".to_owned());
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, Some(report(AgentState::Busy, Some("s1")))),
        set_agent(2, Some(report(AgentState::Waiting, Some("s1")))),
        set_agent(2, Some(with_detail)),
        set_agent(2, Some(report(AgentState::Busy, Some("s1")))),
    ]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart, HookPoint::AgentWaiting],
        "got: {fired:?}"
    );
    let payload = &fired[1].2;
    assert_eq!(
        payload.get("agent_state").map(String::as_str),
        Some("waiting")
    );
    assert_eq!(
        payload.get("agent_previous_state").map(String::as_str),
        Some("busy")
    );
}

#[test]
fn a_first_report_of_waiting_fires_start_then_waiting() {
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, Some(report(AgentState::Waiting, None))),
    ]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart, HookPoint::AgentWaiting],
        "got: {fired:?}"
    );
}

#[test]
fn a_new_session_id_in_the_same_pane_is_a_new_start() {
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, Some(report(AgentState::Busy, Some("s1")))),
        set_agent(2, Some(report(AgentState::Idle, Some("s1")))),
        set_agent(2, Some(report(AgentState::Busy, Some("s2")))),
    ]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart, HookPoint::AgentStart],
        "got: {fired:?}"
    );
}

#[test]
fn clearing_the_record_fires_nothing() {
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, Some(report(AgentState::Busy, Some("s1")))),
        set_agent(2, None),
        set_agent(2, Some(report(AgentState::Exited, Some("s1")))),
    ]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart],
        "got: {fired:?}"
    );
}

#[test]
fn rerunning_an_exited_agent_pane_fires_agent_start() {
    // The pane resets its own record to busy when it restarts; the screen reads around the
    // re-run and sees exited -> busy.
    let fired = hooks_fired_by(vec![
        new_pane(2, Some(Run::Command(agent_command("/work")))),
        hold(2, agent_command("/work")),
        ScreenInstruction::RerunCommandPane(2, None),
    ]);
    assert_eq!(
        points(&fired),
        vec![
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit,
            HookPoint::AgentStart
        ],
        "got: {fired:?}"
    );
    assert_eq!(
        fired[3].2.get("agent_previous_state").map(String::as_str),
        Some("exited")
    );
}

#[test]
fn enter_in_a_held_agent_pane_fires_agent_start() {
    // Enter typed into a held pane is the same road as `RerunCommandPane`: the tab asks the
    // screen, and the screen sees exited -> busy. Before, the pane restarted itself and the
    // edge was never read.
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(None, vec![]);
    for step in [
        new_pane(2, Some(Run::Command(agent_command("/work")))),
        hold(2, agent_command("/work")),
        ScreenInstruction::WriteCharacter(
            Some(KeyWithModifier::new(BareKey::Enter)),
            b"\r".to_vec(),
            false,
            client_id,
            None,
        ),
    ] {
        let _ = mock_screen.to_screen.send(step);
    }
    // The tab answers Enter by sending the screen an instruction of its own, which lands
    // behind anything already queued - teardown's exit included - so wait for the re-run to
    // reach the pty before ending the screen.
    let is_rerun = |instruction: &PtyInstruction| {
        matches!(
            instruction,
            PtyInstruction::ReRunCommandInPane(PaneId::Terminal(2), _, _)
        )
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut reruns = 0;
    while reruns == 0 && std::time::Instant::now() < deadline {
        match pty_receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok((instruction, _)) if is_rerun(&instruction) => reruns += 1,
            Ok(_) | Err(zellij_utils::channels::RecvTimeoutError::Timeout) => {},
            Err(zellij_utils::channels::RecvTimeoutError::Disconnected) => break,
        }
    }
    mock_screen.teardown(vec![screen_thread]);
    reruns += pty_receiver
        .try_iter()
        .filter(|(instruction, _)| is_rerun(instruction))
        .count();

    let fired: Vec<Fired> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                ..
            } => Some((point, pane_id, payload)),
            _ => None,
        })
        .collect();
    assert_eq!(
        points(&fired),
        vec![
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit,
            HookPoint::AgentStart
        ],
        "got: {fired:?}"
    );
    assert_eq!(
        fired[3].2.get("agent_previous_state").map(String::as_str),
        Some("exited")
    );
    assert_eq!(reruns, 1, "the command is restarted once, by the screen");
}

#[test]
fn the_edges_of_an_agent_record() {
    let busy = report(AgentState::Busy, Some("s1"));
    let waiting = report(AgentState::Waiting, Some("s1"));
    let idle = report(AgentState::Idle, Some("s1"));
    let exited = report(AgentState::Exited, Some("s1"));
    let restarted = report(AgentState::Busy, Some("s2"));
    let unnamed = report(AgentState::Busy, None);
    let cases: Vec<(Option<&AgentInfo>, Option<&AgentInfo>, Vec<HookPoint>)> = vec![
        (None, Some(&busy), vec![HookPoint::AgentStart]),
        (
            None,
            Some(&waiting),
            vec![HookPoint::AgentStart, HookPoint::AgentWaiting],
        ),
        (None, Some(&exited), vec![]),
        (Some(&busy), Some(&waiting), vec![HookPoint::AgentWaiting]),
        (Some(&waiting), Some(&waiting), vec![]),
        (Some(&waiting), Some(&busy), vec![]),
        (Some(&busy), Some(&idle), vec![]),
        (Some(&busy), Some(&exited), vec![]),
        (Some(&exited), Some(&busy), vec![HookPoint::AgentStart]),
        (Some(&busy), Some(&restarted), vec![HookPoint::AgentStart]),
        (Some(&unnamed), Some(&busy), vec![]),
        (Some(&busy), None, vec![]),
        (None, None, vec![]),
    ];
    for (previous, current, expected) in cases {
        assert_eq!(
            agent_hook_edges(previous, current),
            expected,
            "{previous:?} -> {current:?}"
        );
    }
}

/// The blinking bell in a pane's corner is armed on the *edge* into a wait a person has to
/// answer, and taken down by the person answering it — not by the agent getting round to
/// saying it is busy again.
///
/// Three things it must not do, each of which it would do if it were driven off the record's
/// state alone: restart on a repeat `waiting` report (a `PermissionRequest` and the
/// `Notification` behind it are two reports of one prompt), light up for a wait the permission
/// mode is about to answer itself, or keep the shared animation clock running after the wait
/// is over.
#[test]
fn a_wait_a_person_must_answer_blinks_until_somebody_answers_it() {
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    drain_background_jobs(&background_jobs);
    let pane_id = PaneId::Terminal(1);
    let blinking_since = |screen: &Screen| {
        screen
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_attention_since(pane_id))
    };

    // A wait the mode answers by itself is a wait everywhere it is *drawn* and nowhere it
    // would interrupt: the record says `waiting`, and nothing blinks.
    let mut quiet = report(AgentState::Waiting, Some("s1"));
    quiet.wait_needs_a_person = false;
    assert!(apply_pane_agent_report(&mut screen, pane_id, Some(quiet)).unwrap());
    assert_eq!(blinking_since(&screen), None);
    assert!(screen.blinking_agent_panes.is_empty());

    assert!(apply_pane_agent_report(
        &mut screen,
        pane_id,
        Some(report(AgentState::Busy, Some("s1")))
    )
    .unwrap());

    // The real thing.
    let waiting = report(AgentState::Waiting, Some("s1"));
    assert!(apply_pane_agent_report(&mut screen, pane_id, Some(waiting.clone())).unwrap());
    let armed_at = blinking_since(&screen).expect("the wait is blinking");
    assert!(screen.blinking_agent_panes.contains(&pane_id));

    // The same prompt reported again — the backstop `Notification` behind a
    // `PermissionRequest` — is the same wait, and must not restart the blink half way
    // through a phase.
    let mut again = waiting.clone();
    again.waiting_on = Some("Bash rm".to_owned());
    assert!(apply_pane_agent_report(&mut screen, pane_id, Some(again)).unwrap());
    assert_eq!(
        blinking_since(&screen),
        Some(armed_at),
        "a repeat report is the same wait"
    );

    // Somebody types here. The record is untouched — the agent has not said anything — but
    // the wait has been noticed, so the corner goes quiet.
    screen
        .tab_containing_pane_mut(pane_id)
        .expect("pane exists")
        .write_to_pane_id(
            &Some(KeyWithModifier::new(BareKey::Char('y'))),
            b"y".to_vec(),
            false,
            pane_id,
            None,
            None,
        )
        .expect("the write lands");
    assert_eq!(blinking_since(&screen), None);
    assert_eq!(
        screen
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id))
            .map(|agent| agent.state),
        Some(AgentState::Waiting),
        "answering the prompt is not the agent saying anything"
    );

    // And the clock lets go of the pane, so an idle session stops waking up for it.
    screen.advance_agent_attention_blinks();
    assert!(screen.blinking_agent_panes.is_empty());
}

/// A launch with an environment is `env K=V codex …`; `env` execs, so the pane runs the agent,
/// and the record is named for it rather than for the wrapper.
#[test]
fn an_env_wrapped_agent_is_named_for_the_agent() {
    let mut wrapped = agent_command("/work");
    wrapped.command = PathBuf::from("/usr/bin/env");
    wrapped.args = vec![
        "FOO=1".to_owned(),
        "codex".to_owned(),
        "--model".to_owned(),
        "o3".to_owned(),
    ];
    let fired = hooks_fired_by(vec![new_pane(2, Some(Run::Command(wrapped)))]);
    assert_eq!(
        points(&fired),
        vec![HookPoint::AgentStart],
        "got: {fired:?}"
    );
    assert_eq!(
        fired[0].2.get("agent_kind").map(String::as_str),
        Some("codex")
    );
}
