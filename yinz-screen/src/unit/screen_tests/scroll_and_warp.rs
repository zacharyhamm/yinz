#[cfg(test)]
const TERMINAL_50: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(50);

#[cfg(test)]
const TAB_0: TabId = TabId::new(0);

use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

/// Far enough out that no test is racing it. The deadline's own effect — a
/// release landing after the caller gave up — is asserted by the tests that
/// pass one already in the past.
fn warp_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(30)
}

#[test]
fn warp_departures_notify_plugins_and_subscribers_without_closing_processes() {
    for scenario in ["single", "editor", "stack", "tab"] {
        for failed in [false, true] {
            let mut screen = match scenario {
                "editor" => screen_with_a_scrollback_editor_open(),
                "stack" => screen_with_a_stack_list(),
                _ => screen_with_an_overlay_over_a_tab(),
            };
            let prepared = match scenario {
                "single" => None,
                "tab" => Some(
                    screen
                        .check_tab_can_be_warped(Some(TAB_0), ClientId::new(1))
                        .unwrap(),
                ),
                _ => screen
                    .prepare_pane_group(
                        Some(PaneId::Terminal(TerminalId::new(if scenario == "editor" {
                            9
                        } else {
                            3
                        }))),
                        ClientId::new(1),
                    )
                    .unwrap(),
            };
            let departing = prepared
                .as_ref()
                .map(|(_, _, prepared)| prepared.slots.clone())
                .unwrap_or_else(|| vec![PaneId::Terminal(TerminalId::new(1))]);
            let remaining: Vec<_> = screen
                .tabs
                .values()
                .flat_map(|tab| tab.get_all_pane_ids())
                .filter(|id| !departing.contains(id))
                .collect();
            assert!(!remaining.is_empty());
            let fake_os = FakeInputOutput::default();
            let messages = fake_os.server_to_client_messages.clone();
            screen.bus.os_input = Some(Box::new(fake_os));
            for (client, panes) in [
                (100, departing.clone()),
                (101, departing.iter().chain(&remaining).copied().collect()),
            ] {
                screen.subscribe_to_pane_renders(
                    ClientId::new(client),
                    panes.into_iter().collect(),
                    None,
                    false,
                );
            }
            messages.lock().unwrap().clear();
            let (to_plugin, plugins): ChannelWithContext<PluginInstruction> = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
            let (to_pty, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
            let release = |id| {
                if failed {
                    Err("release failed".to_owned())
                } else {
                    Ok(ReleasedPane::Local(ReleasedPtyHost {
                        host_id: SupervisorId::new(format!("host-{id}")),
                        host_pid: ProcessId::new(1),
                        control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
                    }))
                }
            };
            let result = if let Some((tab_id, terminals, prepared)) = prepared {
                screen
                    .take_released_tab_for_warp(
                        tab_id,
                        terminals.into_iter().map(|id| (id, release(id))).collect(),
                        prepared,
                    )
                    .map(|_| ())
            } else {
                screen
                    .take_released_pane_for_warp(TerminalId::new(1), release(TerminalId::new(1)))
                    .map(|_| ())
            };
            assert_eq!(result.is_err(), failed);
            let mut closed = vec![];
            for (instruction, _) in plugins.try_iter() {
                if let PluginInstruction::Update(events) = instruction {
                    for (plugin, client, event) in events {
                        if let Event::PaneClosed(id) = event {
                            assert_eq!((plugin, client), (None, None));
                            closed.push(id);
                        }
                    }
                }
            }
            let expected: Vec<yinz_types::data::PaneId> =
                if failed { vec![] } else { departing.to_vec() };
            assert_eq!(closed.len(), expected.len(), "{scenario}, failed={failed}");
            for id in &expected {
                assert_eq!(closed.iter().filter(|closed| *closed == id).count(), 1);
            }
            let messages = messages.lock().unwrap();
            for client in [100, 101] {
                let events = messages
                    .get(&ClientId::new(client))
                    .cloned()
                    .unwrap_or_default();
                let closed: Vec<_> = events
                    .iter()
                    .filter_map(|event| match event {
                        ServerToClientMsg::SubscribedPaneClosed { pane_id } => Some(*pane_id),
                        _ => None,
                    })
                    .collect();
                assert_eq!(closed.len(), expected.len());
                for id in &expected {
                    assert_eq!(closed.iter().filter(|closed| *closed == id).count(), 1);
                }
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, ServerToClientMsg::Exit { .. }))
                        .count(),
                    usize::from(!failed && client == 100)
                );
                assert_eq!(
                    screen
                        .pane_render_subscribers
                        .contains_key(&ClientId::new(client)),
                    failed || client == 101
                );
            }
            if !failed {
                assert_eq!(
                    screen.pane_render_subscribers[&ClientId::new(101)].pane_ids,
                    remaining.into_iter().collect()
                );
            }
            for (instruction, _) in pty.try_iter() {
                match instruction {
                    PtyInstruction::ClosePane(PaneId::Terminal(id), _) => {
                        panic!("closed transferred process {id}")
                    },
                    PtyInstruction::CloseTab(ids) => assert!(ids.is_empty()),
                    _ => {},
                }
            }
        }
    }
}

#[test]
fn destination_local_group_placement_does_not_notify_departure() {
    for placement in [
        yinz_types::data::WarpPlacement::FocusedTab,
        yinz_types::data::WarpPlacement::Floating,
    ] {
        let (mut screen, messages) =
            create_new_screen_with_message_capture(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TAB_0);
        let (tab_id, reply) = queue_warp_group(&mut screen, placement);
        let pending = screen.pending_group_placements.remove(&tab_id).unwrap();
        let (to_plugin, plugins): ChannelWithContext<PluginInstruction> = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        apply_warp_group(&mut screen, tab_id);
        screen.subscribe_to_pane_renders(
            ClientId::new(100),
            vec![yinz_types::data::PaneId::Terminal(TERMINAL_50)],
            None,
            false,
        );
        screen.pending_group_placements.insert(tab_id, pending);
        screen
            .finish_warp_group(
                tab_id,
                &[PaneId::Terminal(TERMINAL_50)],
                &Default::default(),
            )
            .unwrap();
        reply
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(screen.tabs[&TAB_0]
            .get_pane_with_id(PaneId::Terminal(TERMINAL_50))
            .is_some());
        assert!(screen.pane_render_subscribers[&ClientId::new(100)]
            .pane_ids
            .contains(&yinz_types::data::PaneId::Terminal(TERMINAL_50)));
        assert!(plugins
            .try_iter()
            .all(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events
                    .iter()
                    .all(|(_, _, event)| !matches!(event, Event::PaneClosed(_))),
                _ => true,
            }));
        assert!(messages
            .lock()
            .unwrap()
            .values()
            .flatten()
            .all(|event| !matches!(event, ServerToClientMsg::SubscribedPaneClosed { .. })));
    }
}
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
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "pane filled",
        |frame| frame.contains("something 19"),
    );
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
    let scrolled = wait_for_settled_render(
        &received_server_instructions,
        size,
        "left pane scrolled",
        |frame| frame.contains("SCROLL:  4/13") && !frame.contains("something 19"),
    );
    let checkpoint = received_server_instructions.lock().len();

    // Move focus away to the right pane and back to the left (scrolled) pane. A MoveFocus always
    // leaves Scroll mode first, which is exactly the path that used to reset the scroll position.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "focus on right pane",
        |cursor, _| cursor.is_some_and(|(x, _)| x > 40),
    );
    let checkpoint = received_server_instructions.lock().len();
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Left,
        },
        client_id,
    );
    let returned = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "focus back on scrolled left pane",
        |cursor, frame| {
            cursor.is_some_and(|(x, _)| x < 40)
                && frame.contains("SCROLL:")
                && !frame.contains("something 19")
        },
    );
    assert_eq!(
        returned, scrolled,
        "focus must preserve the scroll position"
    );
    mock_screen.teardown(vec![server_thread, screen_thread]);
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
fn visible_and_hidden_plugin_companions_keep_their_launch_configuration() {
    use yinz_types::data::WarpPlacement;
    for placement in [
        Some(WarpPlacement::FocusedTab),
        Some(WarpPlacement::NewTab),
        None,
    ] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 8 }, false, false);
        let (to_plugin, _plugin_receiver) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        let plugin = PaneId::Plugin(crate::PluginId::new(50));
        let run = Run::Plugin(RunPluginOrAlias::RunPlugin(
            yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider")
                .expect("plugin configuration"),
        ));
        let placed = screen.replace_pane(
            plugin,
            None,
            Some(run.clone()),
            None,
            yinz_bus::pty::ReplacedPane::Suppress,
            ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(TerminalId::new(1))),
            &mut None,
        );
        assert!(placed, "plugin cover");
        let (_, terminals, prepared) = screen
            .prepare_pane_group(Some(plugin), ClientId::new(1))
            .expect("visible plugin")
            .expect("group");
        assert_eq!(terminals, vec![TerminalId::new(1)]);
        assert_eq!(prepared.tiled_layout.run, Some(run.clone()));
        let cover = PaneId::Terminal(TerminalId::new(60));
        let placed = screen.replace_pane(
            cover,
            None,
            None,
            None,
            yinz_bus::pty::ReplacedPane::Suppress,
            ClientTabIndexOrPaneId::PaneId(plugin),
            &mut None,
        );
        assert!(placed, "terminal cover");
        assert!(screen
            .prepare_pane_group(Some(plugin), ClientId::new(1))
            .is_err());
        let (_, terminals, prepared) = screen
            .prepare_pane_group(Some(cover), ClientId::new(1))
            .expect("visible cover")
            .expect("group");
        assert_eq!(terminals, vec![TerminalId::new(60), TerminalId::new(1)]);
        let prepared = if placement.is_none() {
            screen
                .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
                .unwrap()
                .2
        } else {
            prepared
        };
        let relationships = prepared.relationships.as_ref().expect("relationships");
        let links: HashMap<_, _> = relationships
            .hidden
            .iter()
            .map(|hidden| {
                (
                    prepared.slots[hidden.pane as usize],
                    (
                        hidden.restored_by.map(|id| prepared.slots[id as usize]),
                        hidden.is_scrollback_editor,
                    ),
                )
            })
            .collect();
        assert_eq!(
            links,
            HashMap::from([
                (plugin, (Some(cover), false)),
                (PaneId::Terminal(TerminalId::new(1)), (Some(plugin), false)),
            ])
        );
        assert_eq!(
            prepared
                .floating_layout
                .iter()
                .filter(|pane| pane.run == Some(run.clone()))
                .count(),
            1
        );

        // Apply the transmitted layout with IDs allocated by the destination.
        let destination_id = TabId::new(2);
        screen
            .new_tab(
                destination_id,
                (vec![], vec![]),
                None,
                Some(ClientId::new(1)),
            )
            .unwrap();
        let (tx, rx) = crossbeam::channel::bounded(1);
        if let Some(placement) = placement {
            screen.pending_group_placements.insert(
                destination_id,
                (
                    placement,
                    Some(TabId::new(1)),
                    ClientId::new(1),
                    Some(tx),
                    None,
                ),
            );
        }
        screen
            .pending_warp_relationships
            .insert(destination_id, prepared.relationships.unwrap());
        let Run::Plugin(plugin_run) = run.clone() else {
            unreachable!()
        };
        screen
            .apply_layout(
                prepared.tiled_layout,
                prepared.floating_layout,
                vec![(TerminalId::new(160), None)],
                vec![(TerminalId::new(101), None)],
                HashMap::from([(plugin_run, vec![crate::PluginId::new(150)])]),
                destination_id,
                true,
                ClientId::new(1),
                None,
            )
            .expect("reconstruct plugin companions");
        if placement.is_some() {
            rx.try_recv()
                .expect("placement reply")
                .expect("placement succeeded");
        }
        let final_tab = if placement == Some(WarpPlacement::FocusedTab) {
            TabId::new(1)
        } else {
            destination_id
        };
        assert_eq!(
            screen.get_active_pane_id(&ClientId::new(1)),
            Some(PaneId::Terminal(TerminalId::new(160)))
        );
        let destination = screen.tabs.get_mut(&final_tab).unwrap();
        let plugin = PaneId::Plugin(crate::PluginId::new(150));
        let cover = PaneId::Terminal(TerminalId::new(160));
        let subject = PaneId::Terminal(TerminalId::new(101));
        assert!(destination.has_non_suppressed_pane_with_pid(&cover));
        assert!(!destination.has_non_suppressed_pane_with_pid(&plugin));
        assert!(!destination.has_non_suppressed_pane_with_pid(&subject));
        assert_eq!(
            destination.get_pane_with_id(plugin).unwrap().invoked_with(),
            &Some(run)
        );
        destination.close_pane(cover, false, None);
        assert!(destination.has_non_suppressed_pane_with_pid(&plugin));
        assert!(!destination.has_non_suppressed_pane_with_pid(&subject));
        destination.close_pane(plugin, false, None);
        assert!(destination.has_non_suppressed_pane_with_pid(&subject));
    }
}

#[test]
fn a_pane_from_another_session_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    assert_eq!(
        screen.check_pane_can_be_warped(
            Some(PaneId::Terminal(crate::TerminalId::new(99))),
            crate::ClientId::new(1)
        ),
        Err("there is no pane 99 in this session".to_owned())
    );
}

#[test]
fn an_adopted_pane_still_follows_a_client_looking_at_a_tab() {
    // The pane arrives in the tab the client is looking at when it is adopted,
    // not the one they started in - and that tab is named, so nothing between
    // here and the pane's creation can second-guess the placement.
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .go_to_tab(TabNumber::new(1), client_id)
        .expect("TEST");

    assert_eq!(
        adopted_pane_placement_target(
            &mut screen,
            yinz_types::data::WarpPlacement::FocusedTab,
            client_id
        ),
        ClientTabIndexOrPaneId::TabIndex(TabId::new(0)),
    );
}

#[test]
fn an_adopted_pane_from_a_client_with_no_screen_goes_to_the_attached_clients_tab() {
    // A warp request is a client of its own; the tab is the attached client's.
    let attached = 1;
    let requester = 7;
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .go_to_tab(TabNumber::new(2), crate::ClientId::new(attached))
        .expect("TEST");

    assert_eq!(
        adopted_pane_placement_target(
            &mut screen,
            yinz_types::data::WarpPlacement::FocusedTab,
            crate::ClientId::new(requester)
        ),
        ClientTabIndexOrPaneId::TabIndex(TabId::new(1)),
    );
}

#[test]
fn an_adopted_focused_tab_pane_gets_focus_and_leaves_scroll_mode() {
    for default_mode in [InputMode::Normal, InputMode::Locked] {
        for requester in [1, 7] {
            let size = Size { cols: 80, rows: 20 };
            let mut mock_screen = MockScreen::new(size);
            let client_id = mock_screen.main_client_id;
            mock_screen.config.options.default_mode = Some(default_mode);
            // Two panes would each get 800 cells, below this 1600-cell floor.
            mock_screen.config.options.agent_pane_min_columns = Some(80);
            mock_screen.config.options.agent_pane_min_rows = Some(20);
            let screen_thread = mock_screen.run(None, vec![]);
            let instructions = Watched::new(vec![]);
            let server_receiver = mock_screen.server_receiver.take().unwrap();
            let server_thread = log_actions_in_thread!(
                instructions,
                ServerInstruction::KillSession,
                server_receiver
            );
            mock_screen
                .to_screen
                .send(ScreenInstruction::ChangeMode(
                    InputMode::Scroll,
                    None,
                    client_id,
                    None,
                ))
                .expect("TEST");
            mock_screen
                .to_screen
                .send(ScreenInstruction::AdoptedPaneReady {
                    hold: None,
                    placed: crossbeam::channel::unbounded().0,
                    terminal_id: crate::TerminalId::new(50),
                    payload: Box::default(),
                    placement: yinz_types::data::WarpPlacement::FocusedTab,
                    client_id: crate::ClientId::new(requester),
                    cmd: RunCommand {
                        command: PathBuf::from("claude"),
                        ..Default::default()
                    },
                    buffered: vec![],
                    child_exit_status: None,
                })
                .expect("TEST");
            mock_screen.received_background_jobs.wait_until(
                "the adopted agent to be focused in the selected tab",
                yinz_test_support::default_timeout(),
                |jobs| {
                    jobs.iter().find_map(|job| match job {
                        BackgroundJob::ReportSessionInfo(_, info)
                            if info.tabs.len() == 1
                                && info.panes.panes.get(&TabPosition::new(0)).is_some_and(
                                    |panes| {
                                        panes.len() == 2
                                            && panes.iter().any(|pane| {
                                                pane.id
                                                    == PaneId::Terminal(crate::TerminalId::new(50))
                                                    && pane.is_focused
                                            })
                                    },
                                ) =>
                        {
                            Some(())
                        },
                        _ => None,
                    })
                },
            );
            instructions.wait_until(
                "focus to request the configured default mode for the attached client",
                yinz_test_support::default_timeout(),
                |instructions| {
                    instructions
                        .iter()
                        .any(|instruction| {
                            matches!(instruction, ServerInstruction::ChangeMode(cid, mode, _)
                            if *cid == client_id && *mode == default_mode)
                        })
                        .then_some(())
                },
            );
            mock_screen.teardown(vec![server_thread, screen_thread]);
        }
    }
}

#[test]
fn an_adopted_focused_tab_pane_without_attached_clients_is_placed_without_focus() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    assert_eq!(
        screen.connected_client_to_act_for(crate::ClientId::new(7)),
        None
    );
    let (to_screen, receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen
        .place_adopted_pane(
            crate::TerminalId::new(50),
            WarpedPane {
                agent_theme: None,
                contents: Some("transferred screen".into()),
                ..Default::default()
            },
            yinz_types::data::WarpPlacement::FocusedTab,
            crate::ClientId::new(7),
            RunCommand::default(),
            b"buffered output".to_vec(),
            None,
            &mut None,
        )
        .expect("TEST");
    let instructions: Vec<_> = receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    assert!(matches!(
        instructions.as_slice(),
        [ScreenInstruction::NewPane(
            PaneId::Terminal(TERMINAL_50),
            _,
            _,
            _,
            _,
            _,
            ClientTabIndexOrPaneId::TabIndex(TAB_0),
            _,
            _
        ),
        ScreenInstruction::PtyBytes(TERMINAL_50, contents),
        ScreenInstruction::RequestPaneRedraw(TERMINAL_50)]
        if contents == b"transferred screen\x1b[?2004lbuffered output"
    ));
}

/// An adopted pane whose child already exited takes the branch its old session
/// would have: held only if `hold_on_close`, otherwise closed. The supervisor
/// replays the same exit through the pty's `quit_cb`, which takes that branch
/// too — so the two never disagree, and no `HoldPane` is left waiting for a pane
/// that was closed.
#[test]
fn an_adopted_pane_that_already_exited_is_held_or_closed_as_its_command_says() {
    for hold_on_close in [true, false] {
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        let (to_screen, receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
        screen
            .place_adopted_pane(
                crate::TerminalId::new(50),
                WarpedPane::default(),
                yinz_types::data::WarpPlacement::FocusedTab,
                crate::ClientId::new(1),
                RunCommand {
                    command: PathBuf::from("worker"),
                    hold_on_close,
                    ..Default::default()
                },
                vec![],
                Some(Some(3)),
                &mut None,
            )
            .expect("TEST");
        let instructions: Vec<_> = receiver
            .try_iter()
            .map(|(instruction, _)| instruction)
            .collect();
        assert!(!instructions
            .iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::RequestPaneRedraw(_))));
        let held_on_arrival = instructions
            .iter()
            .find_map(|instruction| match instruction {
                ScreenInstruction::NewPane(PaneId::Terminal(TERMINAL_50), _, hold, ..) => {
                    Some(hold.is_some())
                },
                _ => None,
            });
        assert_eq!(
            held_on_arrival,
            Some(hold_on_close),
            "hold_on_close={hold_on_close}: the pane should arrive held exactly when it would have been held"
        );
        let holds = instructions
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    ScreenInstruction::HoldPane(PaneId::Terminal(TERMINAL_50), Some(3), _)
                )
            })
            .count();
        let closes = instructions
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction,
                    ScreenInstruction::ClosePane(
                        PaneId::Terminal(TERMINAL_50),
                        None,
                        None,
                        Some(3)
                    )
                )
            })
            .count();
        assert_eq!(
            (holds, closes),
            if hold_on_close { (1, 0) } else { (0, 1) },
            "hold_on_close={hold_on_close}: exactly one of hold or close, carrying the exit status"
        );
    }
}

#[test]
fn an_adopted_agent_pane_lands_in_the_focused_tab_however_crowded_it_is() {
    // The user asked for the focused tab. `agent_pane_target` moves a
    // client-relative agent pane out of a tab whose share would fall below the
    // floor; a warp's placement is not its to change.
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    screen.agent_pane_min_size = Size { cols: 40, rows: 10 };
    new_tab(&mut screen, 1, TabId::new(0));
    for pid in 2..5 {
        add_pane_to_tab(
            &mut screen,
            TabId::new(0),
            PaneId::Terminal(crate::TerminalId::new(pid)),
            false,
        );
    }
    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));

    let (invoked_with, mut placement, target) = adopted_pane_new_pane(
        &mut screen,
        yinz_types::data::WarpPlacement::FocusedTab,
        client_id,
        RunCommand {
            command: PathBuf::from("claude"),
            ..Default::default()
        },
    );
    assert_eq!(target, ClientTabIndexOrPaneId::TabIndex(TabId::new(0)));
    let placed = screen.agent_pane_target(invoked_with.as_ref(), &mut placement, false, target);
    assert_eq!(
        placed.target,
        ClientTabIndexOrPaneId::TabIndex(TabId::new(0))
    );
    assert_eq!(placed.focus_after_placing, None);
    assert!(
        !plugin_receiver
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::NewTab(..))),
        "no tab was opened for the pane"
    );
}

#[test]
fn warping_a_visible_editor_carries_its_subject() {
    let mut screen = screen_with_a_scrollback_editor_open();
    let (to_pty, receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (tab_id, terminals, prepared) = screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(9))), ClientId::new(1))
        .expect("selection")
        .expect("companions");
    let slots = prepared.slots.clone();
    let released = terminals
        .into_iter()
        .map(|id| {
            (
                id,
                Ok(ReleasedPane::Local(ReleasedPtyHost {
                    host_id: SupervisorId::new(format!("host-{id}")),
                    host_pid: ProcessId::new(1),
                    control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
                })),
            )
        })
        .collect();
    let payload = screen
        .take_released_tab_for_warp(tab_id, released, prepared)
        .expect("transfer");
    assert!(payload.relationships.is_some());
    assert!(payload.tiled_layout.tab_group.is_none());
    assert!(payload.source_instance_id.is_none());
    assert!(slots
        .iter()
        .all(|id| screen.tabs[&tab_id].get_pane_with_id(*id).is_none()));
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyInstruction::ClosePane(_, _))));
}

#[test]
fn a_release_that_failed_leaves_the_pane_where_it_was() {
    // Screen is told about a release either way, and a release that did not
    // happen must not take the pane out of its tab: on the refusal path the pane
    // is untouched and still working, and on the failure path the pty thread has
    // already held it and written the reason into it. Either way, taking it out
    // here would be Screen deciding on its own that the pane is gone.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let result = screen.take_released_pane_for_warp(
        crate::TerminalId::new(1),
        Err("pane 1 has no pty-host supervisor".to_owned()),
    );

    assert_eq!(
        result.expect_err("a pane that was not released cannot be serialized"),
        "terminal_1 (\"Pane #1\", live): pane 1 has no pty-host supervisor",
        "the reason includes the typed pane identity, title and state",
    );
    assert!(
        screen.tabs.values().any(|tab| tab
            .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
            .is_some()),
        "the pane was taken out of its tab by a warp that never happened",
    );
}

#[test]
fn the_hidden_half_of_a_scrollback_editor_pair_cannot_be_warped_refused() {
    let screen = screen_with_a_scrollback_editor_open();
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(1))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
}

/// A pane spawned to replace one that closed meanwhile (after a slow SSH connection, say)
/// has nowhere to go. Screen never owned it, so it tells the pty thread to
/// close it rather than leaving its process, transport and reader running
/// until the session ends.
#[test]
fn a_replacement_for_a_pane_that_is_gone_is_closed_on_the_pty_thread() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    let placed = screen.replace_pane(
        PaneId::Terminal(crate::TerminalId::new(50)),
        None,
        None,
        None,
        yinz_bus::pty::ReplacedPane::Suppress,
        ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(999))),
        &mut None,
    );
    assert!(!placed, "the target is gone");

    let closed_by_pty: Vec<PaneId> = pty_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyInstruction::ClosePane(pane_id, _) => Some(pane_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        closed_by_pty,
        vec![PaneId::Terminal(crate::TerminalId::new(50))]
    );
}

#[test]
fn warping_an_editor_pair_never_closes_either_process() {
    let mut screen = screen_with_a_scrollback_editor_open();
    let (to_pty, receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (tab_id, terminals, prepared) = screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(9))), ClientId::new(1))
        .expect("selection")
        .expect("companions");
    let slots = prepared.slots.clone();
    let released = terminals
        .into_iter()
        .map(|id| {
            (
                id,
                Ok(ReleasedPane::Local(ReleasedPtyHost {
                    host_id: SupervisorId::new(format!("host-{id}")),
                    host_pid: ProcessId::new(1),
                    control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
                })),
            )
        })
        .collect();
    let payload = screen
        .take_released_tab_for_warp(tab_id, released, prepared)
        .expect("transfer");
    assert!(payload.relationships.is_some());
    assert!(slots
        .iter()
        .all(|id| screen.tabs[&tab_id].get_pane_with_id(*id).is_none()));
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyInstruction::ClosePane(_, _))));
}

#[test]
fn warping_a_hidden_replacement_subject_is_refused() {
    let screen = screen_with_a_pane_replaced_in_place();
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(1))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
}

#[test]
fn a_pane_a_plugin_hid_cannot_be_warped() {
    // Parked under its own id with nothing standing in for it: it is on screen
    // nowhere, so nothing on screen changes when it leaves.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .expect("TEST")
        .suppress_pane(PaneId::Terminal(crate::TerminalId::new(2)), None);
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(2))), ClientId::new(1))
        .is_err());
    assert!(screen.tabs[&TabId::new(1)]
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
        .is_some());
}

#[test]
fn the_sessions_last_pane_and_its_editor_can_be_warped() {
    let mut screen = screen_with_a_scrollback_editor_open();
    screen.tabs.retain(|id, _| *id == TabId::new(0));
    let (_, terminals, _) = screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(9))), ClientId::new(1))
        .expect("the last companion group can leave the session")
        .expect("companions");
    assert_eq!(terminals.len(), 2);
}

#[test]
fn the_sessions_only_pane_can_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_pty, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (to_server, server): ChannelWithContext<ServerInstruction> = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(to_server));
    let (response_tx, response_rx) = yinz_bus::warp::pane_warp_reply();

    screen
        .warp_pane(
            true,
            true,
            Some(PaneId::Terminal(TerminalId::new(1))),
            yinz_types::internal_ids::SessionName::new("destination"),
            yinz_types::data::WarpPlacement::NewTab,
            ClientId::new(1),
            response_tx,
            warp_deadline(),
        )
        .expect("start warp");
    assert!(pty.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PtyInstruction::ReleaseTerminalForWarp { terminal_id, .. }
            if terminal_id == TerminalId::new(1)
    )));
    screen.pane_released_for_warp(
        TerminalId::new(1),
        Ok(ReleasedPane::Local(ReleasedPtyHost {
            host_id: SupervisorId::new("host-1"),
            host_pid: ProcessId::new(1),
            control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
        })),
    );
    let (payload, hold) = response_rx
        .try_recv()
        .expect("warp response")
        .expect("warped pane");
    assert_eq!(payload.host_id, SupervisorId::new("host-1"));
    assert_eq!(screen.in_flight_pane_warps, 1);
    screen
        .settle_tabs()
        .expect("retain the rollback destination");
    assert_eq!(screen.tabs.len(), 1);
    assert_eq!(screen.active_tab_ids[&ClientId::new(1)], TabId::new(0));
    assert!(!server
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))));
    // Stands in for the `PaneWarpFinished` the dropped hold sends.
    drop(hold);
    screen.in_flight_pane_warps = 0;
    screen
        .settle_tabs()
        .expect("close the emptied source session");
    assert!(screen.tabs.is_empty());
    assert!(server
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))));
    for (instruction, _) in pty.try_iter() {
        match instruction {
            PtyInstruction::ClosePane(_, _) => panic!("the warped process must stay alive"),
            PtyInstruction::CloseTab(panes) => assert!(panes.is_empty()),
            _ => {},
        }
    }
}

#[test]
fn a_pane_released_after_its_caller_left_is_taken_back_with_its_hold() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_pty, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (response_tx, response_rx) = yinz_bus::warp::pane_warp_reply();
    screen
        .warp_pane(
            true,
            true,
            Some(PaneId::Terminal(TerminalId::new(1))),
            yinz_types::internal_ids::SessionName::new("destination"),
            yinz_types::data::WarpPlacement::NewTab,
            ClientId::new(1),
            response_tx,
            warp_deadline(),
        )
        .expect("start warp");
    drop(response_rx);
    screen.pane_released_for_warp(
        TerminalId::new(1),
        Ok(ReleasedPane::Local(ReleasedPtyHost {
            host_id: SupervisorId::new("host-1"),
            host_pid: ProcessId::new(1),
            control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
        })),
    );
    let taken_back = pty
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PtyInstruction::AdoptTerminal {
                payload,
                placement,
                hold,
                ..
            } => Some((payload, placement, hold)),
            _ => None,
        });
    let Some((payload, placement, hold)) = taken_back else {
        panic!("the unclaimed pane must be adopted back")
    };
    assert_eq!(payload.host_id, SupervisorId::new("host-1"));
    assert_eq!(placement, yinz_types::data::WarpPlacement::NewTab);
    assert!(
        hold.is_some(),
        "the session must stay open until it is placed"
    );
    assert_eq!(screen.in_flight_pane_warps, 1);
    screen.settle_tabs().expect("keep the emptied tab");
    assert_eq!(screen.tabs.len(), 1);
}

fn adopted_pane_finish(
    terminal_id: u32,
    recovery: &str,
) -> (
    yinz_bus::warp::AdoptedPaneFinish,
    crossbeam::channel::Receiver<Result<()>>,
) {
    let (placed, placed_rx) = crossbeam::channel::unbounded();
    (
        yinz_bus::warp::AdoptedPaneFinish {
            terminal_id: TerminalId::new(terminal_id),
            recovery: recovery.to_owned(),
            held: false,
            hold: None,
            placed,
        },
        placed_rx,
    )
}

#[test]
fn an_adopted_terminal_without_a_pane_is_given_back() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_pty, pty): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (finish, placed) = adopted_pane_finish(1, "yinz pty-host adopt placed");
    screen.finish_adopted_pane(finish);
    assert!(pty.try_recv().is_err(), "a placed pane is kept");
    assert!(matches!(placed.try_recv(), Ok(Ok(()))));
    let (finish, placed) = adopted_pane_finish(77, "yinz pty-host adopt lost");
    screen.finish_adopted_pane(finish);
    assert!(matches!(
        pty.try_recv().unwrap().0,
        PtyInstruction::GiveBackAdoptedTerminal {
            terminal_id,
            held: false,
            recovery,
            placed: Some(_),
        } if terminal_id == TerminalId::new(77) && recovery == "yinz pty-host adopt lost"
    ));
    assert!(
        placed.try_recv().is_err(),
        "the pty thread answers once it has given the terminal back"
    );
}

/// The placement check follows `NewPane` directly, so a replayed exit that
/// closes the pane afterwards cannot make it look unplaced and get it given back.
#[test]
fn an_adopted_pane_is_checked_before_its_replayed_exit_can_close_it() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_screen, receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    let (finish, _placed) = adopted_pane_finish(50, "yinz pty-host adopt exited");
    let mut finish = Some(finish);
    screen
        .place_adopted_pane(
            TERMINAL_50,
            WarpedPane::default(),
            yinz_types::data::WarpPlacement::FocusedTab,
            crate::ClientId::new(1),
            RunCommand {
                command: PathBuf::from("worker"),
                ..Default::default()
            },
            vec![],
            Some(Some(3)),
            &mut finish,
        )
        .expect("TEST");
    assert!(finish.is_none(), "the check is queued");
    let instructions: Vec<_> = receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    let new_pane = instructions
        .iter()
        .position(|i| {
            matches!(
                i,
                ScreenInstruction::NewPane(PaneId::Terminal(TERMINAL_50), ..)
            )
        })
        .expect("NewPane");
    assert!(
        matches!(
            instructions.get(new_pane + 1),
            Some(ScreenInstruction::AdoptedPaneFinished(finish))
                if finish.terminal_id == TERMINAL_50
        ),
        "{instructions:?}"
    );
}

#[test]
fn closing_a_pending_group_tab_releases_its_warp_hold() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let (to_screen, receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    let (response, response_rx) = crossbeam::channel::unbounded();
    screen.pending_group_placements.insert(
        TabId::new(1),
        (
            yinz_types::data::WarpPlacement::FocusedTab,
            Some(TabId::new(0)),
            ClientId::new(1),
            Some(response),
            Some(yinz_bus::warp::PaneWarpHold::new(
                screen.bus.senders.clone(),
            )),
        ),
    );
    screen.close_tab_by_id(TabId::new(1)).expect("close tab");
    assert!(screen.pending_group_placements.is_empty());
    assert!(response_rx.try_recv().unwrap().is_err());
    assert!(receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ScreenInstruction::PaneWarpFinished)));
}

#[test]
fn a_terminal_pane_with_somewhere_to_leave_behind_can_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    assert_eq!(
        screen.check_pane_can_be_warped(
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            crate::ClientId::new(1)
        ),
        Ok(TerminalId::new(1))
    );
}

#[test]
fn a_tabs_last_pane_can_still_be_warped() {
    // Deliberately allowed: the render pass sweeps a tab with nothing selectable
    // left in it, so emptying a tab is a tab closing, not a session ending.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    assert_eq!(
        screen
            .tabs
            .get(&TabId::new(1))
            .expect("TEST")
            .get_all_pane_ids()
            .len(),
        1
    );
    assert_eq!(
        screen.check_pane_can_be_warped(
            Some(PaneId::Terminal(crate::TerminalId::new(2))),
            crate::ClientId::new(1)
        ),
        Ok(TerminalId::new(2))
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
fn released_remote_pane(
    screen: &mut Screen,
    terminal_id: yinz_types::internal_ids::TerminalId,
) -> Box<WarpedPane> {
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
                    target: yinz_types::command::PaneTarget::Ssh {
                        host: "build-box".to_owned(),
                    },
                    ..Default::default()
                },
            })),
        )
        .expect("a remote pane should have been taken out of its tab")
}

#[test]
fn a_remote_pane_travels_as_a_broker_address_rather_than_a_socket() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let payload = released_remote_pane(&mut screen, yinz_types::internal_ids::TerminalId::new(1));

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
        remote.cmd.target.ssh_host(),
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
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let somewhere = |screen: &Screen| {
        screen.tabs.values().any(|tab| {
            tab.get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
                .is_some()
        })
    };
    assert!(somewhere(&screen));

    released_remote_pane(&mut screen, yinz_types::internal_ids::TerminalId::new(1));

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
fn a_tab_is_not_taken_apart_for_a_caller_that_has_already_given_up() {
    // Serializing a tab is also what closes it, so a caller found to be gone
    // *after* that has already cost this session the whole tab, with every
    // process in it running unwatched and a log line as the only record. The
    // question has to be asked first, and a tab whose caller has stopped
    // waiting has to stay where it is — its panes held and told why, which is
    // the one form of this the user can see and act on.
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(None, vec![]);
    let (response_tx, response_rx) = crossbeam::channel::unbounded();

    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpTab {
            supports_companions: true,
            supports_held: true,
            tab_id: Some(TabId::new(0)),
            client_id,
            response_channel: response_tx,
            // Already past. No sleep needed and none wanted: what is being
            // tested is the comparison, not the waiting.
            deadline: std::time::Instant::now() - std::time::Duration::from_secs(1),
        })
        .unwrap();

    let released = loop {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the warp should reach the pty thread");
        if let PtyInstruction::ReleaseTerminalsForWarp { terminal_ids, .. } = instruction {
            break terminal_ids;
        }
    };
    assert!(
        !released.is_empty(),
        "the tab under test must have had a pane to release"
    );

    // Every supervisor really was handed over — this is the window the fix is
    // about, not a release that failed.
    mock_screen
        .to_screen
        .send(ScreenInstruction::TabReleasedForWarp {
            tab_index: TabId::new(0),
            released: released
                .iter()
                .map(|terminal_id| {
                    (
                        *terminal_id,
                        Ok(ReleasedPane::Local(ReleasedPtyHost {
                            host_id: SupervisorId::new(format!("host-{terminal_id}")),
                            host_pid: ProcessId::new(1),
                            control_socket: Some(std::path::PathBuf::from("/tmp/warp-test.sock")),
                        })),
                    )
                })
                .collect(),
        })
        .unwrap();

    let mut stranded = Vec::new();
    while stranded.len() < released.len() {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("every released pane must be held and told what happened to it");
        if let PtyInstruction::StrandPaneAfterWarp { terminal_id, .. } = instruction {
            stranded.push(terminal_id);
        }
    }
    stranded.sort_unstable();
    let mut expected = released.clone();
    expected.sort_unstable();
    assert_eq!(
        stranded, expected,
        "a pane whose supervisor is gone must say so in the pane itself, not only in a log"
    );

    assert!(
        response_rx.try_recv().is_err(),
        "nothing should have been sent to a caller that stopped waiting"
    );

    mock_screen.teardown(vec![screen_thread]);
}

#[test]
fn a_tab_is_not_taken_apart_when_its_release_lands_inside_screens_own_reserve() {
    // The pty thread is given the caller's deadline less
    // `TAB_WARP_SCREEN_RESERVE`, because serializing the tab takes time of its
    // own. The check on the way back has to use the same number: a release
    // landing with less than the reserve left is one this thread can no longer
    // serialize in time, and comparing against the caller's *full* deadline
    // let it through — to close the tab and then find nobody listening.
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(None, vec![]);
    let (response_tx, response_rx) = crossbeam::channel::unbounded();

    // Still in the future, but by less than the reserve. Nothing sleeps: the
    // release below lands at once, well inside that window.
    let inside_the_reserve =
        std::time::Instant::now() + crate::screen::warp_ops::TAB_WARP_SCREEN_RESERVE / 4;
    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpTab {
            supports_companions: true,
            supports_held: true,
            tab_id: Some(TabId::new(0)),
            client_id,
            response_channel: response_tx,
            deadline: inside_the_reserve,
        })
        .unwrap();

    let released = loop {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the warp should reach the pty thread");
        if let PtyInstruction::ReleaseTerminalsForWarp { terminal_ids, .. } = instruction {
            break terminal_ids;
        }
    };
    mock_screen
        .to_screen
        .send(ScreenInstruction::TabReleasedForWarp {
            tab_index: TabId::new(0),
            released: released
                .iter()
                .map(|terminal_id| {
                    (
                        *terminal_id,
                        Ok(ReleasedPane::Local(ReleasedPtyHost {
                            host_id: SupervisorId::new(format!("host-{terminal_id}")),
                            host_pid: ProcessId::new(1),
                            control_socket: Some(std::path::PathBuf::from("/tmp/warp-test.sock")),
                        })),
                    )
                })
                .collect(),
        })
        .unwrap();

    let mut stranded = Vec::new();
    while stranded.len() < released.len() {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("every released pane must be held rather than the tab closed");
        if let PtyInstruction::StrandPaneAfterWarp { terminal_id, .. } = instruction {
            stranded.push(terminal_id);
        }
    }
    stranded.sort_unstable();
    let mut expected = released.clone();
    expected.sort_unstable();
    assert_eq!(stranded, expected);
    assert!(
        response_rx.try_recv().is_err(),
        "a tab this thread could not serialize in time must not be sent"
    );

    mock_screen.teardown(vec![screen_thread]);
}

#[test]
fn a_pane_is_not_taken_apart_for_a_caller_that_has_already_given_up() {
    // The single-pane twin of the tab test above, which the pane path did not
    // get when the tab path did: it closed the pane first and discovered the
    // caller was gone from the failed send, by which point the pane was out of
    // its tab with its process running unwatched and a log line as the only
    // record.
    let size = Size { cols: 80, rows: 24 };
    let two_panes = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let client_id = mock_screen.main_client_id;
    let screen_thread = mock_screen.run(Some(two_panes), vec![]);
    let (response_tx, response_rx) = yinz_bus::warp::pane_warp_reply();

    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpPane {
            supports_companions: true,
            supports_held: true,
            pane_id: Some(PaneId::Terminal(crate::TerminalId::new(1))),
            to_session: "elsewhere".to_owned().into(),
            placement: yinz_types::data::WarpPlacement::NewTab,
            client_id,
            response_channel: response_tx,
            deadline: std::time::Instant::now() - std::time::Duration::from_secs(1),
        })
        .unwrap();

    let (terminal_id, to_session, placement) = loop {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the warp should reach the pty thread");
        if let PtyInstruction::ReleaseTerminalForWarp {
            terminal_id,
            to_session,
            placement,
            ..
        } = instruction
        {
            break (terminal_id, to_session, placement);
        }
    };
    assert_eq!(terminal_id, crate::TerminalId::new(1));

    // The supervisor really was handed over: this is the window under test,
    // not a release that failed.
    mock_screen
        .to_screen
        .send(ScreenInstruction::PaneReleasedForWarp {
            terminal_id,
            to_session,
            placement,
            client_id,
            released: Ok(ReleasedPane::Local(ReleasedPtyHost {
                host_id: SupervisorId::new("host-1".to_owned()),
                host_pid: ProcessId::new(1),
                control_socket: Some(std::path::PathBuf::from("/tmp/warp-test.sock")),
            })),
        })
        .unwrap();

    let stranded = loop {
        let (instruction, _) = pty_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the released pane must be held and told what happened to it");
        if let PtyInstruction::StrandPaneAfterWarp {
            terminal_id,
            recovery,
            ..
        } = instruction
        {
            break (terminal_id, recovery);
        }
    };
    assert_eq!(stranded.0, crate::TerminalId::new(1));
    assert!(
        stranded.1.is_some(),
        "a pane whose supervisor is gone must be told how to get it back"
    );
    assert!(
        response_rx.try_recv().is_err(),
        "nothing should have been sent to a caller that stopped waiting"
    );

    mock_screen.teardown(vec![screen_thread]);
}

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
                supports_companions: true,
                supports_held: true,
                tab_id: Some(TabId::new(0)),
                client_id,
                response_channel,
                deadline: warp_deadline(),
            })
            .unwrap();
    }

    let mut releases = 0;
    while releases < 1 {
        if matches!(
            pty_receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("first tab warp should reach the pty thread")
                .0,
            PtyInstruction::ReleaseTerminalsForWarp { .. }
        ) {
            releases += 1;
        }
    }

    let refused = vec![(
        TerminalId::new(0),
        Err("the tab's pane cannot be moved".to_owned()),
    )];
    mock_screen
        .to_screen
        .send(ScreenInstruction::TabReleasedForWarp {
            tab_index: TabId::new(0),
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
            tab_index: TabId::new(0),
            released: refused,
        })
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpTab {
            supports_companions: true,
            supports_held: true,
            tab_id: Some(TabId::new(0)),
            client_id,
            response_channel: crossbeam::channel::unbounded().0,
            deadline: warp_deadline(),
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
    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(
        screen
            .check_tab_can_be_warped(Some(TabId::new(99)), crate::ClientId::new(1))
            .err(),
        Some("there is no tab 99 in this session".to_owned())
    );
}

/// Compare identities after resolving payload-local references, independent of slot order.
fn assert_hidden_warp(
    terminals: &[TerminalId],
    prepared: &crate::screen::PreparedTabWarp,
    expected_terminals: &[u32],
    expected_hidden: &[(u32, Option<u32>, bool)],
) {
    let mut actual: Vec<_> = terminals.iter().map(|id| id.get()).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected_terminals);
    let mut hidden: Vec<_> = prepared
        .relationships
        .as_ref()
        .expect("relationships")
        .hidden
        .iter()
        .map(|hidden| {
            (
                prepared.slots[hidden.pane as usize],
                hidden.restored_by.map(|id| prepared.slots[id as usize]),
                hidden.is_scrollback_editor,
            )
        })
        .collect();
    hidden.sort_unstable();
    let mut expected: Vec<_> = expected_hidden
        .iter()
        .map(|(pane, cover, editor)| {
            (
                PaneId::Terminal(TerminalId::new(*pane)),
                cover.map(|id| PaneId::Terminal(TerminalId::new(id))),
                *editor,
            )
        })
        .collect();
    expected.sort_unstable();
    assert_eq!(hidden, expected);
}

#[test]
fn a_tab_with_a_scrollback_editor_open_can_be_warped() {
    // The editor and its subject travel with their restoration link.
    let mut screen = screen_with_a_scrollback_editor_open();

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    assert_hidden_warp(&terminals, &prepared, &[1, 9], &[(1, Some(9), true)]);
}

#[test]
fn a_tab_holding_a_pane_a_plugin_hid_can_be_warped() {
    // A parked terminal travels with the tab and stays parked.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
    tab.suppress_pane(PaneId::Terminal(crate::TerminalId::new(3)), None);

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    assert_hidden_warp(&terminals, &prepared, &[1, 3], &[(3, None, false)]);
    let destination_id = TabId::new(2);
    screen
        .new_tab(
            destination_id,
            (vec![], vec![]),
            None,
            Some(ClientId::new(1)),
        )
        .unwrap();
    screen
        .pending_warp_relationships
        .insert(destination_id, prepared.relationships.unwrap());
    screen
        .apply_layout(
            prepared.tiled_layout,
            prepared.floating_layout,
            vec![(TerminalId::new(101), None)],
            vec![(TerminalId::new(103), None)],
            HashMap::new(),
            destination_id,
            true,
            ClientId::new(1),
            None,
        )
        .expect("apply parked companion with new IDs");
    let destination = &screen.tabs[&destination_id];
    assert!(destination.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(101))));
    assert!(!destination.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(103))));
    let parked: Vec<_> = destination
        .get_suppressed_panes()
        .map(|(id, pane)| (*id, pane.restored_by))
        .collect();
    assert_eq!(parked, vec![(PaneId::Terminal(TerminalId::new(103)), None)]);
}

#[test]
fn a_tab_holding_a_hidden_floating_pane_can_be_warped() {
    // A suppressed floating terminal travels as a parked companion.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
    tab.suppress_pane(PaneId::Terminal(crate::TerminalId::new(3)), None);
    assert!(
        !tab.pane_is_stack_list_member(&PaneId::Terminal(crate::TerminalId::new(3))),
        "a floating pane is not a stack-list member, whatever else it is",
    );

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    assert_hidden_warp(&terminals, &prepared, &[1, 3], &[(3, None, false)]);
}

#[test]
fn the_sessions_only_tab_can_be_warped() {
    // The pane version refuses the session's last pane, because moving it would
    // end the session by accident. Asking for the last *tab* is asking for
    // everything on purpose, so it is allowed and the session then ends.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(screen.tabs.len(), 1);
    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
        .expect("the only tab should still be movable");
    assert_eq!(
        (tab_index, terminal_ids),
        (TabId::new(0), vec![TerminalId::new(1)])
    );
}

#[test]
fn warping_a_tab_takes_every_pane_that_has_a_process() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(TabId::new(1)), crate::ClientId::new(1))
        .expect("the tab should be movable");
    assert_eq!(tab_index, TabId::new(1));
    assert_eq!(terminal_ids, vec![TerminalId::new(2)]);
}

#[test]
fn a_tab_warp_defaults_to_the_focused_tab() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .go_to_tab(TabNumber::new(1), crate::ClientId::new(1))
        .expect("TEST");

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(None, crate::ClientId::new(1))
        .expect("the focused tab should be movable");
    assert_eq!(
        (tab_index, terminal_ids),
        (TabId::new(0), vec![TerminalId::new(1)])
    );
}

#[test]
fn a_tab_that_can_be_warped_has_already_had_its_arrangement_worked_out() {
    // The ordering that matters most here. Rebuilding a tab's layout from its
    // geometry can fail, and it happens during this check — while the tab is
    // still a tab — rather than after its supervisors have been let go, when
    // failing would leave a tab on screen with nothing attached to it.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let (_, terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(1)), crate::ClientId::new(1))
        .expect("the tab should be movable");
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

/// The receiving session lets a project name the tab unless the user renamed it here.
#[test]
fn a_tab_warp_carries_whether_the_user_named_the_tab() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .rename_by_user("notes".to_owned());

    let (_, _, born) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
        .expect("the tab should be movable");
    let (_, _, renamed) = screen
        .check_tab_can_be_warped(Some(TabId::new(1)), crate::ClientId::new(1))
        .expect("the tab should be movable");

    assert!(!born.user_named);
    assert_eq!(
        (renamed.name.as_deref(), renamed.user_named),
        (Some("notes"), true)
    );
}

#[test]
fn a_tab_of_nothing_but_plugin_panes_has_nothing_to_warp() {
    // Not a refusal on principle — a plugin pane rides along in the layout and is
    // restarted at the far end. But a tab made of nothing else has no process to
    // move, so moving it would be an elaborate way of opening a new tab.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(1)), false, None);

    assert_eq!(
        screen
            .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
            .err(),
        Some("this tab has no panes to move".to_owned())
    );
}

#[test]
fn a_tab_whose_ui_strip_reaches_past_it_can_be_warped() {
    // A tab whose ui strip reaches past the panes under it is not a rectangle,
    // so no split line spans it. That is not a layout, and it used to be refused
    // - "0 of its 4 panes could be placed", with every process in the tab stuck
    // in this session over the shape of a status bar.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    for pane_id in [2, 3] {
        tab.new_pane(
            PaneId::Terminal(crate::TerminalId::new(pane_id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    }
    // Terminal 2 is the tab's full-width bottom strip. Widened over the global
    // pane's eight columns the way `expand_ui_panes_over_global_strips` widens a
    // ui pane - written here rather than driven through a render, because what is
    // being tested is what a tab in that shape does, not when the shape appears.
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId::new(2)), false);
    let strip = tab
        .get_pane_with_id_mut(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST");
    let mut widened = strip.position_and_size();
    let reached_over = 8;
    assert_eq!(
        widened.x, 0,
        "terminal 2 is expected to be the tab's full-width bottom strip",
    );
    let cols = widened.cols.as_usize() + reached_over;
    widened.cols.set_inner(cols);
    strip.set_geom(widened);
    for pane_id in [
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    ] {
        let pane = tab.get_pane_with_id_mut(pane_id).expect("TEST");
        let mut inset = pane.position_and_size();
        inset.x += reached_over;
        pane.set_geom(inset);
    }

    // the premise: the ui strip reaches past the panes under it, so the tab is
    // not a rectangle and no layout describes it
    let tab = screen.tabs.get(&TabId::new(0)).expect("TEST");
    let strip = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("TEST")
        .position_and_size();
    let leftmost_pane_under_it = tab
        .get_tiled_panes()
        .filter(|(pane_id, _)| **pane_id != PaneId::Terminal(crate::TerminalId::new(2)))
        .map(|(_, pane)| pane.position_and_size().x)
        .min()
        .expect("TEST");
    assert!(
        strip.x < leftmost_pane_under_it,
        "a widened ui pane should reach past the panes under it \
         (strip at {}, panes at {leftmost_pane_under_it})",
        strip.x,
    );

    let (tab_index, mut terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
        .expect("a tab whose ui strip reaches past it should still be movable");
    assert_eq!(tab_index, TabId::new(0));
    terminal_ids.sort_unstable();
    assert_eq!(
        terminal_ids,
        vec![TerminalId::new(1), TerminalId::new(2), TerminalId::new(3)],
        "every pane in the tab should travel",
    );
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn a_hidden_stack_list_member_cannot_be_warped_refused() {
    let screen = screen_with_a_stack_list();
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(1))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
}

#[test]
fn warping_a_visible_stack_member_removes_the_whole_list() {
    let mut screen = screen_with_a_stack_list();
    let (to_pty, receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (tab_id, terminals, prepared) = screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(3))), ClientId::new(1))
        .expect("selection")
        .expect("companions");
    let slots = prepared.slots.clone();
    let released = terminals
        .into_iter()
        .map(|id| {
            (
                id,
                Ok(ReleasedPane::Local(ReleasedPtyHost {
                    host_id: SupervisorId::new(format!("host-{id}")),
                    host_pid: ProcessId::new(1),
                    control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
                })),
            )
        })
        .collect();
    let payload = screen
        .take_released_tab_for_warp(tab_id, released, prepared)
        .expect("transfer");
    assert!(payload.relationships.is_some());
    assert!(slots
        .iter()
        .all(|id| screen.tabs[&tab_id].get_pane_with_id(*id).is_none()));
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyInstruction::ClosePane(_, _))));
}

#[test]
fn the_sessions_last_stack_list_can_be_warped() {
    let mut screen = screen_with_a_stack_list();
    screen.tabs.retain(|id, _| *id == TabId::new(0));
    let (_, _, prepared) = screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(3))), ClientId::new(1))
        .expect("the last stack can leave the session")
        .expect("companions");
    assert_eq!(
        prepared.slots.len(),
        screen.tabs[&TabId::new(0)].get_all_pane_ids().len()
    );
}

#[test]
fn warping_a_hidden_editor_member_is_refused() {
    let screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(8))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
}

#[test]
fn warping_the_subject_of_a_hidden_editor_is_refused() {
    let screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(3))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
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
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.replace_pane_with_editor_pane(
        PaneId::Terminal(crate::TerminalId::new(8)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    )
    .expect("TEST");
    tab.focus_hidden_stack_list_member(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );

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
        suppressed.get(&PaneId::Terminal(crate::TerminalId::new(8))),
        Some(&None),
        "the editor took the member's place in the list and is parked as a member",
    );
    assert_eq!(
        suppressed.get(&PaneId::Terminal(crate::TerminalId::new(3))),
        Some(&Some(PaneId::Terminal(crate::TerminalId::new(8)))),
        "the pane being edited is suppressed behind the editor, wherever the editor is",
    );
    assert!(
        tab.pane_is_stack_list_member(&PaneId::Terminal(crate::TerminalId::new(8)))
            && !tab.pane_is_stack_list_member(&PaneId::Terminal(crate::TerminalId::new(3))),
        "the editor is the member; the pane behind it is not",
    );
}

#[test]
fn a_tab_with_a_stack_list_can_be_warped() {
    let mut screen = screen_with_a_stack_list();

    let (tab_index, mut terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
        .expect("a tab holding a stack list should be movable");
    terminal_ids.sort_unstable();
    assert_eq!(
        (tab_index, terminal_ids),
        (
            TabId::new(0),
            vec![TerminalId::new(1), TerminalId::new(2), TerminalId::new(3)]
        ),
        "every member travels, including the ones hidden behind the visible one",
    );
}

#[test]
fn a_tab_whose_stack_list_member_is_being_edited_can_be_warped() {
    // The hidden editor travels together with the subject it restores.
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    assert_hidden_warp(
        &terminals,
        &prepared,
        &[1, 2, 3, 8],
        &[(2, None, false), (3, Some(8), true), (8, None, false)],
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
    let tab = screen.tabs.get(&TabId::new(0)).expect("TEST");
    let in_the_grid: Vec<PaneId> = tab.get_tiled_panes().map(|(pane_id, _)| *pane_id).collect();
    assert_eq!(
        in_the_grid,
        vec![PaneId::Terminal(crate::TerminalId::new(3))],
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
        PaneId::Terminal(crate::TerminalId::new(1)),
        PaneId::Terminal(crate::TerminalId::new(2)),
        PaneId::Terminal(crate::TerminalId::new(3)),
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
        if member == PaneId::Terminal(crate::TerminalId::new(3)) {
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
fn a_warped_tab_carries_its_hidden_stack_list_relationships() {
    let mut screen = screen_with_a_stack_list();
    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("transfer");
    assert_eq!(
        prepared.tiled_panes.len() + prepared.floating_panes.len(),
        terminals.len()
    );
    let stack = &prepared
        .relationships
        .as_ref()
        .expect("relationships")
        .stacks[0];
    assert_eq!(
        stack
            .members
            .iter()
            .map(|i| prepared.slots[*i as usize])
            .collect::<Vec<_>>(),
        vec![
            PaneId::Terminal(TerminalId::new(1)),
            PaneId::Terminal(TerminalId::new(2)),
            PaneId::Terminal(TerminalId::new(3))
        ]
    );
    assert_eq!(
        prepared.slots[stack.selected as usize],
        PaneId::Terminal(TerminalId::new(3))
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

    screen.empty_and_close_tab(TabId::new(0));

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
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(9)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(crate::ClientId::new(1)),
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
        .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
        .expect("a tab with an overlay over it should still be movable");

    assert!(
        !terminal_ids.contains(&TerminalId::new(9)),
        "the overlay's supervisor must not be released, since nobody at the far \
         end would be holding it: {terminal_ids:?}",
    );
    assert!(
        !prepared
            .tiled_panes
            .iter()
            .chain(prepared.floating_panes.iter())
            .any(|pane_id| *pane_id == PaneId::Terminal(crate::TerminalId::new(9))),
        "and it has no slot in the arrangement either",
    );
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn warping_a_tab_re_homes_the_overlay_it_leaves_behind() {
    // The other half, and the one that used to strand a process. `empty_and_close_tab`
    // sweeps what was handed over and says nothing to the pty thread, because those
    // children belong to the other session now. The overlay was never handed over, so
    // sweeping it out here would take it out of the tab before `close_tab_by_id` could
    // see it - and that is the one thing that knows what to do with one. Left where it
    // is, it is re-homed into a surviving tab, which is right for a warp too: an
    // overlay has no slot in the arrangement that travelled, so it stays in this
    // session rather than following the tab or being killed for it.
    let mut screen = screen_with_an_overlay_over_a_tab();
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    screen.empty_and_close_tab(TabId::new(0));

    let closed_by_pty: Vec<PaneId> = pty_receiver
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PtyInstruction::CloseTab(pane_ids) => pane_ids,
            PtyInstruction::ClosePane(pane_id, _) => vec![pane_id],
            _ => vec![],
        })
        .collect();
    assert!(
        closed_by_pty.is_empty(),
        "nothing is closed here: the tab's own pane was warped, and the overlay moved \
         to another tab with its process intact: {closed_by_pty:?}",
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(1))
            .expect("TEST")
            .pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(9))),
        "and it is still an overlay in the tab it landed in",
    );
}

#[test]
fn warping_the_last_tab_closes_its_overlay() {
    // The other side of the gate: an overlay is only left behind if there is a tab to leave it
    // in. Warping away the session's only tab ends the session, so the overlay goes with it
    // rather than being extracted into nothing.
    let mut screen = screen_with_an_overlay_over_a_tab();
    screen.close_tab_by_id(TabId::new(1)).expect("TEST");
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    screen.empty_and_close_tab(TabId::new(0));

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
        vec![PaneId::Terminal(crate::TerminalId::new(9))],
        "with no tab left to re-home it into, the overlay is torn down like everything else",
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
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .close_pane(PaneId::Terminal(crate::TerminalId::new(1)), false, None);

    assert_eq!(
        screen
            .check_tab_can_be_warped(Some(TabId::new(0)), crate::ClientId::new(1))
            .err(),
        Some("this tab has no panes to move".to_owned())
    );
}

#[test]
fn a_tab_with_a_replacement_chain_can_be_warped() {
    // Ordinary replacements preserve the same restoration link as editors.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .suppress_pane_and_replace_with_pid(
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(9)),
            yinz_bus::pty::ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .expect("TEST");

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    assert_hidden_warp(&terminals, &prepared, &[1, 9], &[(1, Some(9), false)]);
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
            total < yinz_ipc::ipc::MAX_IPC_FRAME_LEN,
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
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let warped =
        released_remote_pane_for_warp(&mut screen, yinz_types::internal_ids::TerminalId::new(1));

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
        remote.cmd.target.ssh_host(),
        Some("build-box"),
        "the command travels with a remote pane: the adopting server has no \
         supervisor to learn it from",
    );
}

#[test]
fn a_remote_pane_carries_no_local_address() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let warped =
        released_remote_pane_for_warp(&mut screen, yinz_types::internal_ids::TerminalId::new(1));

    assert_eq!(
        warped.host_id,
        SupervisorId::new(""),
        "a remote pane was given a local supervisor id, which names nothing here",
    );
    assert_eq!(
        warped.control_socket,
        PathBuf::new(),
        "a remote pane was given a local socket path, which nothing is listening on",
    );
}

#[test]
fn warp_group_restores_a_hidden_editor_chain_and_selected_stack_member() {
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    new_tab(&mut screen, 99, TabId::new(1));
    let selected = PaneId::Terminal(TerminalId::new(1));
    let (_, _, prepared) = screen
        .prepare_pane_group(Some(selected), ClientId::new(1))
        .unwrap()
        .unwrap();
    let source = screen.tabs.get_mut(&TabId::new(0)).expect("source");
    let slots = source
        .warp_pane_ids(Some(selected))
        .expect("visible selection");
    assert_eq!(slots.len(), 4);
    for hidden in [3, 8] {
        assert!(source
            .warp_pane_ids(Some(PaneId::Terminal(TerminalId::new(hidden))))
            .is_err());
    }
    let relationships = source.warp_relationships(&slots);
    let original = source.arrangement_stacks();
    source.extract_warp_panes(&slots);
    assert!(slots
        .iter()
        .all(|id| source.get_pane_with_id(*id).is_none()));
    let mut destination_screen = create_new_screen(Size { cols: 80, rows: 8 }, false, false);
    let destination_id = TabId::new(0);
    destination_screen
        .new_tab(
            destination_id,
            (vec![], vec![]),
            None,
            Some(ClientId::new(1)),
        )
        .unwrap();
    destination_screen
        .pending_warp_relationships
        .insert(destination_id, relationships.clone());
    let terminal_ids: Vec<_> = prepared
        .slots
        .iter()
        .map(|id| match id {
            PaneId::Terminal(id) => (*id, None),
            PaneId::Plugin(_) => unreachable!(),
        })
        .collect();
    destination_screen
        .apply_layout(
            prepared.tiled_layout,
            prepared.floating_layout,
            terminal_ids[..1].to_vec(),
            terminal_ids[1..].to_vec(),
            HashMap::new(),
            destination_id,
            true,
            ClientId::new(1),
            None,
        )
        .expect("construct stack companions in a short tab");
    let destination = destination_screen.tabs.get_mut(&destination_id).unwrap();
    assert_eq!(destination.warp_relationships(&slots), relationships);
    let restored = destination.arrangement_stacks();
    assert_eq!(restored[0].members, original[0].members);
    assert_eq!(restored[0].visible, selected);
    let editor = PaneId::Terminal(TerminalId::new(8));
    assert!(destination.focus_hidden_stack_list_member(editor, ClientId::new(1)));
    destination.close_pane(editor, false, None);
    assert!(destination.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
}

#[test]
fn warp_mixed_floating_slots_preserve_repeated_plugin_id_order() {
    use yinz_types::data::{WarpHiddenPane, WarpRelationships};
    use yinz_types::input::layout::FloatingPaneLayout;
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let (to_plugin, _receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    let tab_id = TabId::new(0);
    screen
        .new_tab(tab_id, (vec![], vec![]), None, Some(ClientId::new(1)))
        .unwrap();
    let plugin = RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
    );
    let plugin_layout = |name: &str| FloatingPaneLayout {
        run: Some(Run::Plugin(plugin.clone())),
        name: Some(name.into()),
        ..Default::default()
    };
    screen.pending_warp_relationships.insert(
        tab_id,
        WarpRelationships {
            hidden: vec![
                WarpHiddenPane {
                    pane: 2,
                    restored_by: Some(0),
                    is_scrollback_editor: false,
                },
                WarpHiddenPane {
                    pane: 4,
                    restored_by: Some(3),
                    is_scrollback_editor: false,
                },
            ],
            stacks: vec![],
        },
    );
    let command = yinz_types::input::command::RunCommand {
        command: "/usr/bin/true".into(),
        ..Default::default()
    };
    screen
        .apply_layout(
            TiledPaneLayout {
                run: Some(Run::Plugin(plugin.clone())),
                ..Default::default()
            },
            vec![
                FloatingPaneLayout::default(),
                plugin_layout("hidden"),
                plugin_layout("visible"),
                FloatingPaneLayout {
                    run: Some(Run::Command(command.clone())),
                    name: Some("held subject".into()),
                    borderless: Some(true),
                    ..Default::default()
                },
            ],
            vec![],
            vec![
                (TerminalId::new(10), None),
                (TerminalId::new(11), Some(command.clone())),
            ],
            HashMap::from([(
                plugin,
                vec![
                    crate::PluginId::new(22),
                    crate::PluginId::new(21),
                    crate::PluginId::new(20),
                ],
            )]),
            tab_id,
            true,
            ClientId::new(1),
            None,
        )
        .expect("apply mixed slots");
    let tab = screen.tabs.get_mut(&tab_id).unwrap();
    let hidden = PaneId::Plugin(crate::PluginId::new(21));
    let visible = PaneId::Plugin(crate::PluginId::new(22));
    assert_eq!(
        tab.get_pane_with_id(hidden).unwrap().current_title(),
        "hidden"
    );
    assert_eq!(
        tab.get_pane_with_id(visible).unwrap().current_title(),
        "visible"
    );
    assert!(!tab.has_non_suppressed_pane_with_pid(&hidden));
    assert!(tab.has_non_suppressed_pane_with_pid(&visible));
    assert!(tab.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(10))));
    assert!(!tab.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(11))));
    let subject = tab
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(11)))
        .unwrap();
    assert_eq!(subject.current_title(), "held subject");
    assert_eq!(subject.invoked_with(), &Some(Run::Command(command)));
    assert!(subject.borderless());
    assert!(subject.is_held());
    tab.close_pane(PaneId::Plugin(crate::PluginId::new(20)), false, None);
    assert!(tab.has_non_suppressed_pane_with_pid(&hidden));
    tab.close_pane(visible, false, None);
    assert!(tab.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(11))));
}

#[test]
fn warp_adoption_preserves_explicit_floating_layouts_over_destination_template() {
    use yinz_types::data::{WarpPlacement, WarpedTab};
    use yinz_types::input::layout::FloatingPaneLayout;

    for placement in [Some(WarpPlacement::NewTab), None] {
        for floating_layout in [
            vec![],
            vec![FloatingPaneLayout {
                name: Some("transferred floating pane".into()),
                ..Default::default()
            }],
        ] {
            let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
            screen.default_layout.template = Some((
                TiledPaneLayout::default(),
                vec![FloatingPaneLayout::default()],
            ));
            let (to_plugin, receiver) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
            let plugin = RunPluginOrAlias::RunPlugin(
                yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
            );
            screen
                .place_adopted_tab(
                    WarpedTab {
                        source_instance_id: Some("source".into()),
                        tiled_layout: TiledPaneLayout {
                            tab_group: Some(yinz_types::tab_bar::TabGroupInfo {
                                id: 5,
                                name: "source group".into(),
                            }),
                            run: Some(Run::Plugin(plugin.clone())),
                            focus: Some(true),
                            ..Default::default()
                        },
                        floating_layout: floating_layout.clone(),
                        relationships: Some(Default::default()),
                        ..Default::default()
                    },
                    crate::pty::AdoptedTabPanes {
                        tiled: vec![],
                        floating: vec![],
                    },
                    ClientId::new(1),
                    false,
                    placement,
                    None,
                    None,
                )
                .expect("queue adoption");
            assert_eq!(screen.warped_tab_groups.is_empty(), placement.is_some());
            let (layout, transmitted_floating, tab_id, focus, client_id) = receiver
                .try_iter()
                .find_map(|(instruction, _)| match instruction {
                    PluginInstruction::NewTab(
                        _,
                        _,
                        layout,
                        floating,
                        tab,
                        _,
                        _,
                        focus,
                        client,
                        _,
                    ) => Some((layout.unwrap(), floating, tab, focus, client)),
                    _ => None,
                })
                .expect("adoption requested a new tab");
            assert_eq!(transmitted_floating, Some(floating_layout.clone()));
            if !floating_layout.is_empty() {
                continue;
            }

            // Resolve the same fallback as the plugin thread, then apply only the allocated ID.
            let floating =
                transmitted_floating.unwrap_or_else(|| screen.default_layout.new_tab().1);
            let plugin_id = crate::PluginId::new(150);
            screen
                .apply_layout(
                    layout,
                    floating,
                    vec![],
                    vec![],
                    HashMap::from([(plugin, vec![plugin_id])]),
                    tab_id,
                    focus,
                    client_id,
                    None,
                )
                .expect("apply standalone plugin without destination template slots");
            let pane_id = PaneId::Plugin(plugin_id);
            assert!(screen.tabs[&tab_id].has_non_suppressed_pane_with_pid(&pane_id));
            assert_eq!(screen.get_active_pane_id(&client_id), Some(pane_id));
        }
    }
}

fn queue_warp_group(
    screen: &mut Screen,
    placement: yinz_types::data::WarpPlacement,
) -> (TabId, crossbeam::channel::Receiver<anyhow::Result<()>>) {
    let (tx, rx) = crossbeam::channel::bounded(1);
    let tab_id = screen.get_new_tab_id();
    screen
        .place_adopted_tab(
            yinz_types::data::WarpedTab {
                relationships: Some(Default::default()),
                ..Default::default()
            },
            crate::pty::AdoptedTabPanes {
                tiled: vec![],
                floating: vec![],
            },
            ClientId::new(1),
            false,
            Some(placement),
            Some(tx),
            None,
        )
        .expect("queue group placement");
    assert!(matches!(
        rx.try_recv(),
        Err(crossbeam::channel::TryRecvError::Empty)
    ));
    (tab_id, rx)
}

fn apply_warp_group(screen: &mut Screen, tab_id: TabId) {
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(50), None)],
            vec![],
            HashMap::new(),
            tab_id,
            true,
            ClientId::new(1),
            None,
        )
        .expect("apply adopted layout");
}

#[test]
fn warp_group_acknowledges_new_tab_fallback() {
    use yinz_types::data::WarpPlacement;
    for placement in [WarpPlacement::FocusedTab, WarpPlacement::Floating] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
        new_tab(&mut screen, 1, TabId::new(0));
        let (tab_id, rx) = queue_warp_group(&mut screen, placement);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .resize_whole_tab(Size { cols: 4, rows: 2 })
            .unwrap();
        apply_warp_group(&mut screen, tab_id);
        rx.try_recv()
            .expect("placement reply")
            .expect("new tab fallback succeeded");
        assert!(
            screen.tabs[&tab_id].has_non_suppressed_pane_with_pid(&PaneId::Terminal(TERMINAL_50))
        );
    }
}

#[test]
fn warp_group_uses_destination_default_floating_geometry() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    let ordinary = PaneId::Terminal(TerminalId::new(2));
    tab.new_pane(
        ordinary,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    let expected = tab.get_pane_with_id(ordinary).unwrap().current_geom();
    tab.extract_pane(ordinary, false).unwrap();
    let (tab_id, rx) = queue_warp_group(&mut screen, yinz_types::data::WarpPlacement::Floating);
    apply_warp_group(&mut screen, tab_id);
    rx.try_recv()
        .expect("placement reply")
        .expect("placement succeeded");
    let pane = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(TERMINAL_50))
        .unwrap();
    assert_eq!(
        (pane.x(), pane.y(), pane.cols(), pane.rows()),
        (
            expected.x,
            expected.y,
            expected.cols.as_usize(),
            expected.rows.as_usize()
        )
    );
}

#[test]
fn warp_group_acknowledges_new_tab_and_missing_destination() {
    for placement in [
        yinz_types::data::WarpPlacement::NewTab,
        yinz_types::data::WarpPlacement::FocusedTab,
    ] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
        let (tab_id, rx) = queue_warp_group(&mut screen, placement);
        apply_warp_group(&mut screen, tab_id);
        rx.try_recv()
            .expect("placement reply")
            .expect("new tab fallback");
        assert!(
            screen.tabs[&tab_id].has_non_suppressed_pane_with_pid(&PaneId::Terminal(TERMINAL_50))
        );
    }
}

#[test]
fn warp_floating_stack_requires_room_for_headers_and_content() {
    for floating_rows in [6, 5] {
        let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
        let selected = PaneId::Terminal(TerminalId::new(1));
        let source_id = TabId::new(0);
        let target_id = TabId::new(1);
        let (_, _, prepared) = screen
            .prepare_pane_group(Some(selected), ClientId::new(1))
            .unwrap()
            .unwrap();
        let slots = prepared.slots;
        let relationships = prepared.relationships.unwrap();
        assert_eq!(relationships.stacks[0].members.len(), 3);
        assert_eq!(slots.len(), 4); // Three members plus the hidden editor's subject.
        screen
            .resize_to_screen(Size {
                cols: 80,
                rows: floating_rows * 2,
            })
            .unwrap();
        let destination = &screen.tabs[&target_id];
        let original_ids = destination.get_all_pane_ids();
        let original_geom = destination
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(9)))
            .unwrap()
            .position_and_size();
        let (tx, rx) = crossbeam::channel::bounded(1);
        screen.pending_group_placements.insert(
            source_id,
            (
                yinz_types::data::WarpPlacement::Floating,
                Some(target_id),
                ClientId::new(1),
                Some(tx),
                None,
            ),
        );
        screen
            .finish_warp_group(source_id, &slots, &relationships)
            .unwrap();
        rx.try_recv().expect("placement acknowledged").unwrap();
        let placed = floating_rows == 6;
        let retained_id = if placed { target_id } else { source_id };
        assert_eq!(screen.tabs.contains_key(&source_id), !placed);
        let destination = &screen.tabs[&target_id];
        assert_eq!(
            destination
                .get_pane_with_id(PaneId::Terminal(TerminalId::new(9)))
                .unwrap()
                .position_and_size(),
            original_geom
        );
        if !placed {
            assert_eq!(destination.get_all_pane_ids(), original_ids);
            assert!(!destination.are_floating_panes_visible());
        }
        let retained = screen.tabs.get_mut(&retained_id).unwrap();
        assert!(slots
            .iter()
            .all(|id| retained.get_pane_with_id(*id).is_some()));
        assert_eq!(retained.warp_relationships(&slots), relationships);
        if placed {
            let pane = retained.get_pane_with_id(selected).unwrap();
            assert_eq!(pane.rows(), floating_rows);
            assert_eq!(pane.get_content_rows(), 1);
            let offset = pane.get_content_offset();
            assert_eq!(offset.top, relationships.stacks[0].members.len() + 1);
            assert!(pane.y() + offset.top < pane.y() + pane.rows());
            assert!(pane.y() + pane.rows() <= retained.get_viewport().rows);
        }
        let editor = PaneId::Terminal(TerminalId::new(8));
        assert!(retained.focus_hidden_stack_list_member(editor, ClientId::new(1)));
        retained.close_pane(editor, false, None);
        assert!(retained.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
    }
}

fn screen_with_a_transferred_floating_stack() -> Screen {
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
    let selected = PaneId::Terminal(TerminalId::new(1));
    let source = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    source
        .rename_pane_by_pane_id(
            PaneId::Terminal(TerminalId::new(3)),
            b"editor subject".to_vec(),
        )
        .unwrap();
    let slots = source.warp_pane_ids(Some(selected)).unwrap();
    let relationships = source.warp_relationships(&slots);
    let panes = source.extract_warp_panes(&slots);
    let destination = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    for pane in panes {
        let id = pane.pid();
        if id == selected {
            destination
                .add_floating_pane(pane, id, None, true, Some(ClientId::new(1)))
                .unwrap();
        } else {
            destination.add_suppressed_panes(HashMap::from([(
                id,
                crate::tab::SuppressedPane {
                    pane,
                    restored_by: None,
                },
            )]));
        }
    }
    destination
        .restore_warp_relationships(&slots, &relationships)
        .unwrap();
    screen
}

fn render_floating_stack(tab: &mut crate::tab::Tab) -> String {
    let mut output = yinz_terminal::output::Output::new(
        Default::default(),
        Default::default(),
        true,
        true,
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    tab.render_panes(&mut output, Some(ClientId::new(1)))
        .unwrap();
    tab.render_cursor_last(&mut output, Some(ClientId::new(1)));
    let rendered = output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default();
    let mut grid = grid_for_render_snapshot(Size { cols: 80, rows: 24 });
    take_snapshot_and_cursor_coordinates(&rendered, &mut grid).1
}

#[test]
fn floating_stack_resize_synchronizes_hidden_members_without_redundant_requests() {
    use yinz_types::geometry::{Direction, Resize, ResizeStrategy};

    let mut screen = screen_with_a_transferred_floating_stack();
    let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    let displayed = PaneId::Terminal(TerminalId::new(1));
    let (tx, rx) = channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    tab.senders.to_pty_writer = Some(SenderWithContext::new(tx));
    tab.show_floating_panes();
    render_floating_stack(tab);
    let offset = tab
        .get_pane_with_id(displayed)
        .unwrap()
        .get_content_offset();

    for direction in [Direction::Right, Direction::Down] {
        let before = tab.get_pane_with_id(displayed).unwrap().current_geom();
        tab.resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(direction)),
            displayed,
        )
        .unwrap();
        rx.try_iter().for_each(drop);
        render_floating_stack(tab);
        let visible = tab.get_pane_with_id(displayed).unwrap();
        assert_ne!(visible.current_geom(), before);
        assert_eq!(visible.get_content_offset(), offset);
        let dimensions = (visible.get_content_columns(), visible.get_content_rows());
        let visible_geom = visible.current_geom();
        let resizes: Vec<_> = rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyWriteInstruction::ResizePty(id, cols, rows, _, _) => Some((id, cols, rows)),
                _ => None,
            })
            .collect();
        for id in [2, 8].map(TerminalId::new) {
            let hidden = tab.get_pane_with_id(PaneId::Terminal(id)).unwrap();
            assert_eq!(hidden.current_geom(), visible_geom);
            assert_eq!(hidden.get_content_offset(), offset);
            assert_eq!(
                (hidden.get_content_columns(), hidden.get_content_rows()),
                dimensions
            );
            assert_eq!(
                resizes
                    .iter()
                    .filter(|(resized, _, _)| *resized == id)
                    .copied()
                    .collect::<Vec<_>>(),
                vec![(id, dimensions.0 as u16, dimensions.1 as u16)]
            );
            // Cursor clamping observes the grid's actual size, not just pane geometry.
            tab.handle_pty_bytes(id, b"\x1b[999;999H".to_vec()).unwrap();
            assert_eq!(
                tab.get_pane_with_id(PaneId::Terminal(id))
                    .unwrap()
                    .cursor_coordinates(None)
                    .map(|(x, y, _)| (x, y)),
                Some((
                    dimensions.0 - 1 + offset.left,
                    dimensions.1 - 1 + offset.top
                ))
            );
        }
        render_floating_stack(tab);
        assert!(!rx.try_iter().any(|(instruction, _)| matches!(instruction,
            PtyWriteInstruction::ResizePty(id, ..) if [TerminalId::new(2), TerminalId::new(8)].contains(&id))));
    }
}

#[test]
fn floating_stack_shrink_hides_headers_and_growth_restores_mouse_selection() {
    use yinz_types::geometry::FloatingPaneCoordinates;
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;

    for terminal_resize in [false, true] {
        let mut screen = screen_with_a_transferred_floating_stack();
        let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
        let client = ClientId::new(1);
        let displayed = PaneId::Terminal(TerminalId::new(1));
        tab.show_floating_panes();
        tab.rename_pane_by_pane_id(
            PaneId::Terminal(TerminalId::new(2)),
            b"hidden header".to_vec(),
        )
        .unwrap();
        assert!(render_floating_stack(tab).contains("hidden header"));
        tab.handle_pty_bytes(
            TerminalId::new(1),
            (0..50)
                .map(|i| format!("line {i}\r\n"))
                .collect::<String>()
                .into_bytes(),
        )
        .unwrap();
        if terminal_resize {
            tab.resize_whole_tab(Size { cols: 80, rows: 5 }).unwrap();
        } else {
            tab.change_floating_pane_coordinates(
                &displayed,
                FloatingPaneCoordinates::new(None, None, None, Some("5".to_owned()), None, None)
                    .unwrap(),
            )
            .unwrap();
        }
        let rendered = render_floating_stack(tab);
        assert!(!rendered.contains("hidden header"));
        assert!(rendered.contains("line 49"));
        let pane = tab.get_pane_with_id(displayed).unwrap();
        assert_eq!(pane.rows(), 5);
        assert_eq!(
            pane.get_content_offset(),
            yinz_types::pane_size::Offset::frame(1)
        );
        assert_eq!(pane.get_content_rows(), 3);
        let geom = pane.current_geom();
        let content = Position::new((geom.y + 1) as i32, (geom.x + 2) as u16);
        for event in [
            MouseEvent::new_left_press_event(content),
            MouseEvent::new_left_release_event(content),
            MouseEvent::new_scroll_up_event(content),
        ] {
            tab.handle_mouse_event_with_passthrough(&event, client, None)
                .unwrap();
        }
        assert_eq!(tab.get_active_pane_id(client), Some(displayed));
        assert!(tab.get_pane_with_id(displayed).unwrap().scroll_position().0 > 0);
        for id in [2, 8].map(|id| PaneId::Terminal(TerminalId::new(id))) {
            assert!(tab.pane_is_hidden_stack_list_member(&id));
            assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_rows(), 3);
        }
        if terminal_resize {
            tab.resize_whole_tab(Size { cols: 80, rows: 24 }).unwrap();
        }
        // Six rows fit exactly: three headers, padding, content, bottom frame.
        tab.change_floating_pane_coordinates(
            &displayed,
            FloatingPaneCoordinates::new(None, None, None, Some("6".to_owned()), None, None)
                .unwrap(),
        )
        .unwrap();
        assert!(render_floating_stack(tab).contains("hidden header"));
        let pane = tab.get_pane_with_id(displayed).unwrap();
        assert_eq!(pane.get_content_offset().top, 4);
        assert_eq!(pane.get_content_rows(), 1);
        let geom = pane.current_geom();
        let header = Position::new((geom.y + 1) as i32, (geom.x + 2) as u16);
        for event in [
            MouseEvent::new_left_press_event(header),
            MouseEvent::new_left_release_event(header),
        ] {
            tab.handle_mouse_event_with_passthrough(&event, client, None)
                .unwrap();
        }
        assert_eq!(
            tab.get_active_pane_id(client),
            Some(PaneId::Terminal(TerminalId::new(2)))
        );
        assert!(tab.pane_is_hidden_stack_list_member(&displayed));
    }
}

#[test]
fn mouse_selects_hidden_members_of_a_transferred_floating_stack() {
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;

    for pinned in [false, true] {
        let mut screen = screen_with_a_transferred_floating_stack();
        let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
        let client_id = ClientId::new(1);
        let mut selected = PaneId::Terminal(TerminalId::new(1));
        let slots = tab.warp_pane_ids(Some(selected)).unwrap();
        let members: Vec<_> = tab.warp_relationships(&slots).stacks[0]
            .members
            .iter()
            .map(|slot| slots[*slot as usize])
            .collect();
        tab.show_floating_panes();
        for member in &members {
            tab.set_floating_pane_pinned(*member, pinned);
        }

        for id in [2, 8, 1].map(|id| PaneId::Terminal(TerminalId::new(id))) {
            if pinned {
                tab.hide_floating_panes();
            }
            let geom = tab.get_pane_with_id(selected).unwrap().current_geom();
            let rank = members.iter().position(|member| *member == id).unwrap();
            let header = Position::new((geom.y + rank) as i32, (geom.x + 2) as u16);
            assert!(tab.pane_is_hidden_stack_list_member(&id));

            tab.handle_mouse_event_with_passthrough(
                &MouseEvent::new_buttonless_motion(header),
                client_id,
                None,
            )
            .unwrap();
            assert!(tab.pane_is_hidden_stack_list_member(&id));
            assert!(tab
                .get_floating_panes()
                .any(|(pane_id, _)| *pane_id == selected));
            assert_eq!(tab.are_floating_panes_visible(), !pinned);

            for event in [
                MouseEvent::new_left_press_event(header),
                MouseEvent::new_left_release_event(header),
            ] {
                tab.handle_mouse_event_with_passthrough(&event, client_id, None)
                    .unwrap();
            }
            assert_eq!(tab.get_active_pane_id(client_id), Some(id));
            assert!(!tab.pane_is_hidden_stack_list_member(&id));
            assert!(tab.pane_is_hidden_stack_list_member(&selected));
            assert!(tab.get_floating_panes().any(|(pane_id, _)| *pane_id == id));
            assert!(tab.are_floating_panes_visible());
            assert_eq!(
                tab.get_pane_with_id(id).unwrap().current_geom().is_pinned,
                pinned
            );

            let content = Position::new((geom.y + members.len() + 1) as i32, (geom.x + 2) as u16);
            for event in [
                MouseEvent::new_left_press_event(content),
                MouseEvent::new_left_release_event(content),
            ] {
                tab.handle_mouse_event_with_passthrough(&event, client_id, None)
                    .unwrap();
            }
            assert_eq!(tab.get_active_pane_id(client_id), Some(id));
            assert!(!tab.pane_is_hidden_stack_list_member(&id));
            selected = id;
        }
    }
}

#[test]
fn fullscreen_floating_stack_content_targets_the_displayed_member() {
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;

    for no_ui in [false, true] {
        let mut screen = screen_with_a_transferred_floating_stack();
        let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
        let client_id = ClientId::new(1);
        let displayed = PaneId::Terminal(TerminalId::new(1));
        let hidden = [2, 8].map(|id| PaneId::Terminal(TerminalId::new(id)));
        let slots = tab.warp_pane_ids(Some(displayed)).unwrap();
        let members: Vec<_> = tab.warp_relationships(&slots).stacks[0]
            .members
            .iter()
            .map(|slot| slots[*slot as usize])
            .collect();
        tab.show_floating_panes();
        tab.handle_pty_bytes(
            TerminalId::new(1),
            (0..50)
                .map(|line| format!("line {line}\r\n"))
                .collect::<String>()
                .into_bytes(),
        )
        .unwrap();
        if no_ui {
            tab.toggle_active_pane_no_ui_fullscreen(client_id);
        } else {
            tab.toggle_active_pane_fullscreen(client_id);
        }

        let geom = tab.get_pane_with_id(displayed).unwrap().current_geom();
        let content = Position::new((geom.y + 1) as i32, (geom.x + 2) as u16);
        for event in [
            MouseEvent::new_left_press_event(content),
            MouseEvent::new_left_release_event(content),
            MouseEvent::new_scroll_up_event(content),
        ] {
            tab.handle_mouse_event_with_passthrough(&event, client_id, None)
                .unwrap();
        }
        assert_eq!(tab.get_active_pane_id(client_id), Some(displayed));
        assert!(!tab.pane_is_hidden_stack_list_member(&displayed));
        assert!(hidden
            .iter()
            .all(|id| tab.pane_is_hidden_stack_list_member(id)));
        assert!(tab.get_pane_with_id(displayed).unwrap().scroll_position().0 > 0);

        if no_ui {
            tab.toggle_active_pane_no_ui_fullscreen(client_id);
        } else {
            tab.toggle_active_pane_fullscreen(client_id);
        }
        let geom = tab.get_pane_with_id(displayed).unwrap().current_geom();
        let header = Position::new((geom.y + 1) as i32, (geom.x + 2) as u16);
        let selected = members[1];
        assert!(tab.pane_is_hidden_stack_list_member(&selected));
        for event in [
            MouseEvent::new_left_press_event(header),
            MouseEvent::new_left_release_event(header),
        ] {
            tab.handle_mouse_event_with_passthrough(&event, client_id, None)
                .unwrap();
        }
        assert_eq!(tab.get_active_pane_id(client_id), Some(selected));
        assert!(!tab.pane_is_hidden_stack_list_member(&selected));
        assert!(tab.pane_is_hidden_stack_list_member(&displayed));
    }
}

#[test]
fn tiled_mutations_preserve_a_transferred_floating_stack() {
    let mut screen = screen_with_a_transferred_floating_stack();
    let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(10)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    tab.move_pane_by_pane_id(PaneId::Terminal(TerminalId::new(9)), None);
    for member in [2, 8, 1] {
        let id = PaneId::Terminal(TerminalId::new(member));
        assert!(tab.focus_hidden_stack_list_member(id, ClientId::new(1)));
        assert!(tab.get_floating_panes().any(|(pane_id, _)| *pane_id == id));
    }
}

#[test]
fn floating_stack_layout_serialization_keeps_every_subject_on_the_floating_layer() {
    let screen = screen_with_a_transferred_floating_stack();
    let tab_id = TabId::new(1);
    let tab = &screen.tabs[&tab_id];
    assert!(tab.stack_list_serialization_geoms().is_empty());
    let full_rect = tab
        .get_floating_panes()
        .next()
        .unwrap()
        .1
        .position_and_size();
    let (tiled, floating, _) = screen.tab_panes_for_layout(tab_id, tab, true, false);
    assert_eq!(
        tiled.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![PaneId::Terminal(TerminalId::new(9))]
    );
    assert_eq!(
        floating.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![
            PaneId::Terminal(TerminalId::new(2)),
            PaneId::Terminal(TerminalId::new(3)),
            PaneId::Terminal(TerminalId::new(1))
        ]
    );
    for (_, metadata) in floating {
        let manifest: yinz_types::session_manifest::PaneLayoutManifest = metadata.into();
        assert_eq!(manifest.geom, full_rect);
        assert_eq!(manifest.geom.stacked, None);
    }
    let (dump, _) = yinz_session::session_serialization::serialize_session_layout(
        screen.get_layout_metadata(None, Some(tab_id)).into(),
    )
    .unwrap();
    let layout = yinz_config::kdl::layout::from_kdl(&dump, None, None, None).unwrap();
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.tabs[0].2.len(), 3);
    assert_eq!(layout.tabs[0].2[1].name.as_deref(), Some("editor subject"));
    for pane in &layout.tabs[0].2 {
        use yinz_types::geometry::PercentOrFixed;
        assert_eq!(pane.x, Some(PercentOrFixed::Fixed(full_rect.x)));
        assert_eq!(pane.y, Some(PercentOrFixed::Fixed(full_rect.y)));
        assert_eq!(
            pane.width,
            Some(PercentOrFixed::Fixed(full_rect.cols.as_usize()))
        );
        assert_eq!(
            pane.height,
            Some(PercentOrFixed::Fixed(full_rect.rows.as_usize()))
        );
    }
}

#[test]
fn companion_capability_refusal_and_expired_group_preserve_every_pane() {
    for scenario in [
        "old pane destination",
        "old tab destination",
        "expired group",
    ] {
        let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
        let pty = mock.pty_receiver.take().unwrap();
        let client_id = mock.main_client_id;
        let thread = mock.run(
            Some(TiledPaneLayout {
                children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
                ..Default::default()
            }),
            vec![],
        );
        mock.to_screen
            .send(ScreenInstruction::OpenInPlaceEditor(
                PaneId::Terminal(TerminalId::new(9)),
                ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(TerminalId::new(1))),
            ))
            .unwrap();
        if scenario == "old tab destination" {
            let (tx, rx) = crossbeam::channel::bounded(1);
            mock.to_screen
                .send(ScreenInstruction::WarpTab {
                    supports_companions: false,
                    supports_held: true,
                    tab_id: Some(TabId::new(0)),
                    client_id,
                    response_channel: tx,
                    deadline: warp_deadline(),
                })
                .unwrap();
            assert!(rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap_err()
                .contains("does not support companion"));
        } else {
            let (tx, rx) = yinz_bus::warp::pane_warp_reply();
            mock.to_screen
                .send(ScreenInstruction::WarpPane {
                    supports_companions: scenario == "expired group",
                    supports_held: true,
                    pane_id: Some(PaneId::Terminal(TerminalId::new(9))),
                    to_session: "elsewhere".to_owned().into(),
                    placement: yinz_types::data::WarpPlacement::FocusedTab,
                    client_id,
                    response_channel: tx,
                    deadline: if scenario == "expired group" {
                        std::time::Instant::now() - std::time::Duration::from_secs(1)
                    } else {
                        warp_deadline()
                    },
                })
                .unwrap();
            if scenario == "expired group" {
                let released = loop {
                    let (instruction, _) =
                        pty.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                    if let PtyInstruction::ReleaseTerminalsForWarp { terminal_ids, .. } =
                        instruction
                    {
                        break terminal_ids;
                    }
                };
                let mut ids = released.clone();
                ids.sort_unstable();
                assert_eq!(ids, vec![TerminalId::new(1), TerminalId::new(9)]);
                mock.to_screen
                    .send(ScreenInstruction::TabReleasedForWarp {
                        tab_index: TabId::new(0),
                        released: released
                            .iter()
                            .map(|id| {
                                (
                                    *id,
                                    Ok(ReleasedPane::Local(ReleasedPtyHost {
                                        host_id: SupervisorId::new(format!("companion-{id}")),
                                        host_pid: ProcessId::new(1),
                                        control_socket: Some(PathBuf::from(format!(
                                            "/tmp/companion-{id}.sock"
                                        ))),
                                    })),
                                )
                            })
                            .collect(),
                    })
                    .unwrap();
                let mut recovered = vec![];
                while recovered.len() < 2 {
                    let (instruction, _) =
                        pty.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                    match instruction {
                        PtyInstruction::StrandPaneAfterWarp {
                            terminal_id,
                            recovery,
                            ..
                        } => {
                            assert!(recovery
                                .unwrap()
                                .contains(&format!("companion-{terminal_id}")));
                            recovered.push(terminal_id);
                        },
                        PtyInstruction::ClosePane(..) | PtyInstruction::CloseTab(..) => {
                            panic!("closed released companion")
                        },
                        _ => {},
                    }
                }
                recovered.sort_unstable();
                assert_eq!(recovered, ids);
                assert!(rx.try_recv().is_err());
            } else {
                assert!(rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
                    .unwrap_err()
                    .contains("does not support companion"));
            }
        }
        // A synchronous query is a barrier: all warp handling precedes these observations.
        for (id, hidden) in [(1, true), (9, false), (0, false)] {
            let (tx, rx) = crossbeam::channel::bounded(1);
            mock.to_screen
                .send(ScreenInstruction::GetPaneInfo {
                    pane_id: PaneId::Terminal(TerminalId::new(id)),
                    response_channel: tx,
                })
                .unwrap();
            let pane = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .expect("pane retained");
            assert_eq!(pane.is_suppressed, hidden);
        }
        assert!(
            pty.try_iter().all(|(instruction, _)| !matches!(
                instruction,
                PtyInstruction::ClosePane(..)
                    | PtyInstruction::CloseTab(..)
                    | PtyInstruction::ReleaseTerminalForWarp { .. }
                    | PtyInstruction::ReleaseTerminalsForWarp { .. }
            )),
            "refusal must precede release and preserve processes"
        );
        mock.teardown(vec![thread]);
    }
}

#[test]
fn warp_held_revalidation_recovers_every_released_peer() {
    for group in [false, true] {
        for held_first in [false, true] {
            for failure in ["changed", "closed", "pty failure"] {
                let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();
                let held_id = TerminalId::new(1);
                let preserved_id = PaneId::Terminal(TerminalId::new(3));
                for id in [PaneId::Terminal(held_id), preserved_id] {
                    screen
                        .tabs
                        .get_mut(&TAB_0)
                        .unwrap()
                        .get_pane_with_id_mut(id)
                        .unwrap()
                        .hold(Some(17), false, RunCommand::default());
                }
                let (tab_id, ids, prepared) = if group {
                    screen
                        .prepare_pane_group(Some(PaneId::Terminal(held_id)), ClientId::new(1))
                        .unwrap()
                        .unwrap()
                } else {
                    screen
                        .check_tab_can_be_warped(Some(TAB_0), ClientId::new(1))
                        .unwrap()
                };
                let held: HashMap<_, _> = ids
                    .iter()
                    .filter_map(|id| {
                        screen.tabs[&tab_id]
                            .get_pane_with_id(PaneId::Terminal(*id))
                            .unwrap()
                            .held_for_warp()
                            .map(|held| (*id, held))
                    })
                    .collect();
                assert_eq!(held.len(), 2);
                let mut released: Vec<_> = ids
                    .into_iter()
                    .map(|id| {
                        let pane = match held.get(&id) {
                            Some(held) => ReleasedPane::Held(held.clone()),
                            None => ReleasedPane::Local(ReleasedPtyHost {
                                host_id: format!("host-{id}").into(),
                                host_pid: ProcessId::new(1),
                                control_socket: Some(format!("/tmp/warp-{id}.sock").into()),
                            }),
                        };
                        (id, Ok(pane))
                    })
                    .collect();
                let expected: HashMap<_, _> = released
                    .iter()
                    .filter(|(id, _)| !held.contains_key(id))
                    .map(|(id, result)| (*id, result.as_ref().unwrap().recovery_command()))
                    .collect();
                assert_eq!(expected.len(), 2);
                released.sort_by_key(|(id, _)| (*id == held_id) != held_first);
                let tab = screen.tabs.get_mut(&tab_id).unwrap();
                if failure == "closed" {
                    tab.close_pane(PaneId::Terminal(held_id), false, None);
                } else {
                    tab.get_pane_with_id_mut(PaneId::Terminal(held_id))
                        .unwrap()
                        .hold(Some(23), false, RunCommand::default());
                }
                if failure == "pty failure" {
                    released
                        .iter_mut()
                        .find(|(id, _)| *id == TerminalId::new(2))
                        .unwrap()
                        .1 = Err("release failed".into());
                }
                let remaining = tab.get_all_pane_ids();
                let (to_pty, receiver) = channels::unbounded();
                screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
                let error = screen
                    .take_released_tab_for_warp(tab_id, released, prepared)
                    .unwrap_err();
                assert!(error.contains(if failure == "pty failure" {
                    "release failed"
                } else {
                    "held state changed"
                }));
                let recovery: HashMap<_, _> = receiver
                    .try_iter()
                    .map(|(instruction, _)| match instruction {
                        PtyInstruction::StrandPaneAfterWarp {
                            terminal_id,
                            recovery,
                            why,
                        } => {
                            assert!(why.contains("held state changed"));
                            (terminal_id, recovery.unwrap())
                        },
                        other => panic!("unexpected instruction: {other:?}"),
                    })
                    .collect();
                if failure == "pty failure" {
                    assert!(recovery.is_empty(), "PTY already recovered detached peers");
                } else {
                    assert_eq!(recovery, expected);
                }
                let tab = &screen.tabs[&tab_id];
                assert_eq!(tab.get_all_pane_ids(), remaining);
                assert_eq!(
                    tab.get_pane_with_id(preserved_id).unwrap().held_for_warp(),
                    Some(held[&TerminalId::new(3)].clone())
                );
            }
        }
    }
}

#[test]
fn warp_adopted_agent_exit_takes_precedence_over_transferred_state() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    for state in [AgentState::Busy, AgentState::Waiting] {
        for kind in ["live", "exited held", "first run held", "close"] {
            let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
            new_tab(&mut screen, 1, TAB_0);
            let pane_id = PaneId::Terminal(TERMINAL_50);
            add_pane_to_tab(&mut screen, TAB_0, pane_id, false);
            let cmd = RunCommand {
                command: "codex".into(),
                hold_on_close: kind != "close",
                ..Default::default()
            };
            let held = kind
                .contains("held")
                .then(|| yinz_types::data::HeldTerminal {
                    command: cmd.clone(),
                    first_run: kind == "first run held",
                    exit_status: Some(3),
                });
            let agent = AgentInfo {
                kind: "codex".into(),
                state,
                source: AgentSource::Reported,
                detail: Some("working".into()),
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: Some("/work".into()),
                session_id: Some("session".into()),
                run_id: Some("run".into()),
            };
            let (to_screen, receiver) = channels::unbounded();
            screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
            screen
                .place_adopted_pane(
                    TERMINAL_50,
                    WarpedPane {
                        agent_theme: None,
                        agent: Some(agent.clone()),
                        held: held.clone(),
                        ..Default::default()
                    },
                    yinz_types::data::WarpPlacement::FocusedTab,
                    ClientId::new(1),
                    cmd,
                    vec![],
                    (kind != "first run held").then_some(Some(3)),
                    &mut None,
                )
                .unwrap();
            // Apply the queued state changes in order, using the same pane methods as Screen.
            for (instruction, _) in receiver.try_iter() {
                let tab = screen.tabs.get_mut(&TAB_0).unwrap();
                match instruction {
                    ScreenInstruction::RequestPaneRedraw(_) => {
                        panic!("held or exited transfer requested redraw")
                    },
                    ScreenInstruction::SetPaneAgent(id, agent, _) => {
                        tab.set_pane_agent(id, agent);
                    },
                    ScreenInstruction::HoldPane(id, status, cmd) => {
                        tab.hold_pane(id, status, false, cmd);
                    },
                    ScreenInstruction::RestoreHeldPane(id, held) => {
                        screen.restore_held_pane(id, &held);
                    },
                    ScreenInstruction::ClosePane(id, _, _, status) => {
                        tab.close_pane(id, false, status);
                    },
                    ScreenInstruction::NewPane(id, _, Some(cmd), ..) => {
                        tab.hold_pane(id, None, true, cmd);
                    },
                    _ => {},
                }
            }
            let pane = screen.tabs[&TAB_0].get_pane_with_id(pane_id);
            if kind == "close" {
                assert!(pane.is_none());
            } else {
                let pane = pane.unwrap();
                let mut expected = agent;
                if held.is_none() {
                    expected.state = AgentState::Exited;
                    expected.detail = None;
                }
                assert!(pane.is_held());
                assert_eq!(pane.agent_info(), Some(&expected), "{kind}");
            }
        }
    }
}

#[test]
fn warp_held_state_survives_single_pane_tab_and_companion_transfers() {
    for first_run in [true, false] {
        for kind in ["pane", "tab", "companion"] {
            let mut screen = if kind == "companion" {
                screen_with_a_scrollback_editor_open()
            } else {
                let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
                new_tab(&mut screen, 1, TabId::new(0));
                new_tab(&mut screen, 2, TabId::new(1));
                screen
            };
            let pane_id = PaneId::Terminal(TerminalId::new(1));
            let command = RunCommand {
                command: "/usr/bin/true".into(),
                cwd: Some("/tmp".into()),
                ..Default::default()
            };
            let pane = screen
                .tabs
                .get_mut(&TAB_0)
                .unwrap()
                .get_pane_with_id_mut(pane_id)
                .unwrap();
            pane.hold((!first_run).then_some(17), first_run, command.clone());
            pane.rename(b"saved task".to_vec());
            pane.handle_pty_bytes(b"saved scrollback\r\n".to_vec());
            let expected = pane.held_for_warp().unwrap();
            let (to_pty, pty) = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
            let payload = if kind == "pane" {
                let (tx, rx) = yinz_bus::warp::pane_warp_reply();
                screen
                    .warp_pane(
                        true,
                        true,
                        Some(pane_id),
                        "elsewhere".into(),
                        yinz_types::data::WarpPlacement::FocusedTab,
                        ClientId::new(1),
                        tx,
                        warp_deadline(),
                    )
                    .unwrap();
                let held = pty
                    .try_iter()
                    .find_map(|(i, _)| match i {
                        PtyInstruction::ReleaseTerminalForWarp { held, .. } => held,
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    screen
                        .tabs
                        .get_mut(&TAB_0)
                        .unwrap()
                        .get_pane_with_id_mut(pane_id)
                        .unwrap()
                        .rerun()
                        .is_none(),
                    "restart waits until the transfer finishes"
                );
                screen.pane_released_for_warp(TerminalId::new(1), Ok(ReleasedPane::Held(held)));
                *rx.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
                    .unwrap()
                    .0
            } else {
                let (tx, rx) = crossbeam::channel::bounded(1);
                if kind == "tab" {
                    screen
                        .warp_tab(
                            true,
                            true,
                            Some(TAB_0),
                            ClientId::new(1),
                            tx,
                            warp_deadline(),
                        )
                        .unwrap();
                    let (tab_index, terminal_ids, held) = pty
                        .try_iter()
                        .find_map(|(i, _)| match i {
                            PtyInstruction::ReleaseTerminalsForWarp {
                                tab_index,
                                terminal_ids,
                                held,
                                ..
                            } => Some((tab_index, terminal_ids, held)),
                            _ => None,
                        })
                        .unwrap();
                    screen.tab_released_for_warp(
                        tab_index,
                        terminal_ids
                            .into_iter()
                            .map(|id| (id, Ok(ReleasedPane::Held(held[&id].clone()))))
                            .collect(),
                    );
                    rx.recv().unwrap().unwrap().tiled_panes.remove(0)
                } else {
                    let (tx, rx) = yinz_bus::warp::pane_warp_reply();
                    screen
                        .warp_pane(
                            true,
                            true,
                            Some(PaneId::Terminal(TerminalId::new(9))),
                            "elsewhere".into(),
                            yinz_types::data::WarpPlacement::FocusedTab,
                            ClientId::new(1),
                            tx,
                            warp_deadline(),
                        )
                        .unwrap();
                    let (tab_index, terminal_ids, held) = pty
                        .try_iter()
                        .find_map(|(i, _)| match i {
                            PtyInstruction::ReleaseTerminalsForWarp {
                                tab_index,
                                terminal_ids,
                                held,
                                ..
                            } => Some((tab_index, terminal_ids, held)),
                            _ => None,
                        })
                        .unwrap();
                    screen.tab_released_for_warp(
                        tab_index,
                        terminal_ids
                            .into_iter()
                            .map(|id| {
                                (
                                    id,
                                    Ok(match held.get(&id) {
                                        Some(held) => ReleasedPane::Held(held.clone()),
                                        None => ReleasedPane::Local(ReleasedPtyHost {
                                            host_id: "live".into(),
                                            host_pid: ProcessId::new(1),
                                            control_socket: Some("/live".into()),
                                        }),
                                    }),
                                )
                            })
                            .collect(),
                    );
                    let group = rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap()
                        .unwrap()
                        .0
                        .group
                        .unwrap();
                    group
                        .tiled_panes
                        .into_iter()
                        .chain(group.floating_panes)
                        .find(|p| p.held.is_some())
                        .unwrap()
                }
            };
            assert_eq!(payload.held, Some(expected));
            assert!(payload.contents.unwrap().contains("saved scrollback"));
            assert!(screen
                .tabs
                .values()
                .all(|tab| tab.get_pane_with_id(pane_id).is_none()));
        }
    }
}

#[test]
fn warp_held_replacements_resume_only_when_the_pane_stays_in_the_source() {
    use yinz_types::data::{OriginatingPlugin, AGENT_PANE_CONTEXT};
    use yinz_types::input::hooks::HookPoint;

    for kind in ["pane", "tab", "companion"] {
        for outcome in ["refused", "timeout", "success"] {
            for first_run in [true, false] {
                for keep_scrollback in [true, false] {
                    let mut screen = if kind == "pane" {
                        let mut screen =
                            create_new_screen(Size { cols: 80, rows: 24 }, false, false);
                        new_tab(&mut screen, 1, TAB_0);
                        new_tab(&mut screen, 2, TabId::new(1));
                        screen
                    } else {
                        screen_with_a_scrollback_editor_open()
                    };
                    let terminal_id = TerminalId::new(1);
                    let pane_id = PaneId::Terminal(terminal_id);
                    let live_id = TerminalId::new(if kind == "pane" { 2 } else { 9 });
                    let pane = screen
                        .tabs
                        .get_mut(&TAB_0)
                        .unwrap()
                        .get_pane_with_id_mut(pane_id)
                        .unwrap();
                    pane.hold((!first_run).then_some(17), first_run, RunCommand::default());
                    if !first_run {
                        pane.handle_pty_bytes(b"saved scrollback\r\n".to_vec());
                    }
                    let replacement = RunCommand {
                        command: "codex".into(),
                        args: vec!["replacement".into()],
                        originating_plugin: Some(OriginatingPlugin::new(
                            yinz_types::internal_ids::PluginId::new(2),
                            ClientId::new(1),
                            std::collections::BTreeMap::from([(
                                AGENT_PANE_CONTEXT.into(),
                                "true".into(),
                            )]),
                        )),
                        ..Default::default()
                    };
                    let (to_pty, pty) = channels::unbounded();
                    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty.clone()));
                    for tab in screen.tabs.values_mut() {
                        tab.senders.to_pty = Some(SenderWithContext::new(to_pty.clone()));
                    }
                    let (to_plugin, plugin) = channels::unbounded();
                    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
                    let (pane_tx, pane_rx) = yinz_bus::warp::pane_warp_reply();
                    let (tab_tx, tab_rx) = crossbeam::channel::bounded(1);
                    let deadline = if outcome == "timeout" {
                        std::time::Instant::now()
                    } else {
                        warp_deadline()
                    };
                    if kind == "tab" {
                        screen
                            .warp_tab(true, true, Some(TAB_0), ClientId::new(1), tab_tx, deadline)
                            .unwrap();
                    } else {
                        screen
                            .warp_pane(
                                true,
                                true,
                                Some(if kind == "companion" {
                                    PaneId::Terminal(live_id)
                                } else {
                                    pane_id
                                }),
                                "elsewhere".into(),
                                yinz_types::data::WarpPlacement::FocusedTab,
                                ClientId::new(1),
                                pane_tx,
                                deadline,
                            )
                            .unwrap();
                    }
                    let released = pty
                        .try_iter()
                        .find_map(|(instruction, _)| match instruction {
                            PtyInstruction::ReleaseTerminalForWarp { held, .. } => {
                                Some(vec![(terminal_id, Ok(ReleasedPane::Held(held.unwrap())))])
                            },
                            PtyInstruction::ReleaseTerminalsForWarp {
                                terminal_ids, held, ..
                            } => Some(
                                terminal_ids
                                    .into_iter()
                                    .map(|id| {
                                        (
                                            id,
                                            Ok(match held.get(&id) {
                                                Some(held) => ReleasedPane::Held(held.clone()),
                                                None => ReleasedPane::Local(ReleasedPtyHost {
                                                    host_id: "live".into(),
                                                    host_pid: ProcessId::new(1),
                                                    control_socket: Some("/live".into()),
                                                }),
                                            }),
                                        )
                                    })
                                    .collect(),
                            ),
                            _ => None,
                        })
                        .unwrap();
                    screen.run_command_in_pane_with_id(
                        terminal_id,
                        replacement.clone(),
                        keep_scrollback,
                    );
                    screen.run_command_in_pane_with_id(
                        live_id,
                        replacement.clone(),
                        keep_scrollback,
                    );
                    assert!(pty.try_recv().is_err(), "replacement waits for release");
                    let released = released
                        .into_iter()
                        .map(|(id, result)| {
                            (
                                id,
                                if outcome == "refused" && id == terminal_id {
                                    Err("injected refusal".into())
                                } else {
                                    result
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    if kind == "pane" {
                        screen.pane_released_for_warp(
                            terminal_id,
                            released.into_iter().next().unwrap().1,
                        );
                    } else {
                        screen.tab_released_for_warp(TAB_0, released);
                    }
                    let response = if kind == "tab" {
                        tab_rx.try_recv().map(|result| result.map(|_| ()))
                    } else {
                        pane_rx.try_recv().map(|result| result.map(|_| ()))
                    };
                    if outcome == "timeout" {
                        assert!(response.is_err());
                    } else {
                        assert_eq!(response.unwrap().is_ok(), outcome == "success");
                    }
                    let reruns: Vec<_> = pty
                        .try_iter()
                        .filter_map(|(instruction, _)| match instruction {
                            PtyInstruction::ReRunCommandInPane(id, command, _) => {
                                Some((id, command))
                            },
                            _ => None,
                        })
                        .collect();
                    let hooks: Vec<_> = plugin
                        .try_iter()
                        .filter_map(|(instruction, _)| match instruction {
                            PluginInstruction::RunHook { point, .. } => Some(point),
                            _ => None,
                        })
                        .collect();
                    if outcome == "success" {
                        assert!(reruns.is_empty(), "no source replacement after transfer");
                        assert!(hooks.is_empty());
                        assert!(screen
                            .tabs
                            .values()
                            .all(|tab| tab.get_pane_with_id(pane_id).is_none()));
                    } else {
                        assert_eq!(reruns, vec![(pane_id, replacement.clone())]);
                        assert_eq!(hooks, vec![HookPoint::AgentStart]);
                        let tab = screen.tabs.get_mut(&TAB_0).unwrap();
                        let pane = tab.get_pane_with_id(pane_id).unwrap();
                        assert!(!pane.is_held());
                        assert_eq!(
                            pane.dump_screen(true, None).contains("saved scrollback"),
                            keep_scrollback && !first_run,
                            "{kind}, {outcome}, first_run={first_run}"
                        );
                        assert!(tab.take_pending_respawn(pane_id).is_none());
                        let live_pane = PaneId::Terminal(live_id);
                        let tab = screen
                            .tabs
                            .values_mut()
                            .find(|tab| tab.get_pane_with_id(live_pane).is_some())
                            .unwrap();
                        assert_eq!(
                            tab.take_pending_respawn(live_pane),
                            Some((replacement, keep_scrollback))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn warp_old_destination_and_failed_release_leave_held_panes_restartable() {
    for supported in [false, true] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
        new_tab(&mut screen, 8, TAB_0);
        new_tab(&mut screen, 2, TabId::new(1));
        let pane_id = PaneId::Terminal(TerminalId::new(8));
        screen
            .tabs
            .get_mut(&TAB_0)
            .unwrap()
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .hold(None, true, RunCommand::default());
        let (to_pty, pty) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        let (tx, rx) = crossbeam::channel::bounded(1);
        screen
            .warp_tab(
                true,
                supported,
                Some(TAB_0),
                ClientId::new(1),
                tx,
                warp_deadline(),
            )
            .unwrap();
        if supported {
            screen.tab_released_for_warp(
                TAB_0,
                vec![(TerminalId::new(8), Err("injected failure".into()))],
            );
        } else {
            assert!(pty
                .try_iter()
                .all(|(i, _)| !matches!(i, PtyInstruction::ReleaseTerminalsForWarp { .. })));
        }
        let error = rx.recv().unwrap().unwrap_err();
        assert!(supported || error.contains("terminal_8"), "{error}");
        assert!(screen
            .tabs
            .get_mut(&TAB_0)
            .unwrap()
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .rerun()
            .is_some());
    }
}

#[test]
fn warp_mixed_tab_keeps_terminal_8_distinct_from_status_bar_plugin_8() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        false,
        false,
    );
    let (to_plugin, _plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    new_tab(&mut screen, 99, TabId::new(1));
    let plugin = RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:status-bar").unwrap(),
    );
    let command = RunCommand {
        command: "/usr/bin/true".into(),
        hold_on_start: true,
        ..Default::default()
    };
    screen
        .new_tab(TAB_0, (vec![], vec![]), None, Some(ClientId::new(1)))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout::default(),
                    TiledPaneLayout {
                        run: Some(Run::Command(command.clone())),
                        ..Default::default()
                    },
                    TiledPaneLayout {
                        run: Some(Run::Plugin(plugin.clone())),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            vec![],
            vec![
                (TerminalId::new(1), None),
                (TerminalId::new(8), Some(command.clone())),
            ],
            vec![],
            HashMap::from([(plugin.clone(), vec![crate::PluginId::new(8)])]),
            TAB_0,
            true,
            ClientId::new(1),
            None,
        )
        .unwrap();
    let (to_pty, pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
    let (tx, rx) = crossbeam::channel::bounded(1);
    screen
        .warp_tab(
            true,
            true,
            Some(TAB_0),
            ClientId::new(1),
            tx,
            warp_deadline(),
        )
        .unwrap();
    let (ids, held) = pty
        .try_iter()
        .find_map(|(i, _)| match i {
            PtyInstruction::ReleaseTerminalsForWarp {
                terminal_ids, held, ..
            } => Some((terminal_ids, held)),
            _ => None,
        })
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(held.len(), 1);
    assert_eq!(held[&TerminalId::new(8)].command, command);
    screen.tab_released_for_warp(
        TAB_0,
        ids.into_iter()
            .map(|id| {
                (
                    id,
                    Ok(match held.get(&id) {
                        Some(held) => ReleasedPane::Held(held.clone()),
                        None => ReleasedPane::Local(ReleasedPtyHost {
                            host_id: "live".into(),
                            host_pid: ProcessId::new(1),
                            control_socket: Some("/live".into()),
                        }),
                    }),
                )
            })
            .collect(),
    );
    let payload = rx.recv().unwrap().unwrap();
    assert_eq!(payload.tiled_panes.len(), 2);
    assert_eq!(
        payload
            .tiled_panes
            .iter()
            .filter(|p| p.held.is_some())
            .count(),
        1
    );
    assert!(
        payload
            .tiled_layout
            .extract_run_instructions()
            .contains(&Some(Run::Plugin(plugin))),
        "{:?}",
        payload.tiled_layout
    );
    let closed: Vec<_> = pty
        .try_iter()
        .filter_map(|(i, _)| match i {
            PtyInstruction::ClosePane(id, _) => Some(id),
            _ => None,
        })
        .collect();
    assert!(closed.contains(&PaneId::Terminal(TerminalId::new(8))));
    assert!(closed.contains(&PaneId::Plugin(crate::PluginId::new(8))));
    assert!(!closed.contains(&PaneId::Terminal(TerminalId::new(1))));
}

#[test]
fn pane_and_tab_warp_capture_actual_paste_mode() {
    for enabled in [false, true] {
        for whole_tab in [false, true] {
            let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
            new_tab(&mut screen, 1, TabId::new(0));
            new_tab(&mut screen, 2, TabId::new(1));
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .handle_pty_bytes(
                    TerminalId::new(1),
                    if enabled {
                        b"\x1b[?2004h".to_vec()
                    } else {
                        b"\x1b[?2004l".to_vec()
                    },
                )
                .unwrap();
            assert!(screen
                .take_released_pane_for_warp(TerminalId::new(1), Err("adoption refused".into()))
                .is_err());
            assert_eq!(
                screen.tabs[&TabId::new(0)]
                    .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
                    .unwrap()
                    .supports_bracketed_paste(),
                enabled
            );
            let payload = if whole_tab {
                let (tab, terminals, prepared) = screen
                    .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
                    .unwrap();
                let released = terminals
                    .into_iter()
                    .map(|id| {
                        (
                            id,
                            Ok(ReleasedPane::Local(ReleasedPtyHost {
                                host_id: SupervisorId::new(format!("host-{id}")),
                                host_pid: ProcessId::new(1),
                                control_socket: Some(PathBuf::from(format!("/tmp/warp-{id}.sock"))),
                            })),
                        )
                    })
                    .collect();
                screen
                    .take_released_tab_for_warp(tab, released, prepared)
                    .unwrap()
                    .tiled_panes
                    .remove(0)
            } else {
                *released_remote_pane(&mut screen, TerminalId::new(1))
            };
            assert_eq!(payload.bracketed_paste, enabled);
        }
    }
}

#[test]
fn a_pane_warp_keeps_the_last_tab_until_its_hold_drops() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let pty = mock.pty_receiver.take().unwrap();
    let server = mock.server_receiver.take().unwrap();
    let client_id = mock.main_client_id;
    let thread = mock.run(None, vec![]);
    // An unmatched finish is logged, not allowed to wrap the count.
    mock.to_screen
        .send(ScreenInstruction::PaneWarpFinished)
        .unwrap();
    let (reply, released) = yinz_bus::warp::pane_warp_reply();
    mock.to_screen
        .send(ScreenInstruction::WarpPane {
            supports_companions: true,
            supports_held: true,
            pane_id: Some(PaneId::Terminal(TerminalId::new(0))),
            to_session: "destination".into(),
            placement: yinz_types::data::WarpPlacement::FocusedTab,
            client_id,
            response_channel: reply,
            deadline: warp_deadline(),
        })
        .unwrap();
    loop {
        if matches!(
            pty.recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
                .0,
            PtyInstruction::ReleaseTerminalForWarp { .. }
        ) {
            break;
        }
    }
    let (reply, refused) = yinz_bus::warp::pane_warp_reply();
    mock.to_screen
        .send(ScreenInstruction::WarpPane {
            supports_companions: true,
            supports_held: true,
            pane_id: Some(PaneId::Terminal(TerminalId::new(999))),
            to_session: "destination".into(),
            placement: yinz_types::data::WarpPlacement::FocusedTab,
            client_id,
            response_channel: reply,
            deadline: warp_deadline(),
        })
        .unwrap();
    assert!(refused
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .is_err());
    mock.to_screen
        .send(ScreenInstruction::PaneReleasedForWarp {
            terminal_id: TerminalId::new(0),
            to_session: "destination".into(),
            placement: yinz_types::data::WarpPlacement::FocusedTab,
            client_id,
            released: Ok(ReleasedPane::Local(ReleasedPtyHost {
                host_id: SupervisorId::new("host-1"),
                host_pid: ProcessId::new(1),
                control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
            })),
        })
        .unwrap();
    let (_payload, hold) = released
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .unwrap();
    let mut hold = Some(hold);
    for holding in [true, false] {
        if !holding {
            // The hold's drop sends `PaneWarpFinished`; nothing else does.
            hold.take();
        }
        let (reply, tab) = crossbeam::channel::unbounded();
        mock.to_screen
            .send(ScreenInstruction::GetTabInfo {
                tab_id: TabId::new(0),
                response_channel: reply,
            })
            .unwrap();
        assert_eq!(
            tab.recv_timeout(yinz_test_support::default_timeout())
                .unwrap()
                .is_some(),
            holding
        );
        assert_eq!(
            server
                .try_iter()
                .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))),
            !holding
        );
    }
    mock.teardown(vec![thread]);
}

#[test]
fn an_adopted_held_pane_restores_its_state_and_scrollback() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let client_id = mock.main_client_id;
    let thread = mock.run(None, vec![]);
    let cmd = RunCommand {
        command: "/usr/bin/true".into(),
        hold_on_close: true,
        ..Default::default()
    };
    let pane_id = PaneId::Terminal(TERMINAL_50);
    mock.to_screen
        .send(ScreenInstruction::AdoptedPaneReady {
            hold: None,
            placed: crossbeam::channel::unbounded().0,
            terminal_id: TERMINAL_50,
            payload: Box::new(WarpedPane {
                agent_theme: None,
                held: Some(yinz_types::data::HeldTerminal {
                    command: cmd.clone(),
                    first_run: false,
                    exit_status: Some(17),
                }),
                contents: Some("RESTORED BEFORE ACK\r\n".into()),
                ..Default::default()
            }),
            placement: yinz_types::data::WarpPlacement::FocusedTab,
            client_id,
            cmd,
            buffered: vec![],
            child_exit_status: Some(Some(17)),
        })
        .unwrap();
    // Placement is queued behind `AdoptedPaneReady`, so ask until it has run.
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    let info = loop {
        let (reply, info) = crossbeam::channel::unbounded();
        mock.to_screen
            .send(ScreenInstruction::GetPaneInfo {
                pane_id,
                response_channel: reply,
            })
            .unwrap();
        let info = info
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        match info {
            Some(info) if info.is_held => break info,
            _ => assert!(
                std::time::Instant::now() < deadline,
                "the adopted pane was never restored as held"
            ),
        }
    };
    assert_eq!(info.exit_status, Some(17));
    let (reply, scrollback) = crossbeam::channel::unbounded();
    mock.to_screen
        .send(ScreenInstruction::GetPaneScrollback {
            pane_id,
            client_id,
            get_full_scrollback: true,
            response_channel: reply,
        })
        .unwrap();
    let scrollback = scrollback
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    let yinz_types::data::PaneScrollbackResponse::Ok(scrollback) = scrollback else {
        panic!("missing restored scrollback")
    };
    assert!(scrollback
        .viewport
        .join("\n")
        .contains("RESTORED BEFORE ACK"));
    mock.teardown(vec![thread]);
}

#[test]
fn agent_theme_history_travels_through_pane_and_tab_warps() {
    use yinz_terminal::panes::terminal_character::{AnsiCode, NamedColor};
    use yinz_types::agent::theme::*;
    use yinz_types::colors::PaletteColor;
    fn history(role: Role, id: &str, text: &str) -> Vec<u8> {
        let snapshot = AgentThemeSnapshot {
            version: SNAPSHOT_VERSION,
            provider: "fixture".into(),
            provider_version: "1".into(),
            theme: "fixture".into(),
            provenance: vec![],
            availability: Availability::Available,
            definitions: vec![Definition {
                token: "status".into(),
                channel: Channel::Foreground,
                color: SourceColor::Index(2),
                pair: None,
                target: Some(role.into()),
            }],
            collisions: vec![],
        };
        yinz_terminal::agent_theme::handoff_replay(
            text.into(),
            Some(AgentThemeHandoff {
                snapshots: vec![snapshot],
                associations: vec![PaletteAssociation {
                    offset: 0,
                    palette: Some(0),
                }],
                active: Some(HandoffLaunch {
                    launch_id: id.into(),
                    palette: 0,
                }),
                ..Default::default()
            }),
        )
    }
    let mut theme = yinz_types::style::Style::default().colors;
    theme.exit_code_success.base = PaletteColor::Rgb((11, 22, 33));
    theme.exit_code_error.base = PaletteColor::Rgb((99, 88, 77));
    let check = |pane: &mut dyn Pane, mapped: bool| {
        pane.render_full_viewport();
        pane.set_should_render(true);
        let chunks = pane.render(None).unwrap().unwrap().0;
        for (character, color) in [
            ('S', None),
            ('A', Some((11, 22, 33))),
            ('s', None),
            ('B', Some((99, 88, 77))),
        ] {
            let expected = if mapped {
                color.map(AnsiCode::RgbCode)
            } else {
                None
            }
            .unwrap_or(AnsiCode::NamedColor(NamedColor::Green));
            assert!(
                chunks.iter().any(|chunk| chunk
                    .terminal_characters
                    .iter()
                    .any(|cell| cell.character == character
                        && chunk.application_styles(*cell.styles).foreground == Some(expected))),
                "missing {character}"
            );
        }
    };
    for whole_tab in [false, true] {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        screen.scrollback_lines_to_serialize = Some(0);
        let pane = screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(1)))
            .unwrap();
        pane.update_theme(theme);
        pane.handle_pty_bytes(b"\x1b[32mS".to_vec());
        pane.handle_pty_bytes(history(Role::Success, "first", "\x1b[32mAAA"));
        pane.handle_pty_bytes(yinz_terminal::agent_theme::handoff_replay(
            "\x1b[32ms".into(),
            Some(AgentThemeHandoff::default()),
        ));
        pane.handle_pty_bytes(history(Role::Error, "second", "\x1b[32mBBB"));
        check(pane.as_mut(), true);
        let release = || {
            Ok(ReleasedPane::Local(ReleasedPtyHost {
                host_id: "fixture".into(),
                host_pid: ProcessId::new(1),
                control_socket: Some("/tmp/fixture.sock".into()),
            }))
        };
        let payload = if whole_tab {
            let (tab, ids, prepared) = screen
                .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
                .unwrap();
            screen
                .take_released_tab_for_warp(
                    tab,
                    ids.into_iter().map(|id| (id, release())).collect(),
                    prepared,
                )
                .unwrap()
                .tiled_panes
                .remove(0)
        } else {
            *screen
                .take_released_pane_for_warp(TerminalId::new(1), release())
                .unwrap()
        };
        assert_eq!(payload.agent_theme.as_ref().unwrap().snapshots.len(), 2);
        assert_eq!(
            payload
                .agent_theme
                .as_ref()
                .unwrap()
                .active
                .as_ref()
                .unwrap()
                .launch_id,
            "second"
        );
        let pane = screen
            .tabs
            .get_mut(&TabId::new(1))
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(2)))
            .unwrap();
        pane.update_theme(theme);
        pane.handle_pty_bytes(yinz_terminal::agent_theme::handoff_replay(
            payload.contents.unwrap(),
            payload.agent_theme,
        ));
        check(pane.as_mut(), true);
        pane.update_agent_theme_mapping(AgentThemeMapping::Off);
        check(pane.as_mut(), false);
        pane.update_agent_theme_mapping(AgentThemeMapping::Semantic);
        check(pane.as_mut(), true);
        pane.handle_pty_bytes(b"\r\n\x1b[32mNEW".to_vec());
        let chunks = pane.render(None).unwrap().unwrap().0;
        assert!(chunks
            .iter()
            .any(|chunk| chunk
                .terminal_characters
                .iter()
                .any(|cell| cell.character == 'N'
                    && chunk.application_styles(*cell.styles).foreground
                        == Some(AnsiCode::RgbCode((99, 88, 77))))));
    }
}
