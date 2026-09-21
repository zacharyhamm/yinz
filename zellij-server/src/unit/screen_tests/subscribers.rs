#[cfg(test)]
const TERMINAL_1: zellij_types::internal_ids::TerminalId =
    zellij_types::internal_ids::TerminalId::new(1);

use super::*;
use zellij_test_support::Watched;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;

#[test]
fn subscriber_receives_initial_delivery() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            ..
        } => {
            assert!(*is_initial);
            assert!(scrollback.is_none());
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_receives_initial_with_scrollback() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        Some(0),
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            ..
        } => {
            assert!(*is_initial);
            assert!(scrollback.is_some());
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_no_update_on_unchanged_viewport() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let initial_viewport = {
        let msgs = messages.lock().unwrap();
        let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
        match &client_msgs[0] {
            ServerToClientMsg::PaneRenderUpdate { viewport, .. } => viewport.clone(),
            _ => panic!("Expected PaneRenderUpdate"),
        }
    };

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
        PaneContents {
            viewport: initial_viewport,
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    assert_eq!(client_msgs.len(), 1);
}

#[test]
fn subscriber_receives_update_on_changed_viewport() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
        PaneContents {
            viewport: vec!["changed line".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    assert_eq!(client_msgs.len(), 2);
    match &client_msgs[1] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            viewport,
            ..
        } => {
            assert!(!is_initial);
            assert!(scrollback.is_none());
            assert_eq!(viewport, &vec!["changed line".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_error_for_nonexistent_pane() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(999),
        )],
        None,
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::LogError { lines } => {
            let joined = lines.join(" ");
            assert!(
                joined.contains("not found"),
                "Error message should contain 'not found', got: {}",
                joined
            );
        },
        other => panic!("Expected LogError, got {:?}", other),
    }
    assert!(
        !screen
            .pane_render_subscribers
            .contains_key(&ClientId::new(100)),
        "Subscriber should not be registered for nonexistent pane"
    );
}

#[test]
fn subscriber_state_registered_for_multiple_panes() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    let sub = screen
        .pane_render_subscribers
        .get(&ClientId::new(100))
        .unwrap();
    assert_eq!(sub.pane_ids.len(), 2);
    assert!(sub.pane_ids.contains(&zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(1)
    )));
    assert!(sub.pane_ids.contains(&zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(2)
    )));
}

#[test]
fn multiple_subscribers_receive_updates() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        crate::ClientId::new(101),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
        PaneContents {
            viewport: vec!["new content".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_100_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let client_101_msgs = msgs.get(&ClientId::new(101)).unwrap();
    assert_eq!(client_100_msgs.len(), 2);
    assert_eq!(client_101_msgs.len(), 2);
}

#[test]
fn subscriber_removed_on_remove_client() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    assert!(screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));

    let _ = screen.remove_client(crate::ClientId::new(100));
    assert!(!screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));
}

#[test]
fn subscriber_removed_when_all_panes_closed() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(1),
    ));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_types::data::PaneId::Terminal(TERMINAL_1),
            }
        )
    });
    let has_exit = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            }
        )
    });
    assert!(has_pane_closed, "Should send SubscribedPaneClosed");
    assert!(has_exit, "Should send Exit when all panes closed");
    assert!(!screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));
}

#[test]
fn subscriber_partial_close() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(1),
    ));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_types::data::PaneId::Terminal(TERMINAL_1),
            }
        )
    });
    let has_exit = client_msgs
        .iter()
        .any(|m| matches!(m, ServerToClientMsg::Exit { .. }));
    assert!(
        has_pane_closed,
        "Should send SubscribedPaneClosed for closed pane"
    );
    assert!(!has_exit, "Should NOT send Exit when panes remain");
    assert!(screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));
    assert_eq!(
        screen
            .pane_render_subscribers
            .get(&ClientId::new(100))
            .unwrap()
            .pane_ids
            .len(),
        1
    );
}

#[test]
fn subscriber_full_close_sequence() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(1)),
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(1),
    ));
    screen.notify_pane_closed_to_subscribers(zellij_types::data::PaneId::Terminal(
        zellij_types::internal_ids::TerminalId::new(2),
    ));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let pane_closed_count = client_msgs
        .iter()
        .filter(|m| matches!(m, ServerToClientMsg::SubscribedPaneClosed { .. }))
        .count();
    let exit_count = client_msgs
        .iter()
        .filter(|m| {
            matches!(
                m,
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Normal,
                }
            )
        })
        .count();
    assert_eq!(pane_closed_count, 2, "Two SubscribedPaneClosed messages");
    assert_eq!(exit_count, 1, "One Exit message after all panes closed");
    assert!(!screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));
}

#[test]
fn delivery_path_a_and_b_produce_same_content() {
    for attached in [true, false] {
        let (mut screen, messages) =
            create_new_screen_with_message_capture(Size { cols: 60, rows: 6 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .new_pane(
                PaneId::Terminal(TerminalId::new(2)),
                None,
                None,
                false,
                false,
                NewPanePlacement::Tiled {
                    direction: Some(Direction::Right),
                    borderless: Some(false),
                },
                Some(ClientId::new(1)),
                None,
            )
            .unwrap();
        for (client, pane) in [(100, 1), (101, 2)] {
            screen.subscribe_to_pane_renders(
                ClientId::new(client),
                vec![zellij_types::data::PaneId::Terminal(TerminalId::new(pane))],
                None,
                false,
            );
        }
        if !attached {
            screen.remove_client(ClientId::new(1)).unwrap();
        }
        messages.lock().unwrap().clear();

        let mut expected = HashMap::new();
        for (client, pane, text) in [(100, 1, "first pane"), (101, 2, "second pane")] {
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .handle_pty_bytes(
                    TerminalId::new(pane),
                    format!(
                        "\x1b[31m{text}\x1b[0m\r\nnext line\r\nthird\r\nfourth\r\nfifth\r\nlast"
                    )
                    .into_bytes(),
                )
                .unwrap();
            expected.insert(
                ClientId::new(client),
                vec![ServerToClientMsg::PaneRenderUpdate {
                    pane_id: zellij_types::data::PaneId::Terminal(TerminalId::new(pane)),
                    viewport: [text, "next line", "third", "fourth", "fifth", "last"]
                        .map(str::to_owned)
                        .to_vec(),
                    scrollback: None,
                    is_initial: false,
                }],
            );
        }

        // Exercise the real render dispatch: a report with a regular client, direct pane
        // queries after detach. Both must deliver the literal input to the right subscriber.
        screen.render_to_clients().unwrap();
        assert_eq!(*messages.lock().unwrap(), expected, "attached={attached}");

        screen.render_to_clients().unwrap();
        assert_eq!(
            *messages.lock().unwrap(),
            expected,
            "unchanged output must not be delivered twice; attached={attached}"
        );
    }
}

#[test]
fn close_tab_notifies_subscribers() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![zellij_types::data::PaneId::Terminal(
            zellij_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    messages
        .lock()
        .unwrap()
        .get_mut(&ClientId::new(100))
        .unwrap()
        .clear();

    let _ = screen.go_to_tab(TabNumber::new(1), crate::ClientId::new(1));
    let _ = screen.close_tab(crate::ClientId::new(1));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_types::data::PaneId::Terminal(TERMINAL_1),
            }
        )
    });
    assert!(
        has_pane_closed,
        "Closing tab should notify subscriber of pane closure"
    );
}

#[test]
fn close_pane_notifies_subscribers_via_instruction() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let _session_metadata = mock_screen.clone_session_metadata();

    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };

    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: crate::ClientId::new(100),
            pane_ids: vec![
                zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(
                    0,
                )),
                zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(
                    1,
                )),
            ],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::CloseFocusedPane(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(200));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap_or(&vec![]).clone();
    let has_pane_closed = client_msgs
        .iter()
        .any(|m| matches!(m, ServerToClientMsg::SubscribedPaneClosed { .. }));
    assert!(
        has_pane_closed,
        "Closing pane via CLI action should notify subscriber. Messages: {:?}",
        client_msgs
    );
}

#[test]
fn integration_pty_bytes_delivered_to_subscriber() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: crate::ClientId::new(100),
            pane_ids: vec![zellij_types::data::PaneId::Terminal(
                zellij_types::internal_ids::TerminalId::new(0),
            )],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "hello world\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&ClientId::new(100)).unwrap_or(&vec![]).clone();

    assert!(
        subscriber_msgs.len() >= 2,
        "Should have at least initial + update, got {}",
        subscriber_msgs.len()
    );

    match &subscriber_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { is_initial, .. } => {
            assert!(*is_initial, "First message should be initial");
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }

    let has_hello = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial: false,
            viewport,
            ..
        } => viewport.iter().any(|line| line.contains("hello world")),
        _ => false,
    });
    assert!(
        has_hello,
        "Subsequent message should contain 'hello world' in viewport. Messages: {:?}",
        subscriber_msgs
    );
}

#[test]
fn integration_pty_bytes_not_delivered_when_viewport_unchanged() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: crate::ClientId::new(100),
            pane_ids: vec![zellij_types::data::PaneId::Terminal(
                zellij_types::internal_ids::TerminalId::new(0),
            )],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&ClientId::new(100)).unwrap_or(&vec![]).clone();

    let render_update_count = subscriber_msgs
        .iter()
        .filter(|m| matches!(m, ServerToClientMsg::PaneRenderUpdate { .. }))
        .count();
    assert_eq!(
        render_update_count, 1,
        "Only the initial delivery should be present, not a duplicate. Got {} messages: {:?}",
        render_update_count, subscriber_msgs
    );
}

fn assert_subscriber_scrollback(
    scrollback: Option<usize>,
    expected_scrollback: Option<Vec<String>>,
) {
    let size = Size { cols: 40, rows: 6 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.pane_frame_style = Some(PaneFrameStyle::None);
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let data = (0..8).map(|i| format!("line {i}\r\n")).collect::<String>();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PtyBytes(
            TerminalId::new(0),
            data.into_bytes(),
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: ClientId::new(100),
            pane_ids: vec![zellij_types::data::PaneId::Terminal(TerminalId::new(0))],
            scrollback,
            ansi: false,
        })
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PtyBytes(
            TerminalId::new(0),
            b"post subscribe\r\n".to_vec(),
        ))
        .unwrap();

    // All three instructions share the FIFO screen channel. The frame containing the new
    // text follows subscriber delivery, so this observes actual scheduled output before exit.
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "post-subscription output",
        |frame| frame.contains("post subscribe"),
    );
    mock_screen.teardown(vec![screen_thread, server_thread]);

    let messages = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_messages = messages
        .get(&ClientId::new(100))
        .expect("the subscriber must receive initial and subsequent output");
    assert_eq!(
        subscriber_messages,
        &vec![
            ServerToClientMsg::PaneRenderUpdate {
                pane_id: zellij_types::data::PaneId::Terminal(TerminalId::new(0)),
                viewport: ["line 3", "line 4", "line 5", "line 6", "line 7", ""]
                    .map(str::to_owned)
                    .to_vec(),
                scrollback: expected_scrollback,
                is_initial: true,
            },
            ServerToClientMsg::PaneRenderUpdate {
                pane_id: zellij_types::data::PaneId::Terminal(TerminalId::new(0)),
                viewport: ["line 4", "line 5", "line 6", "line 7", "post subscribe", ""]
                    .map(str::to_owned)
                    .to_vec(),
                scrollback: None,
                is_initial: false,
            },
        ],
        "scrollback={scrollback:?}"
    );
}

#[test]
fn integration_scrollback_from_pre_subscription_pty_bytes() {
    assert_subscriber_scrollback(
        Some(0),
        Some(["line 0", "line 1", "line 2"].map(str::to_owned).to_vec()),
    );
    assert_subscriber_scrollback(
        Some(2),
        Some(["line 1", "line 2"].map(str::to_owned).to_vec()),
    );
}

#[test]
fn integration_no_scrollback_when_not_requested() {
    assert_subscriber_scrollback(None, None);
}

#[test]
fn integration_subscriber_survives_after_regular_client_detach() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: crate::ClientId::new(100),
            pane_ids: vec![zellij_types::data::PaneId::Terminal(
                zellij_types::internal_ids::TerminalId::new(0),
            )],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RemoveClient(crate::ClientId::new(1)));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "after detach\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&ClientId::new(100)).unwrap_or(&vec![]).clone();

    let has_after_detach = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            viewport.iter().any(|line| line.contains("after detach"))
        },
        _ => false,
    });
    assert!(
        has_after_detach,
        "Subscriber should receive updates after regular client detach. Messages: {:?}",
        subscriber_msgs
    );
}

#[test]
fn pane_output_uses_subscriber_generations_and_cleans_up_each_client() {
    use zellij_types::internal_ids::PluginId;
    let plugin = PluginId::new(4);
    let other_plugin = PluginId::new(5);
    let client = ClientId::new(1);
    let other_client = ClientId::new(2);
    let filter = BTreeMap::from([("work".into(), "one".into())]);
    let origin = zellij_types::data::OriginatingPlugin::new(plugin, client, filter.clone());
    let layout = TiledPaneLayout {
        run: Some(Run::Command(RunCommand {
            command: "/usr/bin/true".into(),
            originating_plugin: Some(origin),
            ..Default::default()
        })),
        ..Default::default()
    };
    let mut mock = MockScreen::new(Size { cols: 80, rows: 20 });
    mock.drop_all_pty_messages();
    let received = mock.plugin_receiver.take().unwrap();
    let screen_thread = mock.run(Some(layout), vec![]);
    let subscribe = |plugin, client, generation| {
        ScreenInstruction::SubscribePaneOutput(plugin, client, Some(filter.clone()), generation)
    };
    let output = |n| ScreenInstruction::PtyBytes(TerminalId::new(0), vec![n]);
    for instruction in [
        subscribe(plugin, client, Some(10)),
        subscribe(plugin, other_client, Some(11)),
        subscribe(other_plugin, other_client, None),
        output(1),
        ScreenInstruction::RemoveClient(client),
        output(2),
        // Only plugin retirement removes a registration; disconnect preserves it.
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, Some(12)),
        output(3),
        subscribe(plugin, client, Some(12)),
        output(4),
        ScreenInstruction::UnsubscribePaneOutput(plugin),
        output(5),
        subscribe(plugin, client, Some(13)),
        output(6),
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, Some(13)),
        output(7),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    mock.teardown(vec![screen_thread]);
    let mut deliveries = received
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::DenoEvent(
                plugin,
                client,
                generation,
                Event::PaneOutput { data, .. },
            ) => vec![(data[0], plugin, client, Some(generation))],
            PluginInstruction::Update(events) => events
                .into_iter()
                .filter_map(|(plugin, client, event)| match event {
                    Event::PaneOutput { data, .. } => {
                        Some((data[0], plugin.unwrap(), client.unwrap(), None))
                    },
                    _ => None,
                })
                .collect(),
            _ => vec![],
        })
        .collect::<Vec<_>>();
    deliveries.sort();
    let mut expected = vec![
        (1, plugin, client, Some(10)),
        (2, plugin, client, Some(10)),
        (4, plugin, client, Some(12)),
        (6, plugin, client, Some(13)),
    ];
    for n in 1..=4 {
        expected.push((n, plugin, other_client, Some(11)));
    }
    for n in 1..=7 {
        expected.push((n, other_plugin, other_client, None));
    }
    expected.sort();
    assert_eq!(deliveries, expected);
}
