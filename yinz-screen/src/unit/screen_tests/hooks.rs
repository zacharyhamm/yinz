//! The hooks the screen fires, from the moment to the plugin thread being asked to run them.
//!
//! `agent_exit` used to be a branch in the terminal emulator - `TerminalPane::hold` painted a
//! fixed question over any pane whose originating plugin carried `agent_pane`. What the
//! screen does now is fire a hook, and it fires several: `pane_exit` for any command pane,
//! `agent_exit` for an agent's, `agent_start` and `agent_waiting` off the edges of an agent
//! record. These check each moment fires what it should, and nothing else.

#[cfg(test)]
const TERMINAL_2: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(2);

use super::*;
use crate::plugins::HookFiring;
use crate::screen::{agent_hook_edges, apply_pane_agent_report};
use std::collections::BTreeMap;
use std::path::Path;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::data::HookPayload;
use yinz_types::data::OriginatingPlugin;
use yinz_types::data::AGENT_PANE_CONTEXT;
use yinz_types::input::hooks::HookPoint;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TabId;
use yinz_types::keys::{BareKey, KeyWithModifier};

type Fired = (HookPoint, Option<PaneId>, HookFiring);

/// The configuration a `ReplacePane` firing hands its plugin.
fn config(firing: &HookFiring) -> &BTreeMap<String, String> {
    match firing {
        HookFiring::ReplacePane(configuration) => configuration,
        other => panic!("expected a pane-replacing firing, got {other:?}"),
    }
}

/// The typed payload of an agent point's firing.
fn agent(
    firing: &HookFiring,
) -> (
    crate::TerminalId,
    &AgentInfo,
    Option<AgentState>,
    Option<&str>,
) {
    match firing {
        HookFiring::Headless(HookPayload::Agent {
            pane_id,
            agent,
            previous_state,
            command,
        }) => (*pane_id, agent, *previous_state, command.as_deref()),
        other => panic!("expected an agent firing, got {other:?}"),
    }
}

/// The typed payload of a `pane_exit` firing.
fn exit(
    firing: &HookFiring,
) -> (
    crate::TerminalId,
    Option<i32>,
    Option<&Path>,
    Option<&str>,
    bool,
) {
    match firing {
        HookFiring::Headless(HookPayload::PaneExit {
            pane_id,
            exit_code,
            cwd,
            command,
            held,
        }) => (
            *pane_id,
            *exit_code,
            cwd.as_deref(),
            command.as_deref(),
            *held,
        ),
        other => panic!("expected a pane_exit firing, got {other:?}"),
    }
}

fn agent_command(cwd: &str) -> RunCommand {
    RunCommand {
        command: PathBuf::from("codex"),
        cwd: Some(PathBuf::from(cwd)),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(
            yinz_types::internal_ids::PluginId::new(2),
            crate::ClientId::new(1),
            BTreeMap::from([(AGENT_PANE_CONTEXT.to_owned(), "true".to_owned())]),
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
        // A wait a person must answer is the common case.
        wait_needs_a_person: state == AgentState::Waiting,
        cwd: Some("/work".to_owned()),
        session_id: session_id.map(str::to_owned),
        run_id: None,
    }
}

fn new_pane(pane_id: u32, invoked_with: Option<Run>) -> ScreenInstruction {
    ScreenInstruction::NewPane(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        Some("codex".to_string()),
        None,
        invoked_with,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(crate::ClientId::new(10)),
        None,
        false,
    )
}

fn set_agent(pane_id: u32, agent: Option<AgentInfo>) -> ScreenInstruction {
    ScreenInstruction::SetPaneAgent(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        agent,
        None,
    )
}

fn hold(pane_id: u32, run_command: RunCommand) -> ScreenInstruction {
    ScreenInstruction::HoldPane(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        Some(0),
        run_command,
    )
}

fn run_command_in_pane(pane_id: u32, run_command: RunCommand) -> ScreenInstruction {
    ScreenInstruction::RunCommandInPane(crate::TerminalId::new(pane_id), run_command, false)
}

/// Drive the screen through `steps` and return every hook it asked the plugin thread for, in
/// order. The screen drains its instructions in order and `teardown` waits for the last one, so
/// nothing here needs a sleep.
fn hooks_fired_by(steps: Vec<ScreenInstruction>) -> Vec<Fired> {
    hooks_fired_by_with_config(steps, None, None)
}

fn hooks_fired_by_with_config(
    steps: Vec<ScreenInstruction>,
    config_file_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Vec<Fired> {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.session_metadata.config_file_path = config_file_path;
    mock_screen.session_metadata.config_dir = config_dir;
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
    // Adoption queues the pane and its serialized record on the screen thread itself.
    // Hold this consumer until the test has queued the same complete batch.
    let (response_channel, release) = crossbeam::channel::bounded(0);
    mock_screen
        .to_screen
        .send(ScreenInstruction::ListPanes {
            show_all: true,
            response_channel,
        })
        .unwrap();
    for step in steps {
        let _ = mock_screen.to_screen.send(step);
    }
    release
        .recv_timeout(yinz_test_support::default_timeout())
        .expect("the screen reached the queued adoption batch");
    instructions.wait_until(
        "the buffered agent report to reach its adopted pane",
        yinz_test_support::default_timeout(),
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
    new_tab(&mut screen, 1, TabId::new(0));
    drain_background_jobs(&background_jobs);
    plugin_receiver.try_iter().for_each(drop);

    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let waiting = report(AgentState::Waiting, Some("s1"));
    assert!(screen
        .tab_containing_pane_mut(pane_id)
        .expect("pane exists")
        .set_pane_agent(pane_id, Some(waiting.clone())));
    screen.report_pane_state().expect("pane state reports");

    let cached = screen
        .peer_sessions_cache
        .get("yinz-test")
        .expect("current session stays cached");
    assert!(cached.panes.panes.values().flatten().any(|pane| {
        pane.id == PaneId::Terminal(crate::TerminalId::new(1))
            && pane.agent.as_ref() == Some(&waiting)
    }));
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
    new_tab(&mut screen, 1, TabId::new(0));
    drain_background_jobs(&background_jobs);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
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
                crate::ClientId::new(10),
                None,
            ),
            set_agent(42, Some(fresh)),
            ScreenInstruction::ApplyLayout(
                layout,
                vec![],
                vec![(crate::TerminalId::new(42), None)],
                vec![],
                vec![(
                    crate::TerminalId::new(42),
                    RunCommand::default(),
                    Some(serialized),
                )],
                HashMap::new(),
                TabId::new(1),
                true,
                crate::ClientId::new(10),
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
    assert_eq!(*pane_id, Some(PaneId::Terminal(crate::TerminalId::new(2))));
    let payload = config(payload);
    assert_eq!(payload.get("agent_pane_id").map(String::as_str), Some("2"));
    assert_eq!(
        payload.get("agent_pane_title").map(String::as_str),
        Some("codex")
    );
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
fn agent_exit_captures_the_custom_title_with_host_and_checkout_prefixes() {
    let pane = PaneId::Terminal(TERMINAL_2);
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        ScreenInstruction::RenamePane(pane, b"Review topic".to_vec(), None),
        ScreenInstruction::UpdatePaneHost(TERMINAL_2, Some("build-box".to_owned())),
        ScreenInstruction::SetPaneTitlePrefix(pane, Some("topic".to_owned())),
        hold(2, agent_command("/work/topic")),
    ]);
    let payload = config(
        &fired
            .iter()
            .find(|(point, _, _)| *point == HookPoint::AgentExit)
            .unwrap()
            .2,
    );
    assert_eq!(
        payload.get("agent_pane_title").unwrap(),
        "build-box topic: Review topic"
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
    assert_eq!(
        fired[2].1,
        Some(PaneId::Terminal(crate::TerminalId::new(3)))
    );
}

/// The shape of a pane warped in from another session: no originating plugin - the supervisor
/// drops one that names a plugin in the session the pane left - and a record cleared by the
/// agent's own `SessionEnd` a moment before its process exits. The pane remembers it was an
/// agent, so the exit still reaches the hook.
#[test]
fn a_pane_whose_record_was_cleared_before_it_exited_still_fires_agent_exit() {
    let adopted = RunCommand {
        command: PathBuf::from("claude"),
        cwd: Some(PathBuf::from("/work")),
        hold_on_close: true,
        originating_plugin: None,
        ..Default::default()
    };
    let fired = hooks_fired_by(vec![
        new_pane(3, None),
        set_agent(3, Some(report(AgentState::Busy, Some("s1")))),
        set_agent(3, None),
        hold(3, adopted),
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
    assert_eq!(
        fired[2].1,
        Some(PaneId::Terminal(crate::TerminalId::new(3)))
    );
}

/// Clearing a record a pane never had says nothing about the pane, so it must not arm the
/// memory the test above relies on.
#[test]
fn clearing_a_record_a_pane_never_had_does_not_make_it_an_agent() {
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        set_agent(2, None),
        hold(2, echo_command()),
    ]);
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
}

/// A held agent pane put to a different command is no longer that agent's pane. Neither the
/// record nor the memory of one says anything true about `npm test`, so the second exit is an
/// ordinary command pane's - `pane_exit` and nothing else.
#[test]
fn a_pane_re_run_with_a_different_command_stops_being_an_agent_pane() {
    let agent = RunCommand {
        command: PathBuf::from("claude"),
        cwd: Some(PathBuf::from("/work")),
        hold_on_close: true,
        // No marker: the memory is what is under test, and the context road would answer for
        // the pane on its own.
        originating_plugin: None,
        ..Default::default()
    };
    let fired = hooks_fired_by(vec![
        new_pane(3, None),
        set_agent(3, Some(report(AgentState::Busy, Some("s1")))),
        hold(3, agent),
        run_command_in_pane(3, echo_command()),
        hold(3, echo_command()),
    ]);
    assert_eq!(
        points(&fired),
        vec![
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit,
            HookPoint::PaneExit
        ],
        "got: {fired:?}"
    );
}

/// The same road with the agent's own command: a plugin restarting the agent builds a fresh
/// `RunCommand`, so the comparison reads the command and its arguments and not the fields
/// around them. The pane is still the agent's, and its next exit still fires the hook.
#[test]
fn a_pane_re_run_with_its_own_command_is_still_an_agent_pane() {
    let agent = RunCommand {
        command: PathBuf::from("claude"),
        cwd: Some(PathBuf::from("/work")),
        hold_on_close: true,
        originating_plugin: None,
        ..Default::default()
    };
    let restarted = RunCommand {
        hold_on_close: false,
        ..agent.clone()
    };
    let fired = hooks_fired_by(vec![
        new_pane(3, None),
        set_agent(3, Some(report(AgentState::Busy, Some("s1")))),
        hold(3, agent.clone()),
        run_command_in_pane(3, restarted),
        hold(3, agent),
    ]);
    assert_eq!(
        points(&fired),
        vec![
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit,
            HookPoint::AgentStart,
            HookPoint::PaneExit,
            HookPoint::AgentExit
        ],
        "got: {fired:?}"
    );
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
    // Having an originating plugin is not enough: it is the `agent_pane` marker in its
    // context that says this pane was an agent.
    let from_a_plugin = RunCommand {
        command: PathBuf::from("echo"),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(
            yinz_types::internal_ids::PluginId::new(2),
            crate::ClientId::new(1),
            BTreeMap::new(),
        )),
        ..Default::default()
    };
    let fired = hooks_fired_by_holding(2, from_a_plugin);
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
}

#[test]
fn an_exiting_command_pane_fires_pane_exit() {
    let fired = hooks_fired_by_holding(2, echo_command());
    let (_, pane_id, payload) = &fired[0];
    assert_eq!(*pane_id, Some(PaneId::Terminal(crate::TerminalId::new(2))));
    let (exited_pane, exit_code, cwd, command, held) = exit(payload);
    assert_eq!(exited_pane, yinz_types::internal_ids::TerminalId::new(2));
    assert_eq!(exit_code, Some(0));
    assert_eq!(cwd, Some(Path::new("/work")));
    assert!(held, "a held pane is still there to be re-run");
    assert!(
        command.is_some_and(|c| c.contains("echo")),
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
        ScreenInstruction::ClosePane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            Some(1),
        ),
    ]);
    assert_eq!(points(&fired), vec![HookPoint::PaneExit], "got: {fired:?}");
    let (_, exit_code, _, _, held) = exit(&fired[0].2);
    assert_eq!(exit_code, Some(1));
    assert!(!held);
}

#[test]
fn a_pane_closed_by_the_user_fires_no_pane_exit() {
    // A close that names a client is the user's doing, not the command's.
    let fired = hooks_fired_by(vec![
        new_pane(2, Some(Run::Command(echo_command()))),
        ScreenInstruction::ClosePane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            Some(crate::ClientId::new(10)),
            None,
            None,
        ),
    ]);
    assert!(fired.is_empty(), "expected no hook, got: {fired:?}");
}

#[test]
fn a_shell_pane_closing_fires_no_pane_exit() {
    // Not a command pane: a shell that exits is the user leaving, not a command finishing.
    let fired = hooks_fired_by(vec![
        new_pane(2, None),
        ScreenInstruction::ClosePane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            Some(0),
        ),
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
    let (pane, record, previous, command) = agent(payload);
    assert_eq!(pane, yinz_types::internal_ids::TerminalId::new(2));
    assert_eq!(record.kind, "codex");
    assert_eq!(record.state, AgentState::Busy);
    assert_eq!(record.source, AgentSource::Spawned);
    assert_eq!(record.cwd.as_deref(), Some("/work"));
    assert!(previous.is_none());
    assert!(
        command.is_some_and(|c| c.contains("codex")),
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
    let (_, record, _, _) = agent(&fired[0].2);
    assert_eq!(record.kind, "claude");
    assert_eq!(record.source, AgentSource::Reported);
    assert_eq!(record.session_id.as_deref(), Some("s1"));
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
    let (_, record, previous, _) = agent(&fired[1].2);
    assert_eq!(record.state, AgentState::Waiting);
    assert_eq!(previous, Some(AgentState::Busy));
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
        vec![
            HookPoint::AgentStart,
            HookPoint::AgentIdle,
            HookPoint::AgentStart
        ],
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
        ScreenInstruction::RerunCommandPane(crate::TerminalId::new(2), None),
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
    assert_eq!(agent(&fired[3].2).2, Some(AgentState::Exited));
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
            PtyInstruction::ReRunCommandInPane(PaneId::Terminal(TERMINAL_2), _, _)
        )
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut reruns = 0;
    while reruns == 0 && std::time::Instant::now() < deadline {
        match pty_receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok((instruction, _)) if is_rerun(&instruction) => reruns += 1,
            Ok(_) | Err(yinz_runtime::channels::RecvTimeoutError::Timeout) => {},
            Err(yinz_runtime::channels::RecvTimeoutError::Disconnected) => break,
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
    assert_eq!(agent(&fired[3].2).2, Some(AgentState::Exited));
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
        // The turn ended: from working, or from a wait that was answered.
        (Some(&busy), Some(&idle), vec![HookPoint::AgentIdle]),
        (Some(&waiting), Some(&idle), vec![HookPoint::AgentIdle]),
        (Some(&idle), Some(&idle), vec![]),
        (Some(&exited), Some(&idle), vec![HookPoint::AgentStart]),
        (None, Some(&idle), vec![HookPoint::AgentStart]),
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

#[test]
fn held_waiting_agent_repaints_without_pty_output() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock = MockScreen::new(size);
    let screen_thread = mock.run(None, vec![]);
    let renders = Watched::new(vec![]);
    let server_receiver = mock.server_receiver.take().unwrap();
    let server_thread =
        log_actions_in_thread!(renders, ServerInstruction::KillSession, server_receiver);
    mock.to_screen
        .send(set_agent(0, Some(report(AgentState::Waiting, Some("s1")))))
        .unwrap();
    wait_for_settled_render(&renders, size, "waiting agent bell", |frame| {
        frame.contains('🔔')
    });
    let checkpoint = renders.lock().len();
    // The mock has no plugin thread or PTY EOF to trigger a repaint after this hold.
    mock.to_screen.send(hold(0, echo_command())).unwrap();
    wait_for_render_after(
        &renders,
        size,
        checkpoint,
        "held agent clears waiting bell",
        |_, frame| frame.contains("EXIT CODE: 0") && !frame.contains('🔔'),
    );
    mock.teardown(vec![screen_thread, server_thread]);
}

#[test]
fn replayed_held_waiting_agent_requests_repaint() {
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    let tab_id = TabId::new(0);
    new_tab(&mut screen, 1, tab_id);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(apply_pane_agent_report(
        &mut screen,
        pane_id,
        Some(report(AgentState::Waiting, Some("s1"))),
    )
    .unwrap());
    assert_eq!(screen.plugin_tab_names()[&tab_id], "🔔 Tab #1");
    drain_background_jobs(&background_jobs);

    super::super::replay_pending_plugin_events(&mut screen, vec![hold(1, echo_command())]).unwrap();

    assert_eq!(
        screen.tabs[&tab_id].pane_agent_info(pane_id).unwrap().state,
        AgentState::Exited,
    );
    assert_eq!(screen.plugin_tab_names()[&tab_id], "Tab #1");
    assert!(drain_background_jobs(&background_jobs).contains(&BackgroundJob::RenderToClients));
}

#[test]
fn waiting_agent_bell_survives_typing_and_repeated_reports_without_animation() {
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    let tab_id = TabId::new(0);
    new_tab(&mut screen, 1, tab_id);
    drain_background_jobs(&background_jobs);
    let (to_plugin, plugin_updates) = channels::unbounded();
    screen
        .bus
        .senders
        .replace_to_plugin(SenderWithContext::new(to_plugin));
    screen.cli_event_subscribers_present = true;
    let reported_name = || {
        plugin_updates.try_iter().find_map(|(instruction, _)| {
            let PluginInstruction::Update(events) = instruction else {
                return None;
            };
            events.into_iter().find_map(|(_, _, event)| {
                let Event::TabUpdate(tabs) = event else {
                    return None;
                };
                tabs.into_iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.name)
            })
        })
    };
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let waiting = report(AgentState::Waiting, Some("s1"));
    for _ in 0..2 {
        assert!(apply_pane_agent_report(&mut screen, pane_id, Some(waiting.clone())).unwrap());
        screen
            .tab_containing_pane_mut(pane_id)
            .unwrap()
            .write_to_pane_id(
                &Some(KeyWithModifier::new(BareKey::Char('y'))),
                b"y".to_vec(),
                false,
                pane_id,
                None,
                None,
            )
            .unwrap();
        assert_eq!(screen.plugin_tab_names()[&tab_id], "🔔 Tab #1");
        assert!(screen.tabs[&tab_id]
            .pane_agent_info(pane_id)
            .unwrap()
            .is_waiting_on_a_person());
    }
    assert_eq!(reported_name().as_deref(), Some("🔔 Tab #1"));
    assert_eq!(
        screen.animation_clock_period(std::time::Instant::now()),
        None
    );
    assert!(apply_pane_agent_report(&mut screen, pane_id, None).unwrap());
    assert_eq!(screen.plugin_tab_names()[&tab_id], "Tab #1");
    assert_eq!(reported_name().as_deref(), Some("Tab #1"));
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
    assert_eq!(agent(&fired[0].2).1.kind, "codex");
}

#[test]
fn accepted_hook_reports_keep_events_and_state_in_arrival_order_across_creation() {
    for kind in ["claude", "codex"] {
        assert_hook_report_order(kind);
    }
}

fn assert_hook_report_order(kind: &str) {
    use {yinz_types::agent::report::AgentReport, yinz_types::agent::report::AgentReportState};
    let mut mock = MockScreen::new(Size { cols: 80, rows: 20 });
    // No plugin consumer runs: screen delivery must only enqueue its events.
    let plugins = mock.plugin_receiver.take().unwrap();
    let instruction = |hook: &str, state: AgentReportState| {
        let state = match state {
            AgentReportState::Set(mut agent) => {
                agent.kind = kind.into();
                AgentReportState::Set(agent)
            },
            other => other,
        };
        let json = serde_json::json!({"hook_event_name": hook}).to_string();
        let event = serde_json::from_str(&json).unwrap();
        ScreenInstruction::AgentReport(
            AgentReport {
                report_id: "00000000-0000-4000-8000-000000000001".into(),
                pane_id: yinz_types::data::PaneId::Terminal(
                    yinz_types::internal_ids::TerminalId::new(2),
                ),
                state,
                event: Some(yinz_types::agent::AgentEvent::from_hook(
                    kind,
                    yinz_types::internal_ids::TerminalId::new(2),
                    &event,
                    json,
                )),
                trace: false,
                process_id: ProcessId::new(42),
                kind: kind.into(),
            },
            std::time::Instant::now(),
        )
    };
    let screen_thread = mock.run(None, vec![]);
    // The target pane does not exist yet; its complete reports must wait in order.
    mock.to_screen
        .send(instruction(
            "PreToolUse",
            AgentReportState::Set(report(AgentState::Busy, None)),
        ))
        .unwrap();
    mock.to_screen
        .send(instruction(
            "Stop",
            AgentReportState::Set(report(AgentState::Waiting, None)),
        ))
        .unwrap();
    mock.to_screen.send(new_pane(2, None)).unwrap();
    mock.to_screen
        .send(instruction("PostToolUse", AgentReportState::Unchanged))
        .unwrap();
    mock.to_screen
        .send(instruction(
            "Stop",
            AgentReportState::Set(report(AgentState::Waiting, None)),
        ))
        .unwrap();
    mock.to_screen
        .send(instruction("SessionEnd", AgentReportState::Clear))
        .unwrap();
    mock.teardown(vec![screen_thread]);
    let mut state = None;
    let mut observed = vec![];
    for (instruction, _) in plugins.try_iter() {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                match event {
                    Event::SessionUpdate(sessions, _) => {
                        for session in sessions {
                            for pane in session.panes.panes.values().flatten() {
                                if pane.id == PaneId::Terminal(crate::TerminalId::new(2)) {
                                    if let Some(agent) = &pane.agent {
                                        assert_eq!(agent.kind, kind);
                                    }
                                    state = pane.agent.as_ref().map(|a| a.state);
                                }
                            }
                        }
                    },
                    Event::AgentEvent(event) => {
                        assert_eq!(event.kind, kind);
                        observed.push((event.hook, state));
                    },
                    _ => {},
                }
            }
        }
    }
    let stop_state = if kind == "claude" {
        AgentState::Busy
    } else {
        AgentState::Waiting
    };
    assert_eq!(
        observed,
        vec![
            ("PreToolUse".into(), Some(AgentState::Busy)),
            ("Stop".into(), Some(stop_state)),
            ("PostToolUse".into(), Some(stop_state)),
            ("Stop".into(), Some(stop_state)),
            ("SessionEnd".into(), None),
        ]
    );
}

#[test]
fn agent_double_escape_screen_resets_and_menu_dismissal() {
    for reset in [
        "key",
        "mouse",
        "mode",
        "focus",
        "host_focus",
        "config",
        "menu",
        "disconnect",
    ] {
        let mut mock = MockScreen::new(Size { cols: 80, rows: 20 });
        mock.config.options.agent_double_escape = Some(true);
        mock.config.options.context_menu_on_right_click = Some(true);
        let handle = mock.run(None, vec![]);
        let renders = Watched::new(vec![]);
        let server_receiver = mock.server_receiver.take().unwrap();
        let server_thread =
            log_actions_in_thread!(renders, ServerInstruction::KillSession, server_receiver);
        let size = Size { cols: 80, rows: 20 };
        let client = mock.main_client_id;
        let pane = PaneId::Terminal(crate::TerminalId::new(0));
        let esc = || {
            ScreenInstruction::WriteCharacter(
                Some(KeyWithModifier::new(BareKey::Esc)),
                vec![27],
                false,
                client,
                None,
            )
        };
        mock.to_screen
            .send(set_agent(0, Some(report(AgentState::Busy, Some("s1")))))
            .unwrap();
        mock.to_screen.send(esc()).unwrap();
        wait_for_settled_render(&renders, size, "first Esc hint", |frame| {
            frame.contains("Esc again")
        });
        let checkpoint = renders.lock().len();
        mock.to_screen.send(esc()).unwrap();
        wait_for_render_after(
            &renders,
            size,
            checkpoint,
            "confirmed Esc clears hint",
            |_, frame| !frame.contains("Esc again"),
        );
        let checkpoint = renders.lock().len();
        mock.to_screen.send(esc()).unwrap();
        wait_for_render_after(&renders, size, checkpoint, "next Esc hint", |_, frame| {
            frame.contains("Esc again")
        });
        let checkpoint = renders.lock().len();
        match reset {
            "key" => mock
                .to_screen
                .send(ScreenInstruction::ResetAgentEscape(client))
                .unwrap(),
            "mouse" => mock
                .to_screen
                .send(ScreenInstruction::MouseEvent(
                    MouseEvent::new_buttonless_motion(Position::new(0, 0)),
                    client,
                    None,
                ))
                .unwrap(),
            "mode" => {
                for mode in [InputMode::Pane, InputMode::Normal] {
                    mock.to_screen
                        .send(ScreenInstruction::ChangeMode(mode, None, client, None))
                        .unwrap();
                }
            },
            "host_focus" => {
                for focused in [false, true] {
                    mock.to_screen
                        .send(ScreenInstruction::HostTerminalFocusChanged(client, focused))
                        .unwrap();
                }
            },
            "focus" => {
                mock.to_screen
                    .send(ScreenInstruction::NewPane(
                        PaneId::Terminal(crate::TerminalId::new(2)),
                        None,
                        None,
                        None,
                        NewPanePlacement::default(),
                        false,
                        ClientTabIndexOrPaneId::ClientId(client),
                        None,
                        false,
                    ))
                    .unwrap();
                mock.to_screen
                    .send(ScreenInstruction::FocusPaneWithId(
                        pane, false, false, client, None,
                    ))
                    .unwrap();
            },
            "config" => mock
                .session_metadata
                .propagate_configuration_changes(vec![(client, mock.config.clone())], false),
            "menu" => {
                mock.to_screen
                    .send(ScreenInstruction::MouseEvent(
                        MouseEvent::new_right_press_event(Position::new(3, 6)),
                        client,
                        None,
                    ))
                    .unwrap();
                mock.to_screen.send(esc()).unwrap();
            },
            "disconnect" => {
                mock.to_screen
                    .send(ScreenInstruction::RemoveClient(client))
                    .unwrap();
                mock.to_screen
                    .send(ScreenInstruction::AddClient(
                        client,
                        Size { cols: 80, rows: 20 },
                        None,
                        None,
                    ))
                    .unwrap();
            },
            _ => unreachable!(),
        }
        wait_for_render_after(&renders, size, checkpoint, reset, |_, frame| {
            !frame.contains("Esc again")
        });
        mock.to_screen.send(esc()).unwrap();
        mock.to_screen.send(ScreenInstruction::Exit).unwrap();
        handle.join().unwrap();
        let writes: Vec<_> = mock
            .pty_writer_receiver
            .take()
            .unwrap()
            .try_iter()
            .filter_map(|(msg, _)| match msg {
                PtyWriteInstruction::Write(bytes, _, _) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes
                .iter()
                .filter(|bytes| bytes.as_slice() == b"\x1b")
                .count(),
            1,
            "{reset}: {writes:?}"
        );
        mock.teardown(vec![server_thread]);
    }
}

#[test]
fn a_replacement_agent_starts_a_fresh_record_in_the_same_pane() {
    for kind in ["claude", "codex"] {
        let original = RunCommand {
            command: "claude".into(),
            hold_on_close: true,
            ..Default::default()
        };
        let replacement = RunCommand {
            command: kind.into(),
            ..agent_command("/tmp/topic")
        };
        let fired = hooks_fired_by(vec![
            new_pane(3, None),
            set_agent(3, Some(report(AgentState::Busy, Some("old-session")))),
            hold(3, original),
            run_command_in_pane(3, replacement.clone()),
            hold(3, replacement),
        ]);
        let starts: Vec<_> = fired
            .iter()
            .filter(|(point, _, _)| *point == HookPoint::AgentStart)
            .collect();
        assert_eq!(starts.len(), 2, "{fired:?}");
        let (pane, agent, _, _) = agent(&starts[1].2);
        assert_eq!(pane, crate::TerminalId::new(3));
        assert_eq!(agent.kind, kind);
        assert_eq!(agent.cwd.as_deref(), Some("/tmp/topic"));
        assert_eq!(agent.source, AgentSource::Spawned);
        assert!(agent.session_id.is_none() && agent.run_id.is_none());
        assert_eq!(fired.last().unwrap().0, HookPoint::AgentExit);
        let exits: Vec<_> = fired
            .iter()
            .filter(|(point, _, _)| *point == HookPoint::AgentExit)
            .map(|(_, _, firing)| config(firing))
            .collect();
        assert_eq!(exits.len(), 2);
        assert_eq!(exits[0]["agent_session_id"], "old-session");
        assert!(!exits[1].contains_key("agent_session_id"));
    }
}

#[test]
fn agent_restart_keeps_flags_and_session_while_suppressed_then_fires_exit_again() {
    use yinz_types::agent::control::AgentRestartSession;
    for session in [AgentRestartSession::Resume, AgentRestartSession::Fresh] {
        let (mut screen, _) = screen_with_background_jobs(Size { cols: 80, rows: 24 });
        let (to_plugin, plugins) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        let (to_pty, pty) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        let tab_id = TabId::new(0);
        new_tab(&mut screen, 2, tab_id);
        let pane_id = PaneId::Terminal(TERMINAL_2);
        let plugin_id = yinz_types::internal_ids::PluginId::new(42);
        let original = RunCommand {
            command: "claude".into(),
            args: ["--model", "X", "--permission-mode", "Y", "task"]
                .map(str::to_owned)
                .into(),
            cwd: Some("/tmp/worktree".into()),
            hold_on_close: true,
            ..Default::default()
        };
        apply_pane_agent_report(
            &mut screen,
            pane_id,
            Some(report(AgentState::Busy, Some("sid"))),
        )
        .unwrap();
        // SessionEnd clears the live record before the process exit.
        apply_pane_agent_report(&mut screen, pane_id, None).unwrap();
        super::super::replay_pending_plugin_events(&mut screen, vec![hold(2, original.clone())])
            .unwrap();
        assert_eq!(
            screen
                .get_pane_info(pane_id)
                .unwrap()
                .last_agent_session_id
                .as_deref(),
            Some("sid")
        );
        screen
            .tabs
            .get_mut(&tab_id)
            .unwrap()
            .suppress_pane_and_replace_with_pid(
                pane_id,
                PaneId::Plugin(plugin_id),
                false,
                None,
                None,
                None,
            )
            .unwrap();
        let origin = OriginatingPlugin::new(
            plugin_id,
            crate::ClientId::new(1),
            BTreeMap::from([("job".into(), "fix".into())]),
        );
        let restarted = screen
            .restart_agent_pane(pane_id, Some("P"), session, Some(origin.clone()))
            .unwrap();
        let resumed = session == AgentRestartSession::Resume;
        assert_eq!(restarted.session_id.as_deref(), resumed.then_some("sid"));
        let command = pty
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PtyInstruction::ReRunCommandInPane(id, command, _) if id == pane_id => {
                    Some(command)
                },
                _ => None,
            })
            .unwrap();
        let expected: Vec<String> = if resumed {
            vec![
                "--model",
                "X",
                "--permission-mode",
                "Y",
                "--resume",
                "sid",
                "P",
            ]
        } else {
            vec!["--model", "X", "--permission-mode", "Y", "P"]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(command.args, expected);
        assert_eq!(command.cwd, original.cwd);
        assert_eq!(command.originating_plugin, Some(origin));
        assert_eq!(restarted.command[1..], command.args);
        let info = screen.get_pane_info(pane_id).unwrap();
        assert!(info.is_suppressed && !info.is_held && !info.exited);
        let agent = info.agent.unwrap();
        assert_eq!(agent.state, AgentState::Busy);
        assert_eq!(agent.cwd.as_deref(), Some("/tmp/worktree"));
        assert_eq!(
            info.last_agent_session_id.as_deref(),
            resumed.then_some("sid")
        );
        // CloseSelf uses the ordinary close path, revealing the suppressed pane.
        screen
            .tabs
            .get_mut(&tab_id)
            .unwrap()
            .close_pane(PaneId::Plugin(plugin_id), false, None);
        assert!(!screen.get_pane_info(pane_id).unwrap().is_suppressed);
        let new_session = if resumed { "sid" } else { "new-session" };
        apply_pane_agent_report(
            &mut screen,
            pane_id,
            Some(report(AgentState::Busy, Some(new_session))),
        )
        .unwrap();
        apply_pane_agent_report(&mut screen, pane_id, None).unwrap();
        super::super::replay_pending_plugin_events(&mut screen, vec![hold(2, command)]).unwrap();
        let exits: Vec<_> = plugins
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::RunHook {
                    point: HookPoint::AgentExit,
                    pane_id: id,
                    payload,
                    ..
                } => {
                    assert_eq!(id, Some(pane_id));
                    Some(config(&payload).clone())
                },
                _ => None,
            })
            .collect();
        assert_eq!(exits.len(), 2);
        assert_eq!(exits[0]["agent_session_id"], "sid");
        assert_eq!(exits[1]["agent_session_id"], new_session);
    }
}

#[test]
fn agent_restart_rejects_missing_sessions_live_panes_and_unsafe_commands() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentRestartSession};
    let (mut screen, _) = screen_with_background_jobs(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 2, TabId::new(0));
    let pane = PaneId::Terminal(TERMINAL_2);
    assert!(screen
        .restart_agent_pane(pane, None, AgentRestartSession::Fresh, None)
        .unwrap_err()
        .contains("held"));
    apply_pane_agent_report(&mut screen, pane, Some(report(AgentState::Busy, None))).unwrap();
    let command = RunCommand {
        command: "claude".into(),
        hold_on_close: true,
        ..Default::default()
    };
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hold_pane(pane, Some(0), false, command.clone());
    let (reply, receive) = std::sync::mpsc::channel();
    screen.agent_control_with_reply(
        AgentRequest::Restart {
            pane_id: pane,
            prompt: Some("P".into()),
            session: AgentRestartSession::Resume,
        },
        crate::screen::agent_control::Reply::Host(reply),
    );
    let response = receive.recv().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Rejected);
    assert!(response.message.unwrap().contains("no remembered session"));
    assert!(screen.get_pane_info(pane).unwrap().is_held);
    let unsafe_command = RunCommand {
        args: vec!["--unknown".into()],
        ..command
    };
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .hold_pane(pane, Some(0), false, unsafe_command);
    assert_eq!(
        screen
            .restart_agent_pane(pane, None, AgentRestartSession::Fresh, None)
            .unwrap_err(),
        screen.get_pane_info(pane).unwrap().restart_error.unwrap()
    );
}

#[test]
fn agent_control_fresh_restart_without_a_prompt_forgets_the_previous_session() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentRestartSession};
    let (mut screen, _) = screen_with_background_jobs(Size { cols: 80, rows: 24 });
    let (to_pty, pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let tab_id = TabId::new(0);
    new_tab(&mut screen, 2, tab_id);
    let pane = PaneId::Terminal(TERMINAL_2);
    apply_pane_agent_report(
        &mut screen,
        pane,
        Some(report(AgentState::Busy, Some("old"))),
    )
    .unwrap();
    let command = RunCommand {
        command: "claude".into(),
        hold_on_close: true,
        ..Default::default()
    };
    screen
        .tabs
        .get_mut(&tab_id)
        .unwrap()
        .hold_pane(pane, Some(0), false, command.clone());
    let (reply, receive) = std::sync::mpsc::channel();
    screen.agent_control_with_reply(
        AgentRequest::Restart {
            pane_id: pane,
            prompt: None,
            session: AgentRestartSession::Fresh,
        },
        crate::screen::agent_control::Reply::Host(reply),
    );
    let response = receive.recv().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok);
    let restarted = response.restarted.unwrap();
    assert_eq!(restarted.pane_id, pane);
    assert_eq!(restarted.command, vec!["claude"]);
    assert_eq!(restarted.session_id, None);
    assert!(pty.try_iter().any(|(instruction, _)| matches!(instruction, PtyInstruction::ReRunCommandInPane(id, _, _) if id == pane)));
    let info = screen.get_pane_info(pane).unwrap();
    assert_eq!(info.last_agent_session_id, None);
    assert_eq!(info.agent.unwrap().session_id, None);
    assert!(!info.is_held);
    // A process that exits before its first hook must not report the old session.
    super::super::replay_pending_plugin_events(&mut screen, vec![hold(2, command)]).unwrap();
    assert_eq!(
        screen.get_pane_info(pane).unwrap().last_agent_session_id,
        None
    );
}
