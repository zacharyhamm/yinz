use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

/// A plugin that has a pane is still the anchor: "near me" means beside it, and the fallback
/// must not quietly take over the case it was not written for.
#[test]
fn a_placement_near_a_pane_that_exists_resolves_to_that_pane() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(
        screen.resolve_pane_placement(ClientTabIndexOrPaneId::PaneIdOrClient(
            PaneId::Terminal(crate::TerminalId::new(1)),
            crate::ClientId::new(1)
        )),
        ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(1))),
    );
}

/// The case the variant exists for: a plugin loaded while no client was attached has no pane
/// of its own, so there is nothing to be near.
#[test]
fn a_placement_near_a_pane_that_does_not_exist_falls_back_to_the_client() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(
        screen.resolve_pane_placement(ClientTabIndexOrPaneId::PaneIdOrClient(
            PaneId::Plugin(crate::PluginId::new(999)),
            crate::ClientId::new(1)
        )),
        // NoFocus: the `PaneId` placement this stands in for does not steal focus either
        ClientTabIndexOrPaneId::ClientIdNoFocus(crate::ClientId::new(1)),
    );
}

/// Every other spelling is none of this resolution's business.
#[test]
fn other_placements_pass_through_untouched() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    for target in [
        ClientTabIndexOrPaneId::ClientId(crate::ClientId::new(1)),
        ClientTabIndexOrPaneId::ClientIdNoFocus(crate::ClientId::new(1)),
        ClientTabIndexOrPaneId::TabIndex(TabId::new(0)),
        ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(0)),
        ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(crate::PluginId::new(999))),
    ] {
        assert_eq!(screen.resolve_pane_placement(target), target);
    }
}

/// End to end, the failure the fallback exists to stop: the pty thread has already started the
/// process by the time the screen decides where it goes, so a placement with nowhere to land is
/// not an error the caller sees - it is `forget_unplaced_pane` closing the process behind its
/// back, one `log::error!` the only trace. With the fallback the pane is placed and lives.
#[test]
fn a_pane_opened_near_a_plugin_with_no_pane_is_placed_rather_than_closed() {
    let size = Size { cols: 80, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    let client_id = mock_screen.main_client_id;
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);

    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // No tab holds `Plugin(999)`: this is a plugin whose own `AddPlugin` never landed.
    let _ = mock_screen.to_screen.send(ScreenInstruction::NewPane(
        PaneId::Terminal(crate::TerminalId::new(9)),
        Some("spawned_by_a_headless_plugin".to_owned()),
        None,
        None,
        NewPanePlacement::default(),
        false, // start_suppressed
        ClientTabIndexOrPaneId::PaneIdOrClient(
            PaneId::Plugin(crate::PluginId::new(999)),
            client_id,
        ),
        None,
        false, // set_blocking
    ));

    // The pane's own title in a frame is the placement: a forgotten pane never renders.
    //
    // The wait has to come before the teardown, which sends `KillSession` straight to the server
    // channel and so races the render. Two things are done by hand rather than through
    // `wait_for_settled_render`: the bound is seconds rather than `default_timeout()`, because
    // that is 60s and nextest kills the process at 60s - a failing run would be an unexplained
    // timeout rather than a message - and the wait is caught so the threads are joined before it
    // is re-raised. One instruction produces this render; a correct run returns in milliseconds.
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        received_server_instructions.wait_until(
            "a render showing the pane the paneless plugin opened",
            std::time::Duration::from_secs(10),
            |events| {
                take_snapshots_and_cursor_coordinates_from_render_events(events.iter(), size)
                    .into_iter()
                    .find(|(_, snapshot)| snapshot.contains("spawned_by_a_headless_plugin"))
            },
        )
    }));

    mock_screen.teardown(vec![pty_thread, server_thread, screen_thread]);

    if let Err(panic) = rendered {
        std::panic::resume_unwind(panic);
    }

    let closed_by_pty: Vec<PaneId> = received_pty_instructions
        .lock()
        .iter()
        .filter_map(|instruction| match instruction {
            PtyInstruction::ClosePane(pane_id, _) => Some(*pane_id),
            _ => None,
        })
        .collect();
    assert!(
        closed_by_pty.is_empty(),
        "the pane was placed, not forgotten: {closed_by_pty:?}"
    );
}

fn startup_placement(stage: u8) {
    use crate::screen::thread_main::{replay_pending_pane_event, PendingEvents};
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let client = ClientId::new(1);
    let tab = TabId::new(0);
    let pane = PaneId::Terminal(TerminalId::new(9));
    let mut pending = PendingEvents::default();
    if stage > 0 {
        screen
            .new_tab(tab, (vec![], vec![]), None, Some(client))
            .unwrap();
    }
    if stage == 2 {
        screen
            .apply_layout(
                TiledPaneLayout::default(),
                vec![],
                vec![(TerminalId::new(1), None)],
                vec![],
                HashMap::new(),
                tab,
                true,
                client,
                None,
            )
            .unwrap();
    }
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    screen
        .place_new_pane(
            pane,
            Some("immediate CLI pane".into()),
            None,
            None,
            NewPanePlacement::default(),
            false,
            ClientTabIndexOrPaneId::ClientId(client),
            Some(crate::route::NotificationEnd::new(tx)),
            false,
            &mut pending,
        )
        .unwrap();
    if stage < 2 {
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "placement must wait for a live destination"
        );
        if stage == 0 {
            screen
                .new_tab(tab, (vec![], vec![]), None, Some(client))
                .unwrap();
        }
        screen
            .apply_layout(
                TiledPaneLayout::default(),
                vec![],
                vec![(TerminalId::new(1), None)],
                vec![],
                HashMap::new(),
                tab,
                true,
                client,
                None,
            )
            .unwrap();
        for event in std::mem::take(&mut pending.waiting_for_tab) {
            replay_pending_pane_event(&mut screen, event, &mut pending).unwrap();
        }
    }
    let reply = rx.try_recv().expect("completed placement");
    assert!(reply.error_message.is_none(), "{reply:?}");
    assert_eq!(reply.affected_pane_id, Some(pane));
    assert!(screen.tabs.get(&tab).unwrap().has_pane_with_pid(&pane));
}

#[test]
fn new_pane_before_initial_tab_waits_for_its_destination() {
    startup_placement(0);
}

#[test]
fn new_pane_during_initial_layout_survives_landing() {
    startup_placement(1);
}

#[test]
fn new_pane_after_initial_layout_completes_immediately() {
    startup_placement(2);
}

#[test]
fn deferred_startup_pane_is_closed_if_initial_destination_closes_or_fails() {
    use crate::screen::thread_main::{replay_pending_pane_event, PendingEvents};
    for failed in [false, true] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
        let (tx, instructions) = yinz_runtime::channels::unbounded();
        screen.bus.senders.to_pty = Some(yinz_runtime::channels::SenderWithContext::new(tx));
        let client = ClientId::new(1);
        let tab = TabId::new(0);
        let pane = PaneId::Terminal(TerminalId::new(9));
        let mut pending = PendingEvents::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        screen
            .place_new_pane(
                pane,
                None,
                None,
                None,
                NewPanePlacement::default(),
                false,
                ClientTabIndexOrPaneId::ClientId(client),
                Some(crate::route::NotificationEnd::new(tx)),
                false,
                &mut pending,
            )
            .unwrap();
        screen
            .new_tab(tab, (vec![], vec![]), None, Some(client))
            .unwrap();
        if failed {
            screen.recover_pending_tab(tab).unwrap();
        } else {
            screen.close_tab_by_id(tab).unwrap();
        }
        for event in std::mem::take(&mut pending.waiting_for_tab) {
            replay_pending_pane_event(&mut screen, event, &mut pending).unwrap();
        }
        assert!(rx.try_recv().unwrap().error_message.is_some());
        assert!(
            pending.waiting_for_tab.is_empty(),
            "closed destinations are not startup"
        );
        assert!(screen.tab_containing_pane(pane).is_none());
        assert!(instructions.try_iter().any(|(instruction, _)| matches!(instruction, PtyInstruction::ClosePane(id, _) if id == pane)), "rejected startup process must be closed");
    }
}

#[test]
fn cancelling_startup_before_any_layout_cannot_report_successful_placement() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let mut pending = crate::screen::thread_main::PendingEvents::default();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    screen
        .place_new_pane(
            PaneId::Terminal(TerminalId::new(9)),
            None,
            None,
            None,
            NewPanePlacement::default(),
            false,
            ClientTabIndexOrPaneId::ClientId(ClientId::new(1)),
            Some(crate::route::NotificationEnd::new(tx)),
            false,
            &mut pending,
        )
        .unwrap();
    drop(pending);
    assert!(
        rx.try_recv().unwrap().error_message.is_some(),
        "no live tab ever accepted the pane"
    );
}
