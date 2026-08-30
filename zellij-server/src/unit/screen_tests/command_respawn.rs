//! `RunCommandInPane`: running a *different* command in an existing command pane, and the race
//! it is built to survive - the replacement arriving before the screen has processed the pane's
//! hold. The pty reports an exit to the plugin and to the screen on different queues, so a
//! plugin advancing a sequence can answer the exit event while the `HoldPane` is still in
//! flight; the tab stashes the replacement and starts it when the hold lands.

use super::*;
use std::collections::BTreeMap;
use zellij_utils::data::OriginatingPlugin;

fn step_command(cmd: &str, step: &str) -> RunCommand {
    RunCommand {
        command: PathBuf::from(cmd),
        hold_on_close: true,
        originating_plugin: Some(OriginatingPlugin::new(
            1,
            1,
            BTreeMap::from([("__step".to_owned(), step.to_owned())]),
        )),
        ..Default::default()
    }
}

fn new_pane(pane_id: u32) -> ScreenInstruction {
    ScreenInstruction::NewPane(
        PaneId::Terminal(pane_id),
        Some("step1".to_string()),
        None,
        None,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(10),
        None,
        false,
    )
}

fn hold(pane_id: u32, run_command: RunCommand) -> ScreenInstruction {
    ScreenInstruction::HoldPane(PaneId::Terminal(pane_id), Some(0), run_command)
}

/// Drive the screen through `steps` and return every command the pty was asked to respawn, in
/// order. The screen drains its instructions in order and `teardown` waits for the last one, so
/// nothing here needs a sleep.
fn respawns_sent_to_pty(steps: Vec<ScreenInstruction>) -> Vec<RunCommand> {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    for step in steps {
        let _ = mock_screen.to_screen.send(step);
    }
    mock_screen.teardown(vec![screen_thread]);
    pty_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ReRunCommandInPane(_, run_command, _) => Some(run_command),
            _ => None,
        })
        .collect()
}

#[test]
fn a_held_pane_respawns_with_the_replacement_command() {
    let respawns = respawns_sent_to_pty(vec![
        new_pane(2),
        hold(2, step_command("step1", "1")),
        ScreenInstruction::RunCommandInPane(2, step_command("step2", "2"), true),
    ]);
    assert_eq!(
        respawns
            .iter()
            .map(|command| command.command.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("step2")],
        "the pty is asked to spawn the replacement, not the pane's own command"
    );
    assert_eq!(
        respawns[0]
            .originating_plugin
            .as_ref()
            .and_then(|origin| origin.context.get("__step"))
            .map(String::as_str),
        Some("2"),
        "the replacement's own context travels with it"
    );
}

#[test]
fn a_replacement_arriving_before_the_hold_waits_for_it() {
    // The race: the plugin answered the exit event before the screen processed the hold. The
    // pane is still "running" when the replacement arrives, so it is stashed - and starts the
    // moment the hold lands, not never.
    let respawns = respawns_sent_to_pty(vec![
        new_pane(2),
        ScreenInstruction::RunCommandInPane(2, step_command("step2", "2"), true),
        hold(2, step_command("step1", "1")),
    ]);
    assert_eq!(
        respawns
            .iter()
            .map(|command| command.command.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("step2")],
        "the stashed replacement runs when the pane is held"
    );
}

#[test]
fn a_second_replacement_wins_over_a_stashed_one() {
    let respawns = respawns_sent_to_pty(vec![
        new_pane(2),
        ScreenInstruction::RunCommandInPane(2, step_command("step2", "2"), true),
        ScreenInstruction::RunCommandInPane(2, step_command("step3", "3"), true),
        hold(2, step_command("step1", "1")),
    ]);
    assert_eq!(
        respawns
            .iter()
            .map(|command| command.command.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("step3")],
        "last write wins while the pane is still running"
    );
}

#[test]
fn closing_the_pane_drops_a_stashed_replacement() {
    let respawns = respawns_sent_to_pty(vec![
        new_pane(2),
        ScreenInstruction::RunCommandInPane(2, step_command("step2", "2"), true),
        ScreenInstruction::ClosePane(PaneId::Terminal(2), None, None, None),
        new_pane(3),
        hold(3, step_command("other", "1")),
    ]);
    assert!(
        respawns.is_empty(),
        "a stash does not outlive its pane, got: {respawns:?}"
    );
}
