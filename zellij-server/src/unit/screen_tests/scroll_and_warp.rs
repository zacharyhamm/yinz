#[cfg(test)]
const TERMINAL_50: zellij_types::internal_ids::TerminalId =
    zellij_types::internal_ids::TerminalId::new(50);

#[cfg(test)]
const TAB_0: TabId = TabId::new(0);

use super::*;
use zellij_test_support::Watched;
use zellij_types::internal_ids::ProcessId;
use zellij_types::internal_ids::SupervisorId;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TabPosition;

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
                    panes.into_iter().map(Into::into).collect(),
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
            let expected: Vec<zellij_types::data::PaneId> = if failed {
                vec![]
            } else {
                departing.iter().copied().map(Into::into).collect()
            };
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
                    remaining.into_iter().map(Into::into).collect()
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
        zellij_types::data::WarpPlacement::FocusedTab,
        zellij_types::data::WarpPlacement::Floating,
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
            vec![zellij_types::data::PaneId::Terminal(TERMINAL_50)],
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
            .contains(&zellij_types::data::PaneId::Terminal(TERMINAL_50)));
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
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    let (to_plugin, _plugin_receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let plugin = PaneId::Plugin(crate::PluginId::new(50));
    let run = Run::Plugin(RunPluginOrAlias::RunPlugin(
        zellij_runtime::plugin_location::run_plugin_from_url("zellij:strider")
            .expect("plugin configuration"),
    ));
    screen
        .replace_pane(
            plugin,
            None,
            Some(run.clone()),
            None,
            false,
            ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(TerminalId::new(1))),
        )
        .expect("plugin cover");
    let (_, terminals, prepared) = screen
        .prepare_pane_group(Some(plugin), ClientId::new(1))
        .expect("visible plugin")
        .expect("group");
    assert_eq!(terminals, vec![TerminalId::new(1)]);
    assert_eq!(prepared.tiled_layout.run, Some(run.clone()));
    let cover = PaneId::Terminal(TerminalId::new(60));
    screen
        .replace_pane(
            cover,
            None,
            None,
            None,
            false,
            ClientTabIndexOrPaneId::PaneId(plugin),
        )
        .expect("terminal cover");
    assert!(screen
        .prepare_pane_group(Some(plugin), ClientId::new(1))
        .is_err());
    let (_, terminals, prepared) = screen
        .prepare_pane_group(Some(cover), ClientId::new(1))
        .expect("visible cover")
        .expect("group");
    assert_eq!(terminals.len(), 2);
    assert!(prepared
        .floating_layout
        .iter()
        .any(|pane| pane.run == Some(run.clone())));
    assert_eq!(
        prepared.relationships.expect("relationships").hidden.len(),
        2
    );
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
            zellij_types::data::WarpPlacement::FocusedTab,
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
            zellij_types::data::WarpPlacement::FocusedTab,
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
                    terminal_id: crate::TerminalId::new(50),
                    payload: Box::default(),
                    placement: zellij_types::data::WarpPlacement::FocusedTab,
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
                zellij_test_support::default_timeout(),
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
                zellij_test_support::default_timeout(),
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
            WarpedPane::default(),
            zellij_types::data::WarpPlacement::FocusedTab,
            crate::ClientId::new(7),
            RunCommand::default(),
            vec![],
            None,
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
        )]
    ));
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
        zellij_types::data::WarpPlacement::FocusedTab,
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
        "pane 1 has no pty-host supervisor",
        "the reason has to reach the caller unreworded",
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

/// A pane spawned to replace one that closed meanwhile (a VM boot later, say)
/// has nowhere to go. Screen never owned it, so it tells the pty thread to
/// close it rather than leaving its process, transport and reader running
/// until the session ends.
#[test]
fn a_replacement_for_a_pane_that_is_gone_is_closed_on_the_pty_thread() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));

    screen
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(50)),
            None,
            None,
            None,
            false,
            ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(crate::TerminalId::new(999))),
        )
        .expect("TEST");

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
fn a_pane_and_the_editor_open_on_it_cannot_be_the_last_two_panes_warped_group_refused() {
    let mut screen = screen_with_a_scrollback_editor_open();
    screen.tabs.retain(|id, _| *id == TabId::new(0));
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(9))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
}

#[test]
fn the_sessions_only_pane_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));

    assert_eq!(
        screen.check_pane_can_be_warped(
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            crate::ClientId::new(1)
        ),
        Err("this is the session's only pane; moving it would close the session".to_owned())
    );
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
    terminal_id: zellij_types::internal_ids::TerminalId,
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
                    target: zellij_types::command::PaneTarget::Ssh {
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

    let payload = released_remote_pane(&mut screen, zellij_types::internal_ids::TerminalId::new(1));

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

    released_remote_pane(&mut screen, zellij_types::internal_ids::TerminalId::new(1));

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
    let (response_tx, response_rx) = crossbeam::channel::unbounded();

    mock_screen
        .to_screen
        .send(ScreenInstruction::WarpPane {
            supports_companions: true,
            pane_id: Some(PaneId::Terminal(crate::TerminalId::new(1))),
            to_session: "elsewhere".to_owned().into(),
            placement: zellij_types::data::WarpPlacement::NewTab,
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

#[test]
fn a_tab_with_a_scrollback_editor_open_can_be_warped() {
    // Refused for the same reason a single pane with one open is: the
    // suppressed-pane bookkeeping is keyed by the visible pane, so closing the
    // pair hands over the wrong pty.
    let mut screen = screen_with_a_scrollback_editor_open();

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    let relationships = prepared.relationships.expect("hidden relationships");
    assert!(!relationships.hidden.is_empty());
    assert_eq!(
        terminals.len(),
        prepared.tiled_panes.len() + prepared.floating_panes.len()
    );
}

#[test]
fn a_tab_holding_a_pane_a_plugin_hid_can_be_warped() {
    // Parked with nothing standing in for it, like a stack-list member - and
    // unlike one, with no slot in the layout the tab travels as. A single such
    // pane can be warped on its own (`a_pane_a_plugin_hid_can_be_warped`), because
    // then it is the thing being moved; taking the tab out from under it would
    // release a supervisor nobody at the far end is holding.
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
    let relationships = prepared.relationships.expect("hidden relationships");
    assert!(!relationships.hidden.is_empty());
    assert_eq!(
        terminals.len(),
        prepared.tiled_panes.len() + prepared.floating_panes.len()
    );
}

#[test]
fn a_tab_holding_a_hidden_floating_pane_can_be_warped() {
    // The same refusal, asked of the one kind of pane that can never be the
    // exception to it: a stack is a shape the *tiled* layout takes, so a hidden
    // floating pane is a hidden pane and nothing else.
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
    let relationships = prepared.relationships.expect("hidden relationships");
    assert!(!relationships.hidden.is_empty());
    assert_eq!(
        terminals.len(),
        prepared.tiled_panes.len() + prepared.floating_panes.len()
    );
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
fn warping_a_stack_list_counts_all_members_for_last_pane_refusal() {
    let mut screen = screen_with_a_stack_list();
    screen.tabs.retain(|id, _| *id == TabId::new(0));
    let before = screen.tabs[&TabId::new(0)].get_all_pane_ids();
    assert!(screen
        .prepare_pane_group(Some(PaneId::Terminal(TerminalId::new(3))), ClientId::new(1))
        .is_err());
    assert_eq!(screen.tabs[&TabId::new(0)].get_all_pane_ids(), before);
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
    // The case the refusal is really for, in the middle of the thing the refusal
    // makes an exception for. A stack-list member with a scrollback editor open on
    // it is a pair: the editor is the member and travels, but the pane it is
    // editing is parked behind it with no slot of its own. Letting the tab go
    // would dismantle it around a pty nobody released.
    let mut screen = screen_with_an_editor_on_a_hidden_stack_list_member();

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    let relationships = prepared.relationships.expect("hidden relationships");
    assert!(!relationships.hidden.is_empty());
    assert_eq!(
        terminals.len(),
        prepared.tiled_panes.len() + prepared.floating_panes.len()
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
    // The pair this refusal is really about, and the one an `is_scrollback_editor`
    // check would let through: `suppress_pane_and_replace_with_pid` sets that flag
    // to `false` while parking the hidden pane behind the visible one - under its
    // own id, with the stand-in named in `restored_by` - so closing the tab swaps
    // it back in rather than removing it.
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
            false,
            None,
            None,
            None,
        )
        .expect("TEST");

    let (_, terminals, prepared) = screen
        .check_tab_can_be_warped(Some(TabId::new(0)), ClientId::new(1))
        .expect("hidden contents travel");
    let relationships = prepared.relationships.expect("hidden relationships");
    assert!(!relationships.hidden.is_empty());
    assert_eq!(
        terminals.len(),
        prepared.tiled_panes.len() + prepared.floating_panes.len()
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
            total < zellij_ipc::ipc::MAX_IPC_FRAME_LEN,
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
        released_remote_pane_for_warp(&mut screen, zellij_types::internal_ids::TerminalId::new(1));

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
        released_remote_pane_for_warp(&mut screen, zellij_types::internal_ids::TerminalId::new(1));

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
    let panes = source.extract_warp_panes(&slots);
    assert!(slots
        .iter()
        .all(|id| source.get_pane_with_id(*id).is_none()));
    let destination = screen.tabs.get_mut(&TabId::new(1)).expect("destination");
    for pane in panes {
        let id = pane.pid();
        if id == selected {
            destination
                .add_tiled_pane(pane, id, true, Some(ClientId::new(1)))
                .expect("visible pane");
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
        .expect("restore");
    let restored = destination.arrangement_stacks();
    assert_eq!(restored[0].members, original[0].members);
    assert_eq!(restored[0].visible, selected);
    let editor = PaneId::Terminal(TerminalId::new(8));
    assert!(destination.focus_hidden_stack_list_member(editor, ClientId::new(1)));
    destination.close_pane(editor, false, None);
    assert!(destination.has_non_suppressed_pane_with_pid(&PaneId::Terminal(TerminalId::new(3))));
}

fn queue_warp_group(
    screen: &mut Screen,
    placement: zellij_types::data::WarpPlacement,
) -> (TabId, crossbeam::channel::Receiver<anyhow::Result<()>>) {
    let (tx, rx) = crossbeam::channel::bounded(1);
    let tab_id = screen.get_new_tab_id();
    screen
        .place_adopted_tab(
            zellij_types::data::WarpedTab {
                relationships: Some(Default::default()),
                ..Default::default()
            },
            crate::pty::AdoptedTabPanes {
                tiled: vec![],
                floating: vec![],
            },
            ClientId::new(1),
            Some(placement),
            Some(tx),
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
    use zellij_types::data::WarpPlacement;
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
    let (tab_id, rx) = queue_warp_group(&mut screen, zellij_types::data::WarpPlacement::Floating);
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
        zellij_types::data::WarpPlacement::NewTab,
        zellij_types::data::WarpPlacement::FocusedTab,
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
        let manifest: zellij_types::session_manifest::PaneLayoutManifest = metadata.into();
        assert_eq!(manifest.geom, full_rect);
        assert_eq!(manifest.geom.stacked, None);
    }
    let (dump, _) = zellij_session::session_serialization::serialize_session_layout(
        screen.get_layout_metadata(None, Some(tab_id)).into(),
    )
    .unwrap();
    let layout = zellij_config::kdl::layout::from_kdl(&dump, None, None, None).unwrap();
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.tabs[0].2.len(), 3);
    assert_eq!(layout.tabs[0].2[1].name.as_deref(), Some("editor subject"));
    for pane in &layout.tabs[0].2 {
        use zellij_types::geometry::PercentOrFixed;
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
