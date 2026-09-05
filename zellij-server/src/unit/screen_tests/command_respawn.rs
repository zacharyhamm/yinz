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
        PaneId::Terminal(crate::TerminalId(pane_id)),
        Some("step1".to_string()),
        None,
        None,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(crate::ClientId(10)),
        None,
        false,
    )
}

fn hold(pane_id: u32, run_command: RunCommand) -> ScreenInstruction {
    ScreenInstruction::HoldPane(
        PaneId::Terminal(crate::TerminalId(pane_id)),
        Some(0),
        run_command,
    )
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
        ScreenInstruction::RunCommandInPane(crate::TerminalId(2), step_command("step2", "2"), true),
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
        ScreenInstruction::RunCommandInPane(crate::TerminalId(2), step_command("step2", "2"), true),
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
        ScreenInstruction::RunCommandInPane(crate::TerminalId(2), step_command("step2", "2"), true),
        ScreenInstruction::RunCommandInPane(crate::TerminalId(2), step_command("step3", "3"), true),
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
fn a_stashed_replacement_survives_a_cross_tab_pane_move() {
    // The pane is moved to another tab while its replacement is still stashed (the
    // pane's own command has not exited yet) - the stash must travel with the pane, or
    // the replacement is silently lost in the tab it left.
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    let client_id = mock_screen.main_client_id;

    // pane 2 is created in tab 0
    let _ = mock_screen.to_screen.send(new_pane(2));
    // A second tab, applied against its own stable id (unlike `MockScreen::new_tab`,
    // which always applies to tab 0) so it is not left `is_pending` forever.
    let second_tab_index = 1;
    let _ = mock_screen.to_screen.send(ScreenInstruction::NewTab(
        None,
        None,
        Some(TiledPaneLayout::default()),
        vec![],
        None,
        (Some(vec![]), Some(vec![])),
        None,
        false,
        true,
        client_id,
        None,
    ));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![],
        vec![(crate::TerminalId(0), None)],
        vec![],
        vec![],
        HashMap::new(),
        second_tab_index,
        true,
        client_id,
        None,
        None,
    ));
    // explicitly re-focus pane 2 (in tab 0) rather than relying on whichever pane
    // creation last happened to leave focused
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::FocusPaneWithId(
            PaneId::Terminal(crate::TerminalId(2)),
            false,
            false,
            client_id,
            None,
        ));
    // pane 2's command is still running - this stashes into tab 0's
    // pending_command_respawns rather than respawning immediately
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RunCommandInPane(
            crate::TerminalId(2),
            step_command("step2", "2"),
            true,
        ));
    // move the active pane (2) from tab 0 into tab 1
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneRight(client_id, None));
    // now the exit for pane 2's original command lands - wherever the pane now lives
    let _ = mock_screen
        .to_screen
        .send(hold(2, step_command("step1", "1")));

    mock_screen.teardown(vec![screen_thread]);
    let respawns: Vec<RunCommand> = pty_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ReRunCommandInPane(_, run_command, _) => Some(run_command),
            _ => None,
        })
        .collect();
    assert_eq!(
        respawns
            .iter()
            .map(|command| command.command.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("step2")],
        "the stashed replacement follows the pane into the tab it was moved to"
    );
}

#[test]
fn a_suppressed_pane_replacement_survives_its_tab_closing() {
    let mut screen = screen_with_a_pane_replaced_in_place();
    screen.tabs.get_mut(&0).unwrap().restore_pending_respawn(
        PaneId::Terminal(crate::TerminalId(1)),
        (step_command("step2", "2"), true),
    );

    screen.close_tab_by_id(0).unwrap();
    let (_, replacement, keep_scrollback) = screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .hold_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            Some(0),
            false,
            step_command("step1", "1"),
        )
        .expect("the replacement should follow the suppressed pane");

    assert_eq!(replacement.command, PathBuf::from("step2"));
    assert!(keep_scrollback);
}

#[test]
fn an_overlay_pane_replacement_survives_its_tab_closing() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    open_floating_pane(&mut screen, 0, 7, "overlay", true);
    screen.tabs.get_mut(&0).unwrap().restore_pending_respawn(
        PaneId::Terminal(crate::TerminalId(7)),
        (step_command("step2", "2"), true),
    );

    screen.close_tab_by_id(0).unwrap();
    let (_, replacement, keep_scrollback) = screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .hold_pane(
            PaneId::Terminal(crate::TerminalId(7)),
            Some(0),
            false,
            step_command("step1", "1"),
        )
        .expect("the replacement should follow the overlay pane");

    assert_eq!(replacement.command, PathBuf::from("step2"));
    assert!(keep_scrollback);
}

#[test]
fn closing_the_pane_drops_a_stashed_replacement() {
    let respawns = respawns_sent_to_pty(vec![
        new_pane(2),
        ScreenInstruction::RunCommandInPane(crate::TerminalId(2), step_command("step2", "2"), true),
        ScreenInstruction::ClosePane(PaneId::Terminal(crate::TerminalId(2)), None, None, None),
        new_pane(3),
        hold(3, step_command("other", "1")),
    ]);
    assert!(
        respawns.is_empty(),
        "a stash does not outlive its pane, got: {respawns:?}"
    );
}

fn session_reports(jobs: &[BackgroundJob]) -> usize {
    jobs.iter()
        .filter(|job| matches!(job, BackgroundJob::ReportSessionInfo(..)))
        .count()
}

/// Where a pane runs reaches plugins through the whole session manifest, so an `UpdatePaneHost`
/// that changes nothing is a broadcast that says nothing. Both no-change cases are ordinary:
/// a workflow step respawning on the host it was already on re-reports that host, and closing a
/// local pane retracts a host it never announced.
///
/// Counting broadcasts is safe here because this drives `Screen` directly - one call, one
/// decision - rather than waiting on the render scheduler to coalesce anything.
#[test]
fn only_a_changed_pane_host_is_broadcast() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, background_jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, 0);
    let _ = drain_background_jobs(&background_jobs);

    screen
        .set_pane_host(crate::TerminalId(1), Some("build-box".to_owned()), None)
        .unwrap();
    let jobs = drain_background_jobs(&background_jobs);
    assert_eq!(
        session_reports(&jobs),
        1,
        "learning that a pane runs elsewhere is news"
    );
    assert!(
        jobs.contains(&BackgroundJob::RenderToClients),
        "a changed host repaints its pane title: {jobs:?}"
    );

    screen
        .set_pane_host(crate::TerminalId(1), Some("build-box".to_owned()), None)
        .unwrap();
    let jobs = drain_background_jobs(&background_jobs);
    assert_eq!(
        session_reports(&jobs),
        0,
        "a respawn on the same host is not"
    );
    assert!(
        !jobs.contains(&BackgroundJob::RenderToClients),
        "an unchanged host needs no repaint: {jobs:?}"
    );

    screen
        .set_pane_host(crate::TerminalId(1), None, None)
        .unwrap();
    assert_eq!(
        session_reports(&drain_background_jobs(&background_jobs)),
        1,
        "a respawn back onto this machine is"
    );

    screen
        .set_pane_host(crate::TerminalId(1), None, None)
        .unwrap();
    assert_eq!(
        session_reports(&drain_background_jobs(&background_jobs)),
        0,
        "and a local pane's close retracts a host it never had"
    );
}
