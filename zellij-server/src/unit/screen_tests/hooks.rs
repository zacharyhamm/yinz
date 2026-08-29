//! The `agent_exit` hook, from a pane's exit to the plugin thread being asked to run it.
//!
//! This used to be a branch in the terminal emulator - `TerminalPane::hold` painted a fixed
//! question over any pane whose originating plugin carried `hocus_agent_pane`. What the screen
//! does now is fire a hook, and these check the two halves of that: an agent pane fires one, and
//! an ordinary command pane still does not.

use super::*;
use std::collections::BTreeMap;
use zellij_utils::data::OriginatingPlugin;
use zellij_utils::data::HOCUS_AGENT_PANE_CONTEXT;
use zellij_utils::data::{AgentInfo, AgentSource, AgentState};
use zellij_utils::input::hooks::HookPoint;

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

/// Hold `pane_id` with `run_command` and return every hook the screen asked for.
fn hooks_fired_by_holding(
    pane_id: u32,
    run_command: RunCommand,
) -> Vec<(HookPoint, PaneId, BTreeMap<String, String>)> {
    hooks_fired_by_holding_after(pane_id, run_command, None)
}

/// As above, with an agent record set on the pane before it exits - the road a pane that was
/// never spawned as an agent takes to becoming one, by its agent reporting for itself.
fn hooks_fired_by_holding_after(
    pane_id: u32,
    run_command: RunCommand,
    report: Option<AgentInfo>,
) -> Vec<(HookPoint, PaneId, BTreeMap<String, String>)> {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);

    let _ = mock_screen.to_screen.send(ScreenInstruction::NewPane(
        PaneId::Terminal(pane_id),
        Some("codex".to_string()),
        None,
        None,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(client_id),
        None,
        false,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    if let Some(agent) = report {
        let _ = mock_screen.to_screen.send(ScreenInstruction::SetPaneAgent(
            PaneId::Terminal(pane_id),
            Some(agent),
            None,
        ));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::HoldPane(
        PaneId::Terminal(pane_id),
        Some(0),
        run_command,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let fired = plugin_receiver
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
    mock_screen.teardown(vec![screen_thread]);
    fired
}

#[test]
fn an_exiting_agent_pane_fires_the_agent_exit_hook() {
    let fired = hooks_fired_by_holding(2, agent_command("/work/.worktrees/topic"));
    assert_eq!(fired.len(), 1, "expected exactly one hook, got: {fired:?}");
    let (point, pane_id, payload) = &fired[0];
    assert_eq!(*point, HookPoint::AgentExit);
    assert_eq!(*pane_id, PaneId::Terminal(2));
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
    let report = AgentInfo {
        kind: "claude".to_owned(),
        state: AgentState::Busy,
        source: AgentSource::Reported,
        detail: None,
        cwd: Some("/work".to_owned()),
        session_id: Some("s1".to_owned()),
    };
    let fired = hooks_fired_by_holding_after(3, typed_by_hand, Some(report));
    assert_eq!(fired.len(), 1, "expected exactly one hook, got: {fired:?}");
    assert_eq!(fired[0].0, HookPoint::AgentExit);
    assert_eq!(fired[0].1, PaneId::Terminal(3));
}

#[test]
fn an_ordinary_command_pane_fires_no_hook() {
    // The hook is for agent panes. An ordinary command pane still just holds, showing whatever
    // its command left on the screen - which is the behaviour that was there before agents were.
    let ordinary = RunCommand {
        command: PathBuf::from("echo"),
        args: vec!["hello".to_string()],
        hold_on_close: true,
        ..Default::default()
    };
    let fired = hooks_fired_by_holding(2, ordinary);
    assert!(fired.is_empty(), "expected no hook, got: {fired:?}");
}

#[test]
fn a_command_pane_from_a_plugin_that_is_not_an_agent_fires_no_hook() {
    // Having an originating plugin is not enough: it is the `hocus_agent_pane` marker in its
    // context that says this pane was an agent.
    let from_a_plugin = RunCommand {
        command: PathBuf::from("echo"),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(2, 1, BTreeMap::new())),
        ..Default::default()
    };
    let fired = hooks_fired_by_holding(2, from_a_plugin);
    assert!(fired.is_empty(), "expected no hook, got: {fired:?}");
}
