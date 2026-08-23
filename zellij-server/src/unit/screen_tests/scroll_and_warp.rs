use super::*;
use zellij_test_support::Watched;

#[test]
pub fn scroll_position_is_preserved_across_move_focus() {
    let size = Size { cols: 80, rows: 10 };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let client_id = mock_screen.main_client_id;
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // Fill and scroll up the left pane (terminal 0), then leave Scroll mode: with the fix, the
    // scroll position must survive the mode change (this used to be unconditionally cleared).
    fill_pane_zero(&mock_screen);
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    let scroll_up = CliAction::ScrollUp { pane_id: None };
    for _ in 0..4 {
        send_cli_action_to_server(&session_metadata, scroll_up.clone(), client_id);
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Normal,
        None,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Move focus away to the right pane and back to the left (scrolled) pane. A MoveFocus always
    // leaves Scroll mode first, which is exactly the path that used to reset the scroll position.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Left,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let (_cursor_position, last_snapshot) = snapshots.last().unwrap();
    assert!(
        last_snapshot.contains("SCROLL:"),
        "the left pane should still show a scroll indicator after moving focus away and back, got:\n{}",
        last_snapshot
    );
    assert!(
        !last_snapshot.contains("something 19"),
        "the left pane should still be scrolled up (the last line should not be visible) after moving focus away and back, got:\n{}",
        last_snapshot
    );
}

#[test]
pub fn focusing_a_scrolled_pane_enters_scroll_mode() {
    assert_focus_change_syncs_scroll_mode(InputMode::Normal);
}

#[test]
pub fn focusing_a_scrolled_pane_enters_scroll_mode_with_locked_default() {
    // Unlock-first users have Locked as their default mode; leaving a scrolled pane must return
    // them to Locked (not Normal), and Scroll is entered from Locked.
    assert_focus_change_syncs_scroll_mode(InputMode::Locked);
}

// ---------------------------------------------------------------------------
// Warp eligibility
// ---------------------------------------------------------------------------
//
// `check_pane_can_be_warped` is the only place a warp can be refused cheaply.
// Everything it does not catch is caught after the supervisor has been released,
// when the pane belongs to nobody and the refusal costs a recovery step, so each
// of these is worth pinning down.

#[test]
fn a_plugin_pane_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Plugin(1)), 1),
        Err("this is a plugin pane; it has no process, so there is nothing to move".to_owned())
    );
}

#[test]
fn a_pane_from_another_session_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(99)), 1),
        Err("there is no pane 99 in this session".to_owned())
    );
}

/// What a global pane is told when someone tries to move it out of the session.
///
/// Its own constant because four tests assert it, and the wording is the whole
/// point: "there is no pane 99 in this session" was true of the tab lookup and
/// useless to the person reading it.
const GLOBAL_PANE_WARP_REFUSAL: &str = "this is a global pane; it belongs to the whole session \
                                        rather than to a tab, so there is no other session to \
                                        move it into";

#[test]
fn a_global_pane_cannot_be_warped() {
    // A global pane is docked to the session, not held by a tab, so there is
    // nothing about it that could arrive somewhere else.
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(99)), 1),
        Err(GLOBAL_PANE_WARP_REFUSAL.to_owned())
    );
}

#[test]
fn a_split_inside_a_global_pane_cannot_be_warped() {
    // A split of a global pane is itself a global pane, so one predicate has to
    // cover both - which is why this asks about the split rather than the anchor.
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_split(&mut screen, 98, PaneId::Terminal(99));

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(98)), 1),
        Err(GLOBAL_PANE_WARP_REFUSAL.to_owned())
    );
}

#[test]
fn a_client_focused_on_a_global_pane_warps_nothing_by_accident() {
    // The dangerous one. Asked for "the focused pane" with a global pane focused,
    // resolving focus through the tab answers with a pane the user is not even
    // looking at - and warping it moves it to another session for them.
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen
        .focus_global_pane(PaneId::Terminal(99), 1)
        .expect("TEST");

    assert_eq!(
        screen.check_pane_can_be_warped(None, 1),
        Err(GLOBAL_PANE_WARP_REFUSAL.to_owned())
    );
}

#[test]
fn a_global_plugin_pane_is_refused_as_a_global_pane_not_as_a_plugin() {
    // Most global panes are plugin panes, so the order of the two guards decides
    // which explanation the user gets. "it has no process" is true and unhelpful:
    // it suggests the pane is the wrong kind, when the problem is where it lives.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/hocus.wasm", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Plugin(77)), 1),
        Err(GLOBAL_PANE_WARP_REFUSAL.to_owned())
    );
}

#[test]
fn an_adopted_pane_lands_in_the_tab_when_the_client_is_on_a_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen
        .focus_global_pane(PaneId::Terminal(99), client_id)
        .expect("TEST");

    assert_eq!(
        adopted_pane_placement_target(
            &mut screen,
            zellij_utils::data::WarpPlacement::FocusedTab,
            client_id
        ),
        ClientTabIndexOrPaneId::TabIndex(0),
        "an adopted pane must name the tab rather than the client, or it splits the global pane",
    );
}

#[test]
fn an_adopted_floating_pane_lands_in_the_tab_when_the_client_is_on_a_global_pane() {
    // No `NewPane` reaches the global area any more, whatever its placement.
    // Pinned here from the warp side as well, because an adopted pane picks its
    // target before the pane-creation path ever sees it.
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen
        .focus_global_pane(PaneId::Terminal(99), client_id)
        .expect("TEST");

    assert_eq!(
        adopted_pane_placement_target(
            &mut screen,
            zellij_utils::data::WarpPlacement::Floating,
            client_id
        ),
        ClientTabIndexOrPaneId::TabIndex(0),
    );
}

#[test]
fn an_adopted_pane_still_follows_a_client_looking_at_a_tab() {
    // The fallback is only for a client parked on a global pane. Everyone else
    // keeps the placement they had, which is what makes the pane arrive in the
    // tab they are actually looking at rather than the one they started in.
    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        adopted_pane_placement_target(
            &mut screen,
            zellij_utils::data::WarpPlacement::FocusedTab,
            client_id
        ),
        ClientTabIndexOrPaneId::ClientId(client_id),
    );
}

#[test]
fn an_adopted_tab_takes_a_client_off_a_global_pane() {
    // A whole tab arriving moves the client into it. The pane placements have to
    // *name* the tab to stay out of the global strip; a tab is built by
    // `new_tab`, which moves the client itself - so what this has to check is not
    // where the panes go but that the client is looking at them when they arrive.
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen
        .focus_global_pane(PaneId::Terminal(99), client_id)
        .expect("TEST");
    assert_eq!(
        screen.global_panes.focused_pane_id(client_id),
        Some(PaneId::Terminal(99)),
    );

    // The result is not asserted: the panes are set aside on the pty thread and
    // filled in by a later instruction, so a tab adopted against these empty
    // channels never finishes arriving. The focus is handed over before any of
    // that, which is the whole point of asking here.
    let _ = screen.place_adopted_tab(
        zellij_utils::data::WarpedTab::default(),
        crate::pty::AdoptedTabPanes {
            tiled: vec![],
            floating: vec![],
        },
        client_id,
    );

    assert_eq!(
        screen.global_panes.focused_pane_id(client_id),
        None,
        "the client would have been looking at the global pane while its new tab arrived",
    );
}

#[test]
fn a_session_whose_other_terminals_are_all_global_is_still_down_to_its_last_pane() {
    // `selectable_terminal_pane_count` asks the tabs, and global panes are held
    // by the session rather than by a tab - so a session hung with global
    // terminals is still a session with one pane in it. Warping that pane away
    // would leave nothing a tab could focus, which is the closing this guard
    // exists to refuse.
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    for (id, edge) in [
        (97, GlobalPaneEdge::Left),
        (98, GlobalPaneEdge::Right),
        (99, GlobalPaneEdge::Top),
    ] {
        gp_insert_global_pane(&mut screen, id, edge, PercentOrFixed::Fixed(8), false);
    }

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Err("this is the session's only pane; moving it would close the session".to_owned()),
    );
}

#[test]
fn the_visible_half_of_a_scrollback_editor_pair_can_be_warped() {
    // Warping what you are looking at: the editor goes, and the pane it was
    // editing comes back into the slot the editor was holding for it - which is
    // what closing the editor would have done anyway.
    let mut screen = screen_with_a_scrollback_editor_open();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(9)), 1),
        Ok(9)
    );

    released_pane_for_warp(&mut screen, 9);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert!(
        tab.get_static_and_floating_pane_ids()
            .contains(&PaneId::Terminal(1)),
        "the pane being edited is back in the grid",
    );
    assert_eq!(
        tab.get_suppressed_panes().count(),
        0,
        "and nothing is left suppressed behind a pane that has gone",
    );
}

#[test]
fn a_release_that_failed_leaves_the_pane_where_it_was() {
    // Screen is told about a release either way, and a release that did not
    // happen must not take the pane out of its tab: on the refusal path the pane
    // is untouched and still working, and on the failure path the pty thread has
    // already held it and written the reason into it. Either way, taking it out
    // here would be Screen deciding on its own that the pane is gone.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let result =
        screen.take_released_pane_for_warp(1, Err("pane 1 has no pty-host supervisor".to_owned()));

    assert_eq!(
        result.expect_err("a pane that was not released cannot be serialized"),
        "pane 1 has no pty-host supervisor",
        "the reason has to reach the caller unreworded",
    );
    assert!(
        screen
            .tabs
            .values()
            .any(|tab| tab.get_pane_with_id(PaneId::Terminal(1)).is_some()),
        "the pane was taken out of its tab by a warp that never happened",
    );
}

#[test]
fn the_hidden_half_of_a_scrollback_editor_pair_can_be_warped() {
    // The pane the user actually means when they name terminal 1. It leaves, and
    // the editor goes with it: an editor whose subject is in another session now
    // is a dump of a pane that no longer exists.
    let mut screen = screen_with_a_scrollback_editor_open();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );

    released_pane_for_warp(&mut screen, 1);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert_eq!(
        tab.get_all_pane_ids(),
        Vec::<PaneId>::new(),
        "both halves of the pair are gone",
    );
}

#[test]
fn warping_a_pane_being_edited_closes_the_editor_on_the_pty_thread_too() {
    // The editor is being closed rather than moved, so unlike the warped pane it
    // has to be reported to the pty thread - otherwise its supervisor and its
    // child outlive the pane.
    let mut screen = screen_with_a_scrollback_editor_open();
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    released_pane_for_warp(&mut screen, 1);

    let closed_by_pty: Vec<PaneId> = pty_receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PtyInstruction::ClosePane(pane_id, _) => vec![pane_id],
            _ => vec![],
        })
        .collect();
    assert_eq!(
        closed_by_pty,
        vec![PaneId::Terminal(9)],
        "the editor is closed; the warped pane is handed over, never closed",
    );
}

#[test]
fn warping_a_pane_replaced_in_place_leaves_the_pane_that_replaced_it() {
    // Not the editor case: the stand-in has its own process and its own reason to
    // be on screen. It stays, and simply stops restoring anything.
    let mut screen = screen_with_a_pane_replaced_in_place();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );

    released_pane_for_warp(&mut screen, 1);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert_eq!(
        tab.get_all_pane_ids(),
        vec![PaneId::Terminal(9)],
        "the pane that replaced it is still here, and nothing is suppressed behind it",
    );
}

#[test]
fn a_pane_a_plugin_hid_can_be_warped() {
    // Parked under its own id with nothing standing in for it: it is on screen
    // nowhere, so nothing on screen changes when it leaves.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&1)
        .expect("TEST")
        .suppress_pane(PaneId::Terminal(2), None);
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(2)), 1),
        Ok(2)
    );

    released_pane_for_warp(&mut screen, 2);

    assert_eq!(
        screen.tabs.get(&1).expect("TEST").get_all_pane_ids(),
        Vec::<PaneId>::new(),
    );
}

#[test]
fn a_pane_and_the_editor_open_on_it_cannot_be_the_last_two_panes_warped() {
    // The pair would take the whole session with it: the editor is closed rather
    // than left behind, so warping the pane empties the session.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .replace_pane_with_editor_pane(PaneId::Terminal(9), PaneId::Terminal(1))
        .expect("TEST");

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Err(
            "this pane and the scrollback editor open on it are the session's last panes; \
             moving it would close the session"
                .to_owned()
        )
    );
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(9)), 1),
        Ok(9),
        "the editor alone can still go: the pane it was editing is left behind",
    );
}

#[test]
fn the_sessions_only_pane_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Err("this is the session's only pane; moving it would close the session".to_owned())
    );
}

#[test]
fn a_terminal_pane_with_somewhere_to_leave_behind_can_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );
}

#[test]
fn a_tabs_last_pane_can_still_be_warped() {
    // Deliberately allowed: the render pass sweeps a tab with nothing selectable
    // left in it, so emptying a tab is a tab closing, not a session ending.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.tabs.get(&1).expect("TEST").get_all_pane_ids().len(),
        1
    );
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(2)), 1),
        Ok(2)
    );
}

// ---------------------------------------------------------------------------
// Remote panes
// ---------------------------------------------------------------------------
//
// A remote pane's process is on another machine, so there is no local supervisor
// socket to hand over. What travels instead is the broker's address for it. The
// point of these is that Screen builds a payload for one at all: before this, a
// remote pane failed the release with "no pty-host supervisor", which is a true
// statement about this machine and a misleading one about the pane.

/// A remote pane released for a warp, as the pty thread would report one.
fn released_remote_pane(screen: &mut Screen, terminal_id: u32) -> Box<WarpedPane> {
    screen
        .take_released_pane_for_warp(
            terminal_id,
            Ok(ReleasedPane::Remote(ReleasedRemotePane {
                host: "build-box".to_owned(),
                pane_token: format!("token-{terminal_id}"),
                received: 4096,
                cmd: RunCommand {
                    command: PathBuf::from("/usr/bin/cargo"),
                    args: vec!["watch".to_owned()],
                    remote_host: Some("build-box".to_owned()),
                    ..Default::default()
                },
            })),
        )
        .expect("a remote pane should have been taken out of its tab")
}

#[test]
fn a_remote_pane_travels_as_a_broker_address_rather_than_a_socket() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let payload = released_remote_pane(&mut screen, 1);

    let remote = payload
        .remote
        .as_ref()
        .expect("a remote pane must carry the broker address that is its only way home");
    assert_eq!(remote.host, "build-box");
    assert_eq!(remote.pane_token, "token-1");
    assert_eq!(
        remote.received, 4096,
        "the byte count must survive: the broker replays from exactly here, and the pane's \
         serialized screen already holds everything before it"
    );
    assert_eq!(
        remote.cmd.remote_host.as_deref(),
        Some("build-box"),
        "the command travels with the pane, because a resume carries none — without it the pane \
         cannot title itself or re-run on the right machine"
    );
    assert!(
        payload.host_id.is_empty() && payload.control_socket.as_os_str().is_empty(),
        "the local supervisor fields must stay empty rather than plausible: a path here would be \
         one the adopting session might try to attach to"
    );
}

#[test]
fn a_remote_pane_leaves_its_tab_like_any_other() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let somewhere = |screen: &Screen| {
        screen
            .tabs
            .values()
            .any(|tab| tab.get_pane_with_id(PaneId::Terminal(1)).is_some())
    };
    assert!(somewhere(&screen));

    released_remote_pane(&mut screen, 1);

    assert!(
        !somewhere(&screen),
        "a warped remote pane must be taken out of its tab, or it is on screen in two sessions"
    );
}

// ---------------------------------------------------------------------------
// Tab warp eligibility
// ---------------------------------------------------------------------------
//
// The same argument as above, with more at stake: what a tab warp releases
// before it can discover a problem is not one supervisor but every supervisor in
// the tab.

#[test]
fn a_refused_duplicate_tab_warp_does_not_strand_the_still_attached_panes() {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(None, vec![]);
    let (first_response_tx, _first_response_rx) = crossbeam::channel::unbounded();
    let (second_response_tx, second_response_rx) = crossbeam::channel::unbounded();

    for response_channel in [first_response_tx, second_response_tx] {
        mock_screen
            .to_screen
            .send(ScreenInstruction::WarpTab {
                tab_index: Some(0),
                client_id,
                response_channel,
            })
            .unwrap();
    }

    let mut releases = 0;
    while releases < 2 {
        if matches!(
            pty_receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("both tab warps should reach the pty thread")
                .0,
            PtyInstruction::ReleaseTerminalsForWarp { .. }
        ) {
            releases += 1;
        }
    }

    let refused = vec![(0, Err("the tab's pane cannot be moved".to_owned()))];
    mock_screen
        .to_screen
        .send(ScreenInstruction::TabReleasedForWarp {
            tab_index: 0,
            released: refused.clone(),
        })
        .unwrap();
    assert!(second_response_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the newer request should receive the first refusal")
        .is_err(),);

    mock_screen
        .to_screen
        .send(ScreenInstruction::TabReleasedForWarp {
            tab_index: 0,
            released: refused,
        })
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpTab {
            tab_index: Some(0),
            client_id,
            response_channel: crossbeam::channel::unbounded().0,
        })
        .unwrap();

    loop {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the follow-up warp should run after both refused replies");
        assert!(
            !matches!(instruction, PtyInstruction::StrandPaneAfterWarp { .. }),
            "a pane whose release was refused is still attached and must not be stranded",
        );
        if matches!(instruction, PtyInstruction::ReleaseTerminalsForWarp { .. }) {
            break;
        }
    }

    mock_screen.teardown(vec![screen_thread]);
}

#[test]
fn a_tab_that_does_not_exist_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(99), 1).err(),
        Some("there is no tab 99 in this session".to_owned())
    );
}

#[test]
fn a_tab_with_a_scrollback_editor_open_cannot_be_warped() {
    // Refused for the same reason a single pane with one open is: the
    // suppressed-pane bookkeeping is keyed by the visible pane, so closing the
    // pair hands over the wrong pty.
    let mut screen = screen_with_a_scrollback_editor_open();

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

#[test]
fn a_tab_holding_a_pane_a_plugin_hid_cannot_be_warped() {
    // Parked with nothing standing in for it, like a stack-list member - and
    // unlike one, with no slot in the layout the tab travels as. A single such
    // pane can be warped on its own (`a_pane_a_plugin_hid_can_be_warped`), because
    // then it is the thing being moved; taking the tab out from under it would
    // release a supervisor nobody at the far end is holding.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(1),
        None,
    )
    .expect("TEST");
    tab.suppress_pane(PaneId::Terminal(3), None);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

#[test]
fn a_tab_holding_a_hidden_floating_pane_cannot_be_warped() {
    // The same refusal, asked of the one kind of pane that can never be the
    // exception to it: a stack is a shape the *tiled* layout takes, so a hidden
    // floating pane is a hidden pane and nothing else.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(1),
        None,
    )
    .expect("TEST");
    tab.suppress_pane(PaneId::Terminal(3), None);
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "a floating pane is not a stack-list member, whatever else it is",
    );

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

#[test]
fn the_sessions_only_tab_can_be_warped() {
    // The pane version refuses the session's last pane, because moving it would
    // end the session by accident. Asking for the last *tab* is asking for
    // everything on purpose, so it is allowed and the session then ends.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(screen.tabs.len(), 1);
    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("the only tab should still be movable");
    assert_eq!((tab_index, terminal_ids), (0, vec![1]));
}

#[test]
fn warping_a_tab_takes_every_pane_that_has_a_process() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(1), 1)
        .expect("the tab should be movable");
    assert_eq!(tab_index, 1);
    assert_eq!(terminal_ids, vec![2]);
}

#[test]
fn a_tab_warp_defaults_to_the_focused_tab() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.go_to_tab(1, 1).expect("TEST");

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(None, 1)
        .expect("the focused tab should be movable");
    assert_eq!((tab_index, terminal_ids), (0, vec![1]));
}

#[test]
fn a_tab_that_can_be_warped_has_already_had_its_arrangement_worked_out() {
    // The ordering that matters most here. Rebuilding a tab's layout from its
    // geometry can fail, and it happens during this check — while the tab is
    // still a tab — rather than after its supervisors have been let go, when
    // failing would leave a tab on screen with nothing attached to it.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let (_, terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(1), 1)
        .expect("the tab should be movable");
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn a_tab_of_nothing_but_plugin_panes_has_nothing_to_warp() {
    // Not a refusal on principle — a plugin pane rides along in the layout and is
    // restarted at the far end. But a tab made of nothing else has no process to
    // move, so moving it would be an elaborate way of opening a new tab.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.close_pane(PaneId::Terminal(1), false, None);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some("this tab has no panes with a process to move".to_owned())
    );
}

#[test]
fn a_tab_whose_ui_strip_reaches_past_it_can_be_warped() {
    // The shape this was reported as: with a global pane docked to one side of
    // the session, the tab's ui strips are widened over it while the panes
    // between them stay inset, so the tab is not a rectangle and no split line
    // spans it. That is not a layout, and it used to be refused - "0 of its 4
    // panes could be placed", with every process in the tab stuck in this
    // session over the shape of a status bar.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    for pane_id in [2, 3] {
        tab.new_pane(
            PaneId::Terminal(pane_id),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .expect("TEST");
    }
    // Terminal 2 is the tab's full-width bottom strip. Widened over the global
    // pane's eight columns the way `expand_ui_panes_over_global_strips` widens a
    // ui pane - written here rather than driven through a render, because what is
    // being tested is what a tab in that shape does, not when the shape appears.
    tab.set_pane_selectable(PaneId::Terminal(2), false);
    let strip = tab.get_pane_with_id_mut(PaneId::Terminal(2)).expect("TEST");
    let mut widened = strip.position_and_size();
    let reached_over = 8;
    assert_eq!(
        widened.x, 0,
        "terminal 2 is expected to be the tab's full-width bottom strip",
    );
    let cols = widened.cols.as_usize() + reached_over;
    widened.cols.set_inner(cols);
    strip.set_geom(widened);
    for pane_id in [PaneId::Terminal(1), PaneId::Terminal(3)] {
        let pane = tab.get_pane_with_id_mut(pane_id).expect("TEST");
        let mut inset = pane.position_and_size();
        inset.x += reached_over;
        pane.set_geom(inset);
    }

    // the premise: the ui strip reaches past the panes under it, so the tab is
    // not a rectangle and no layout describes it
    let tab = screen.tabs.get(&0).expect("TEST");
    let strip = tab
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    let leftmost_pane_under_it = tab
        .get_tiled_panes()
        .filter(|(pane_id, _)| **pane_id != PaneId::Terminal(2))
        .map(|(_, pane)| pane.position_and_size().x)
        .min()
        .expect("TEST");
    assert!(
        strip.x < leftmost_pane_under_it,
        "a ui pane beside a global pane should reach past the panes under it \
         (strip at {}, panes at {leftmost_pane_under_it})",
        strip.x,
    );

    let (tab_index, mut terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("a tab whose ui strip reaches past it should still be movable");
    assert_eq!(tab_index, 0);
    terminal_ids.sort_unstable();
    assert_eq!(
        terminal_ids,
        vec![1, 2, 3],
        "every pane in the tab should travel",
    );
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn a_hidden_stack_list_member_can_be_warped() {
    // Parked under its own id with a slot of its own: it leaves the list the way
    // closing it would, and the members that stay are undisturbed.
    let mut screen = screen_with_a_stack_list();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );

    released_pane_for_warp(&mut screen, 1);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(1)),
        "the warped member is out of the list",
    );
    assert!(
        tab.pane_is_stack_list_member(&PaneId::Terminal(2))
            && tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "the other two are still a list",
    );
    assert!(
        tab.pane_is_hidden_stack_list_member(&PaneId::Terminal(2)),
        "and the member that was visible still is",
    );
}

#[test]
fn warping_the_visible_stack_list_member_promotes_a_hidden_one() {
    // Nothing is left holding the list's slot in the grid, so a member has to be
    // put there - the same promotion closing the visible member does.
    let mut screen = screen_with_a_stack_list();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(3)), 1),
        Ok(3)
    );

    released_pane_for_warp(&mut screen, 3);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert!(
        tab.get_static_and_floating_pane_ids()
            .contains(&PaneId::Terminal(2)),
        "a hidden member was promoted into the grid",
    );
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "and the warped member is out of the list",
    );
}

#[test]
fn a_stack_list_can_be_warped_away_a_member_at_a_time_until_one_is_left() {
    // The members are the session's panes even though only one of them is on
    // screen, so the count that decides "this would close the session" has to see
    // them: refusing at three panes left would refuse a whole tab's worth of work.
    let mut screen = screen_with_only_a_stack_list();

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );
    released_pane_for_warp(&mut screen, 1);
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(2)), 1),
        Ok(2)
    );
    released_pane_for_warp(&mut screen, 2);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(3)), 1),
        Err("this is the session's only pane; moving it would close the session".to_owned()),
        "the last member is the last pane",
    );
}

#[test]
fn warping_a_hidden_member_that_is_an_editor_hands_its_place_to_the_pane_it_was_editing() {
    // The editor holds nobody's place while it is off screen, so this is not a
    // swap: pane 3 has no slot to be put back into, and putting it in one the
    // editor does not have would lose it. It takes the editor's place in the list
    // instead, which is what closing the editor there does.
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(8)), 1),
        Ok(8)
    );

    released_pane_for_warp(&mut screen, 8);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert!(
        tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "the pane that was being edited is a member of the list now",
    );
    assert!(
        tab.get_suppressed_panes()
            .any(|(pane_id, suppressed)| *pane_id == PaneId::Terminal(3)
                && suppressed.restored_by.is_none()),
        "and it is parked in its own right, with nothing standing in for it",
    );
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(8)),
        "while the editor has gone",
    );
}

#[test]
fn warping_a_pane_edited_by_a_hidden_member_takes_that_member_with_it() {
    // The other direction: the pane leaves, so the editor of it is closed - and
    // because the editor was a stack-list member, the list has to lose it too
    // rather than be left pointing at a pane that has gone.
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(3)), 1),
        Ok(3)
    );

    released_pane_for_warp(&mut screen, 3);

    let tab = screen.tabs.get(&0).expect("TEST");
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(8))
            && !tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "neither half of the pair is in the list",
    );
    assert!(
        tab.pane_is_stack_list_member(&PaneId::Terminal(1))
            && tab.pane_is_stack_list_member(&PaneId::Terminal(2)),
        "the members that had nothing to do with it are untouched",
    );
}

#[test]
fn a_stack_list_member_and_the_pane_it_stands_in_for_are_each_keyed_by_themselves() {
    // The case `suppressed_panes` used to need a side table for: a scrollback
    // editor is opened on a member, which makes the editor's id the member id, and
    // then another member is selected, so the editor is suppressed while still
    // standing in for the pane it is editing. Both are in the map at once, and
    // each is under its own id - the association is in `restored_by`, so neither
    // has to be re-keyed when either of them moves.
    let mut screen = screen_with_a_stack_list();
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.replace_pane_with_editor_pane(PaneId::Terminal(8), PaneId::Terminal(3))
        .expect("TEST");
    tab.focus_hidden_stack_list_member(PaneId::Terminal(1), 1);

    let suppressed: HashMap<PaneId, Option<PaneId>> = tab
        .get_suppressed_panes()
        .map(|(pane_id, suppressed)| {
            assert_eq!(
                *pane_id,
                suppressed.pane.pid(),
                "every suppressed pane is keyed by its own id",
            );
            (
                *pane_id,
                suppressed.restored_by.map(|stand_in| stand_in.pane_id),
            )
        })
        .collect();

    assert_eq!(
        suppressed.get(&PaneId::Terminal(8)),
        Some(&None),
        "the editor took the member's place in the list and is parked as a member",
    );
    assert_eq!(
        suppressed.get(&PaneId::Terminal(3)),
        Some(&Some(PaneId::Terminal(8))),
        "the pane being edited is suppressed behind the editor, wherever the editor is",
    );
    assert!(
        tab.pane_is_stack_list_member(&PaneId::Terminal(8))
            && !tab.pane_is_stack_list_member(&PaneId::Terminal(3)),
        "the editor is the member; the pane behind it is not",
    );
}

#[test]
fn a_tab_with_a_stack_list_can_be_warped() {
    let mut screen = screen_with_a_stack_list();

    let (tab_index, mut terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("a tab holding a stack list should be movable");
    terminal_ids.sort_unstable();
    assert_eq!(
        (tab_index, terminal_ids),
        (0, vec![1, 2, 3]),
        "every member travels, including the ones hidden behind the visible one",
    );
}

#[test]
fn a_tab_whose_stack_list_member_is_being_edited_cannot_be_warped() {
    // The case the refusal is really for, in the middle of the thing the refusal
    // makes an exception for. A stack-list member with a scrollback editor open on
    // it is a pair: the editor is the member and travels, but the pane it is
    // editing is parked behind it with no slot of its own. Letting the tab go
    // would dismantle it around a pty nobody released.
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

#[test]
fn a_stack_lists_synthetic_geoms_describe_one_stack_filling_the_visible_panes_rect() {
    // The seam between two well-tested halves. `warped_tab`'s properties start
    // from manifests that already have a stack's shape, and the tab-warp tests ask
    // only that every member has a slot - so what a real `Tab` hands the layout
    // builder is checked by neither. The members are not in the grid, so their
    // geometry is synthesised here or nowhere.
    let screen = screen_with_a_stack_list();
    let tab = screen.tabs.get(&0).expect("TEST");
    let in_the_grid: Vec<PaneId> = tab.get_tiled_panes().map(|(pane_id, _)| *pane_id).collect();
    assert_eq!(
        in_the_grid,
        vec![PaneId::Terminal(3)],
        "the open member is the only one holding the list's slot",
    );
    let full_rect = tab
        .get_tiled_panes()
        .next()
        .expect("TEST")
        .1
        .position_and_size();

    let geoms = tab.stack_list_serialization_geoms();
    let members = [
        PaneId::Terminal(1),
        PaneId::Terminal(2),
        PaneId::Terminal(3),
    ];
    assert_eq!(geoms.len(), members.len(), "every member gets a geom");
    let stack_id = geoms[&members[0]].stacked;
    assert!(stack_id.is_some(), "the members travel as a stack");

    let mut expected_y = full_rect.y;
    for member in members {
        let geom = geoms[&member];
        assert_eq!(
            geom.stacked, stack_id,
            "{member:?} is in a stack of its own",
        );
        assert_eq!((geom.x, geom.cols), (full_rect.x, full_rect.cols));
        assert_eq!(
            geom.y, expected_y,
            "{member:?} does not start where the member above it ended",
        );
        if member == PaneId::Terminal(3) {
            assert_eq!(
                geom.rows.as_usize(),
                full_rect.rows.as_usize() - 2,
                "the open member should take what the two collapsed ones leave",
            );
        } else {
            assert!(
                geom.rows.is_fixed() && geom.rows.as_usize() == 1,
                "{member:?} should be collapsed to its title row, but has {:?}",
                geom.rows,
            );
        }
        expected_y += geom.rows.as_usize();
    }
    // Which member is open is not carried as a flag - it is carried as the one
    // member taller than a title row - so the rows have to add up to the rect or
    // the stack arrives either overflowing it or short of it.
    assert_eq!(
        expected_y,
        full_rect.y + full_rect.rows.as_usize(),
        "the stack should occupy exactly the rect the open member had",
    );
}

#[test]
fn a_warped_tab_carries_its_hidden_stack_list_members_in_its_layout() {
    // The invariant that makes the warp safe: a released supervisor with no slot
    // in the layout is a process nobody would be holding at the far end.
    let mut screen = screen_with_a_stack_list();

    let (_, terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("a tab holding a stack list should be movable");
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
    // Slot order, not tab order: the receiving session fills its slots in this
    // order, so this is the members arriving top to bottom the way they were
    // stacked.
    assert_eq!(
        prepared.tiled_panes,
        vec![
            PaneId::Terminal(1),
            PaneId::Terminal(2),
            PaneId::Terminal(3),
        ],
        "the members should have slots in the order they were stacked",
    );

    // And the slots should be a stack rather than three panes that happen to be
    // adjacent - a layout is what the far end rebuilds from, so a stack that
    // arrived as an ordinary split would be a tab the user did not send.
    let stack = &prepared.tiled_layout;
    assert!(
        stack.children_are_stacked,
        "the tab's one node should be the stack",
    );
    assert_eq!(stack.children.len(), 3, "with a child per member");
    let expanded: Vec<usize> = stack
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.is_expanded_in_stack)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        expanded,
        vec![2],
        "exactly one member is open, and it is the one that was open here",
    );
}

#[test]
fn warping_a_tab_does_not_kill_the_children_of_its_hidden_stack_list_members() {
    // The whole point of emptying a tab before closing it: `close_tab_by_id` ends
    // by telling the pty thread to close whatever panes are left, and that SIGHUPs
    // their children. A hidden stack-list member left parked would be in that
    // list - and its child is a process this session has just handed to another
    // one, so killing it kills a pane the far end is about to show.
    let mut screen = screen_with_a_stack_list();
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    screen.empty_and_close_tab(0);

    let closed_by_pty: Vec<PaneId> = pty_receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PtyInstruction::CloseTab(pane_ids) => pane_ids,
            PtyInstruction::ClosePane(pane_id, _) => vec![pane_id],
            _ => vec![],
        })
        .collect();
    assert_eq!(
        closed_by_pty,
        Vec::<PaneId>::new(),
        "a warped tab hands its processes over; none of them may be closed here",
    );
}

/// A tab with a tiled pane and a terminal overlay above it.
fn screen_with_an_overlay_over_a_tab() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(9),
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(1),
            None,
        )
        .expect("TEST");
    screen
}

#[test]
fn a_warped_tab_leaves_its_overlay_behind() {
    // An overlay does not travel: there is no slot for it in a layout, and a
    // terminal overlay's overlay-ness could not be re-derived at the far end even
    // if there were - it is not written in a plugin configuration anywhere. So it
    // is neither released nor carried, and the tab arrives without it.
    let mut screen = screen_with_an_overlay_over_a_tab();

    let (_, terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("a tab with an overlay over it should still be movable");

    assert!(
        !terminal_ids.contains(&9),
        "the overlay's supervisor must not be released, since nobody at the far \
         end would be holding it: {terminal_ids:?}",
    );
    assert!(
        !prepared
            .tiled_panes
            .iter()
            .chain(prepared.floating_panes.iter())
            .any(|pane_id| *pane_id == PaneId::Terminal(9)),
        "and it has no slot in the arrangement either",
    );
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn warping_a_tab_closes_the_overlay_it_leaves_behind() {
    // The other half, and the one that used to strand a process. `empty_and_close_tab`
    // sweeps what was handed over and says nothing to the pty thread, because those
    // children belong to the other session now. The overlay was never handed over, so
    // sweeping it out here would have taken it out of the tab before `close_tab_by_id`
    // could name it - leaving a child running with nothing attached to it. Left where
    // it is, it closes with the tab.
    let mut screen = screen_with_an_overlay_over_a_tab();
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    screen.empty_and_close_tab(0);

    let closed_by_pty: Vec<PaneId> = pty_receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PtyInstruction::CloseTab(pane_ids) => pane_ids,
            PtyInstruction::ClosePane(pane_id, _) => vec![pane_id],
            _ => vec![],
        })
        .collect();
    assert_eq!(
        closed_by_pty,
        vec![PaneId::Terminal(9)],
        "the overlay, and only the overlay: the tab's own pane was warped and must \
         not be closed here",
    );
}

#[test]
fn a_tab_whose_only_process_is_an_overlay_has_nothing_to_warp() {
    // The guard has to be asked of the panes that travel rather than of all of
    // them. Asked of all of them, a tab like this passes - and then warps with an
    // empty payload, which is an elaborate way of opening a new tab.
    let mut screen = screen_with_an_overlay_over_a_tab();
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .close_pane(PaneId::Terminal(1), false, None);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some("this tab has no panes with a process to move".to_owned())
    );
}

#[test]
fn a_tab_with_a_pane_replaced_in_place_still_cannot_be_warped() {
    // The pair this refusal is really about, and the one an `is_scrollback_editor`
    // check would let through: `suppress_pane_and_replace_with_pid` sets that flag
    // to `false` while parking the hidden pane behind the visible one - under its
    // own id, with the stand-in named in `restored_by` - so closing the tab swaps
    // it back in rather than removing it.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .suppress_pane_and_replace_with_pid(
            PaneId::Terminal(1),
            PaneId::Terminal(9),
            false,
            None,
            None,
            None,
        )
        .expect("TEST");

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

// ---------------------------------------------------------------------------
// Warp scrollback budget
// ---------------------------------------------------------------------------

#[test]
fn a_tabs_panes_share_one_frames_worth_of_scrollback() {
    // A whole tab travels in a single IPC frame. If every pane spent the
    // single-pane budget, a tab of sixteen would sit exactly on
    // MAX_IPC_FRAME_LEN and a tab of seventeen would be unsendable — which would
    // lose the tab at the very last step, after every supervisor had already
    // been released.
    for panes in [1, 2, 8, 16, 64, 256] {
        let total = Screen::warp_budget_per_pane(panes) * panes;
        assert!(
            total < zellij_utils::ipc::MAX_IPC_FRAME_LEN,
            "a tab of {panes} panes would not fit in a frame",
        );
    }
}

#[test]
fn a_small_tab_is_not_charged_for_panes_it_does_not_have() {
    // Sharing the allowance out must not make a one-pane tab carry *more*
    // scrollback than a single warped pane would, or the two paths would
    // truncate differently for no reason the user could see.
    assert_eq!(
        Screen::warp_budget_per_pane(1),
        Screen::WARP_CONTENTS_BUDGET
    );
    assert!(Screen::warp_budget_per_pane(64) < Screen::WARP_CONTENTS_BUDGET);
}

#[test]
fn a_scrollback_over_budget_keeps_its_most_recent_end() {
    // Truncating from the front is what makes this safe: the receiving side
    // feeds the result to a VTE parser, which picks up mid-stream.
    let contents = "abcdefghij".repeat(100);
    let fitted = Screen::fit_to_budget(contents.clone(), 100);
    assert_eq!(fitted.len(), 100);
    assert!(contents.ends_with(&fitted));
}

#[test]
fn a_scrollback_within_budget_is_left_alone() {
    let contents = "hello".to_owned();
    assert_eq!(Screen::fit_to_budget(contents.clone(), 100), contents);
}

// ---------------------------------------------------------------------------
// Warping a pane whose process is on another machine
// ---------------------------------------------------------------------------
//
// A remote pane is released as a broker and a token rather than as a supervisor
// socket, and `Screen` has to put that somewhere a `WarpedPane` can carry it.
// The two local fields stay empty on purpose: they name an address on *this*
// machine, and reading them for a remote pane is the mistake `remote` being
// `Some` exists to catch.

#[test]
fn a_remote_pane_warps_as_a_broker_and_a_token() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let warped = released_remote_pane_for_warp(&mut screen, 1);

    let remote = warped
        .remote
        .as_ref()
        .expect("a remote pane must travel with the address of the broker holding it");
    assert_eq!(remote.host, "build-box");
    assert_eq!(remote.pane_token, "token-1");
    assert_eq!(
        remote.received, 4096,
        "the byte count the broker replays from was dropped, which would overlap \
         the replay with the pane's serialized screen",
    );
    assert_eq!(
        remote.cmd.remote_host.as_deref(),
        Some("build-box"),
        "the command travels with a remote pane: the adopting server has no \
         supervisor to learn it from",
    );
}

#[test]
fn a_remote_pane_carries_no_local_address() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let warped = released_remote_pane_for_warp(&mut screen, 1);

    assert_eq!(
        warped.host_id, "",
        "a remote pane was given a local supervisor id, which names nothing here",
    );
    assert_eq!(
        warped.control_socket,
        PathBuf::new(),
        "a remote pane was given a local socket path, which nothing is listening on",
    );
}
