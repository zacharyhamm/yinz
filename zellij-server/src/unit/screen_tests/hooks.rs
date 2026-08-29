//! The hooks the screen fires, from the moment to the plugin thread being asked to run them.
//!
//! `agent_exit` used to be a branch in the terminal emulator - `TerminalPane::hold` painted a
//! fixed question over any pane whose originating plugin carried `hocus_agent_pane`. What the
//! screen does now is fire a hook, and it fires several: `pane_exit` for any command pane,
//! `agent_exit` for an agent's, `agent_start` and `agent_waiting` off the edges of an agent
//! record. These check each moment fires what it should, and nothing else.

use super::*;
use crate::screen::agent_hook_edges;
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
        cwd: Some("/work".to_owned()),
        session_id: session_id.map(str::to_owned),
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
