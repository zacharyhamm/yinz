#[cfg(test)]
const TERMINAL_1: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(1);

use super::super::subscriptions::SUBSCRIBER_SNAPSHOT_READS;
use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;

#[test]
fn list_clients_preserves_the_requesting_plugin_generation() {
    let mut screen = MockScreen::new(Size { cols: 80, rows: 20 });
    let receiver = screen.pty_receiver.take().unwrap();
    let received = Watched::new(vec![]);
    let logger = log_actions_in_thread!(received, PtyInstruction::Exit, receiver);
    let screen_thread = screen.run(None, vec![]);
    let plugin = crate::PluginId::new(7);
    let client = screen.main_client_id;
    for generation in [None, Some(42)] {
        screen
            .to_screen
            .send(ScreenInstruction::ListClientsToPlugin(
                plugin, client, generation,
            ))
            .unwrap();
    }
    let requests = received.wait_until(
        "both ListClients requests",
        yinz_test_support::default_timeout(),
        |log| {
            let requests: Vec<_> = log
                .iter()
                .filter_map(|instruction| match instruction {
                    PtyInstruction::ListClientsToPlugin(metadata, plugin, client, generation) => {
                        Some((
                            metadata.all_clients_metadata().contains_key(client),
                            *plugin,
                            *client,
                            *generation,
                        ))
                    },
                    _ => None,
                })
                .collect();
            (requests.len() == 2).then_some(requests)
        },
    );
    screen.teardown(vec![logger, screen_thread]);
    assert_eq!(
        requests,
        [
            (true, plugin, client, None),
            (true, plugin, client, Some(42))
        ]
    );
}

#[test]
fn subscriber_receives_initial_delivery() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
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
fn subscribers_reuse_plain_render_reports_and_read_missing_or_ansi_contents() {
    let client = ClientId::new(1);
    let plain = ClientId::new(100);
    let ansi = ClientId::new(101);
    let pane_id = PaneId::Terminal(TERMINAL_1);
    for include_report_contents in [false, true] {
        let (mut screen, messages) =
            create_new_screen_with_message_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.subscribe_to_pane_renders(plain, vec![pane_id], None, false);
        screen.subscribe_to_pane_renders(ansi, vec![pane_id], None, true);
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.handle_pty_bytes(TERMINAL_1, b"\x1b[31mupdated-content".to_vec())
            .unwrap();
        let pane = tab.get_pane_with_id(pane_id).unwrap();
        let direct_plain = pane.pane_contents(None, false, None).viewport;
        let direct_ansi = pane.pane_contents_with_ansi(None, false, None).viewport;
        assert_ne!(direct_plain, direct_ansi);
        let mut report = yinz_types::data::PaneRenderReport::default();
        let report_viewport = vec!["already-serialized-content".to_owned()];
        if include_report_contents {
            report.add_pane_contents(
                &[client],
                pane_id,
                PaneContents {
                    viewport: report_viewport.clone(),
                    ..Default::default()
                },
            );
        }
        let contents =
            Screen::subscriber_pane_contents(pane, None, false, None, false, Some(&report));
        if include_report_contents {
            assert!(matches!(contents, std::borrow::Cow::Borrowed(borrowed)
                if std::ptr::eq(borrowed, &report.all_pane_contents[&client][&pane_id])));
        } else {
            assert!(matches!(contents, std::borrow::Cow::Owned(_)));
        }
        assert!(matches!(
            Screen::subscriber_pane_contents(pane, None, false, None, true, Some(&report)),
            std::borrow::Cow::Owned(_)
        ));
        messages.lock().unwrap().clear();
        screen.deliver_to_pane_subscribers(Some(&report));
        let expected_plain = if include_report_contents {
            report_viewport
        } else {
            direct_plain
        };
        {
            let messages = messages.lock().unwrap();
            for (subscriber, expected) in [(plain, expected_plain), (ansi, direct_ansi)] {
                assert_eq!(messages[&subscriber].len(), 1);
                assert!(matches!(
                    &messages[&subscriber][0],
                    ServerToClientMsg::PaneRenderUpdate {
                        viewport, is_initial: false, scrollback: None, ..
                    } if viewport == &expected
                ));
            }
        }
        messages.lock().unwrap().clear();
        screen.pane_subscriber_snapshots_dirty = true;
        screen.deliver_to_pane_subscribers(Some(&report));
        assert!(messages.lock().unwrap().is_empty());
    }
}

#[test]
fn subscribers_receive_cached_plugin_contents_on_inactive_tabs() {
    let client = ClientId::new(1);
    let subscriber = ClientId::new(100);
    let plugin = crate::PluginId::new(20);
    let plugin_tab = TabId::new(0);
    for (ansi, keep_cached_client_attached) in
        [(false, true), (true, true), (false, false), (true, false)]
    {
        let (mut screen, messages) =
            create_new_screen_with_message_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, plugin_tab);
        let (plugin_sender, _plugin_receiver) = channels::unbounded();
        let tab = screen.tabs.get_mut(&plugin_tab).unwrap();
        tab.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
        tab.new_pane(
            PaneId::Plugin(plugin),
            None,
            None,
            false,
            false,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
        tab.handle_plugin_bytes(plugin, client, b"cached-plugin-content".to_vec())
            .unwrap();

        new_tab(&mut screen, 2, TabId::new(1));
        // These clients never visited the plugin, so neither has a cached grid.
        screen.add_client(ClientId::new(2)).unwrap();
        screen.add_client(ClientId::new(3)).unwrap();
        if !keep_cached_client_attached {
            screen.remove_client(client).unwrap();
        }
        assert!(screen.tabs[&plugin_tab].has_no_connected_clients());
        assert!(screen.tabs[&plugin_tab]
            .get_pane_with_id(PaneId::Plugin(plugin))
            .unwrap()
            .pane_contents(Some(ClientId::new(2)), false, None)
            .viewport
            .is_empty());
        screen.subscribe_to_pane_renders(subscriber, vec![PaneId::Plugin(plugin)], None, ansi);
        assert!(matches!(
            &messages.lock().unwrap()[&subscriber][0],
            ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: true, .. }
                if if keep_cached_client_attached {
                    viewport.iter().any(|line| line.contains("cached-plugin-content"))
                } else { viewport.is_empty() }
        ));
        // A plugin update must not invalidate this unrelated terminal subscription.
        screen.subscribe_to_pane_renders(
            ClientId::new(101),
            vec![PaneId::Terminal(TerminalId::new(2))],
            None,
            !ansi,
        );
        screen.deliver_to_pane_subscribers(None);
        messages.lock().unwrap().clear();
        SUBSCRIBER_SNAPSHOT_READS.with_borrow_mut(Vec::clear);

        screen
            .handle_plugin_render_assets(vec![crate::plugins::PluginRenderAsset::new(
                plugin,
                client,
                b"updated-plugin-content".to_vec(),
            )])
            .unwrap();
        screen.deliver_to_pane_subscribers(None);
        let reads = SUBSCRIBER_SNAPSHOT_READS.with_borrow_mut(std::mem::take);
        assert_eq!(
            reads,
            if keep_cached_client_attached {
                vec![(PaneId::Plugin(plugin), ansi)]
            } else {
                vec![]
            }
        );
        if keep_cached_client_attached {
            assert!(matches!(
                &messages.lock().unwrap()[&subscriber][0],
                ServerToClientMsg::PaneRenderUpdate { viewport, is_initial: false, .. }
                    if viewport.iter().any(|line| line.contains("updated-plugin-content"))
            ));
        } else {
            assert!(messages.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn subscribers_use_one_client_even_when_other_plugin_grids_have_contents() {
    let watcher = ClientId::new(1);
    let departed = ClientId::new(2);
    let live = ClientId::new(3);
    let later = ClientId::new(4);
    let followed = ClientId::new(5);
    let subscriber = ClientId::new(100);
    let plugins = [
        (crate::PluginId::new(20), live),
        (crate::PluginId::new(21), later),
        (crate::PluginId::new(22), followed),
    ];
    for (ansi, use_report) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut screen, messages) =
            create_new_screen_with_message_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.add_watcher_client(watcher).unwrap();
        screen.add_client(later).unwrap();
        screen.add_client(live).unwrap();
        screen.followed_client_id = Some(followed);
        let (sender, _receiver) = channels::unbounded();
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.senders.to_plugin = Some(SenderWithContext::new(sender));
        for (plugin, selected) in plugins {
            tab.new_pane(
                PaneId::Plugin(plugin),
                None,
                None,
                false,
                false,
                NewPanePlacement::default(),
                Some(live),
                None,
            )
            .unwrap();
            // Only the first plugin has a grid for the lowest eligible client.
            let pane = tab.get_pane_with_id_mut(PaneId::Plugin(plugin)).unwrap();
            for client in [live, later] {
                pane.remove_client(client);
            }
            // Higher, departed, and watcher grids must never substitute for that client.
            for (client, bytes) in [
                (watcher, "watcher-grid"),
                (departed, "departed-grid"),
                (selected, "live-grid"),
            ] {
                tab.handle_plugin_bytes(plugin, client, bytes.as_bytes().to_vec())
                    .unwrap();
            }
        }
        // The lowest eligible client wins even when another eligible grid is nonempty.
        tab.handle_plugin_bytes(plugins[0].0, later, b"later-grid".to_vec())
            .unwrap();
        let pane_ids = plugins.map(|(plugin, _)| PaneId::Plugin(plugin));
        screen.subscribe_to_pane_renders(subscriber, pane_ids.to_vec(), None, ansi);
        {
            let messages = messages.lock().unwrap();
            assert_eq!(messages[&subscriber].len(), plugins.len());
            for message in &messages[&subscriber] {
                assert!(matches!(message,
                    ServerToClientMsg::PaneRenderUpdate { pane_id, viewport, is_initial: true, .. }
                        if if *pane_id == pane_ids[0] {
                            viewport.concat().contains("live-grid")
                        } else { viewport.is_empty() }
                ));
            }
        }
        messages.lock().unwrap().clear();
        screen
            .handle_plugin_render_assets(
                plugins
                    .map(|(plugin, selected)| {
                        crate::plugins::PluginRenderAsset::new(
                            plugin,
                            selected,
                            b"live-update".to_vec(),
                        )
                    })
                    .to_vec(),
            )
            .unwrap();
        let mut report = yinz_types::data::PaneRenderReport::default();
        for (plugin, selected) in plugins {
            for (client, text) in [
                (watcher, "watcher-report"),
                (departed, "departed-report"),
                (selected, "live-report"),
            ] {
                report.add_pane_contents(
                    &[client],
                    PaneId::Plugin(plugin),
                    PaneContents {
                        viewport: vec![text.to_owned()],
                        ..Default::default()
                    },
                );
            }
        }
        report.add_pane_contents(
            &[later],
            pane_ids[0],
            PaneContents {
                viewport: vec!["later-report".to_owned()],
                ..Default::default()
            },
        );
        screen.deliver_to_pane_subscribers(use_report.then_some(&report));
        let expected = if use_report && !ansi {
            "live-report"
        } else {
            "live-update"
        };
        let messages = messages.lock().unwrap();
        assert_eq!(messages[&subscriber].len(), 1);
        assert!(matches!(&messages[&subscriber][0],
            ServerToClientMsg::PaneRenderUpdate { pane_id, viewport, is_initial: false, .. }
                if *pane_id == pane_ids[0] && viewport.concat().contains(expected)
        ));
        drop(messages);

        // An empty selected-client report is authoritative even with a nonempty direct grid.
        report.add_pane_contents(&[live], pane_ids[0], PaneContents::default());
        screen.mark_subscriber_pane_dirty(pane_ids[0]);
        screen.deliver_to_pane_subscribers(use_report.then_some(&report));
        if use_report && !ansi {
            assert!(
                screen.pane_render_subscribers[&subscriber].previous_viewports[&pane_ids[0]]
                    .is_empty()
            );
        }

        // Losing the selected grid cannot switch the source to a higher client's grid.
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .get_pane_with_id_mut(pane_ids[0])
            .unwrap()
            .remove_client(live);
        screen.mark_subscriber_pane_dirty(pane_ids[0]);
        screen.deliver_to_pane_subscribers(None);
        assert!(
            screen.pane_render_subscribers[&subscriber].previous_viewports[&pane_ids[0]].is_empty()
        );

        // Detaching the selected client does change the source to the next live client.
        screen.remove_client(live).unwrap();
        screen.deliver_to_pane_subscribers(None);
        assert!(
            screen.pane_render_subscribers[&subscriber].previous_viewports[&pane_ids[0]]
                .concat()
                .contains("later-grid")
        );

        // With no regular clients left, the retained followed client is the fallback.
        screen.remove_client(later).unwrap();
        screen.followed_client_id = Some(followed);
        screen.render().unwrap();
        screen.deliver_to_pane_subscribers(None);
        assert!(
            screen.pane_render_subscribers[&subscriber].previous_viewports[&pane_ids[2]]
                .concat()
                .contains("live-update")
        );
    }
}

#[test]
fn subscribers_only_serialize_dirty_panes_in_requested_formats() {
    let (mut screen, _) = create_new_screen_with_message_capture(Size { cols: 80, rows: 20 });
    for id in 1..=4 {
        new_tab(&mut screen, id, TabId::new(id as usize - 1));
    }
    let panes = [1, 2, 3, 4].map(|id| PaneId::Terminal(TerminalId::new(id)));
    for (id, pane, ansi) in [
        (100, panes[0], false),
        (101, panes[0], false),
        (102, panes[1], true),
        (103, panes[1], true),
        (104, panes[2], false),
        (105, panes[2], true),
    ] {
        screen.subscribe_to_pane_renders(ClientId::new(id), vec![pane], None, ansi);
    }
    screen.deliver_to_pane_subscribers(None);
    let take_reads = || SUBSCRIBER_SNAPSHOT_READS.with_borrow_mut(std::mem::take);
    take_reads();
    let mut pending = super::super::thread_main::PendingEvents::default();

    screen
        .handle_pty_bytes(TerminalId::new(4), b"unsubscribed".to_vec(), &mut pending)
        .unwrap();
    screen.deliver_to_pane_subscribers(None);
    assert!(take_reads().is_empty());
    assert!(!screen.subscriber_snapshots_pending());

    screen
        .handle_pty_bytes(TerminalId::new(1), b"plain".to_vec(), &mut pending)
        .unwrap();
    screen.deliver_to_pane_subscribers(None);
    assert_eq!(take_reads(), [(panes[0], false)]);

    screen.deferred_subscriber_renders = true;
    for (id, bytes) in [(2, "ANSI"), (3, "both"), (2, " latest")] {
        screen
            .handle_pty_bytes(TerminalId::new(id), bytes.as_bytes().to_vec(), &mut pending)
            .unwrap();
        screen.deliver_to_pane_subscribers(None);
    }
    assert!(take_reads().is_empty());
    assert!(screen.subscriber_snapshots_pending());
    screen.deferred_subscriber_renders = false;
    screen.deliver_to_pane_subscribers(None);
    let mut reads = take_reads();
    reads.sort();
    assert_eq!(
        reads,
        [(panes[1], true), (panes[2], false), (panes[2], true)]
    );
    assert!(
        screen.pane_render_subscribers[&ClientId::new(102)].previous_viewports[&panes[1]]
            .concat()
            .contains("ANSI latest")
    );
    screen.deliver_to_pane_subscribers(None);
    assert!(take_reads().is_empty());

    screen.render().unwrap();
    screen.deliver_to_pane_subscribers(None);
    let mut reads = take_reads();
    reads.sort();
    assert_eq!(
        reads,
        [
            (panes[0], false),
            (panes[1], true),
            (panes[2], false),
            (panes[2], true)
        ]
    );

    screen.mark_subscriber_pane_dirty(panes[0]);
    screen.notify_pane_closed_to_subscribers(panes[0]);
    assert!(!screen.pane_subscriber_dirty_panes.contains(&panes[0]));
    screen.mark_subscriber_pane_dirty(panes[1]);
    screen.pane_render_subscribers.clear();
    screen.deliver_to_pane_subscribers(None);
    assert!(screen.pane_subscriber_dirty_panes.is_empty());
    assert!(take_reads().is_empty());
}

#[test]
fn subscriber_receives_initial_with_scrollback() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
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
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        std::borrow::Cow::Owned(PaneContents {
            viewport: initial_viewport,
            ..Default::default()
        }),
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        std::borrow::Cow::Owned(PaneContents {
            viewport: vec!["changed line".to_string()],
            ..Default::default()
        }),
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(999),
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
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    let sub = screen
        .pane_render_subscribers
        .get(&ClientId::new(100))
        .unwrap();
    assert_eq!(sub.pane_ids.len(), 2);
    assert!(sub.pane_ids.contains(&yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(1)
    )));
    assert!(sub.pane_ids.contains(&yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(2)
    )));
}

#[test]
fn multiple_subscribers_receive_updates() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        crate::ClientId::new(101),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        std::borrow::Cow::Owned(PaneContents {
            viewport: vec!["new content".to_string()],
            ..Default::default()
        }),
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    assert!(screen
        .pane_render_subscribers
        .contains_key(&ClientId::new(100)));

    screen.pane_subscriber_snapshots_dirty = false;
    screen.remove_client(crate::ClientId::new(100)).unwrap();
    assert!(!screen.pane_subscriber_snapshots_dirty);
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(1),
    ));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: yinz_types::data::PaneId::Terminal(TERMINAL_1),
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
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(1),
    ));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: yinz_types::data::PaneId::Terminal(TERMINAL_1),
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
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(2)),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(1),
    ));
    screen.notify_pane_closed_to_subscribers(yinz_types::data::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(2),
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
                vec![yinz_types::data::PaneId::Terminal(TerminalId::new(pane))],
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
                    pane_id: yinz_types::data::PaneId::Terminal(TerminalId::new(pane)),
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
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
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
                pane_id: yinz_types::data::PaneId::Terminal(TERMINAL_1),
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
                yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(0)),
                yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
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
            pane_ids: vec![yinz_types::data::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(0),
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
            pane_ids: vec![yinz_types::data::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(0),
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
            pane_ids: vec![yinz_types::data::PaneId::Terminal(TerminalId::new(0))],
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
                pane_id: yinz_types::data::PaneId::Terminal(TerminalId::new(0)),
                viewport: ["line 3", "line 4", "line 5", "line 6", "line 7", ""]
                    .map(str::to_owned)
                    .to_vec(),
                scrollback: expected_scrollback,
                is_initial: true,
            },
            ServerToClientMsg::PaneRenderUpdate {
                pane_id: yinz_types::data::PaneId::Terminal(TerminalId::new(0)),
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
            pane_ids: vec![yinz_types::data::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(0),
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
    use yinz_types::internal_ids::PluginId;
    let plugin = PluginId::new(4);
    let other_plugin = PluginId::new(5);
    let client = ClientId::new(1);
    let other_client = ClientId::new(2);
    let filter = BTreeMap::from([("work".into(), "one".into())]);
    let origin = yinz_types::data::OriginatingPlugin::new(plugin, client, filter.clone());
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
        // A parent and two workers can all subscribe for the same acting client.
        subscribe(plugin, client, Some(10)),
        subscribe(plugin, client, Some(11)),
        subscribe(plugin, client, Some(12)),
        subscribe(plugin, other_client, Some(10)),
        subscribe(plugin, other_client, Some(11)),
        subscribe(plugin, other_client, Some(12)),
        subscribe(plugin, client, None),
        subscribe(other_plugin, client, Some(11)),
        subscribe(other_plugin, other_client, None),
        output(1),
        // Replacing a filter affects only this generation and client.
        ScreenInstruction::SubscribePaneOutput(
            plugin,
            client,
            Some(BTreeMap::from([("work".into(), "different".into())])),
            Some(11),
        ),
        output(2),
        subscribe(plugin, client, Some(11)),
        ScreenInstruction::RemoveClient(client),
        output(3),
        // Explicit unsubscribe leaves this worker's other client registered.
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, Some(11)),
        output(4),
        // Disposal removes only this generation, across all acting clients.
        ScreenInstruction::UnsubscribePaneOutput(plugin, Some(12)),
        output(5),
        ScreenInstruction::UnsubscribePaneOutput(plugin, Some(10)),
        output(6),
        subscribe(plugin, client, Some(13)),
        subscribe(plugin, other_client, Some(13)),
        output(7),
        // A late disposal cannot unsubscribe the replacement.
        ScreenInstruction::UnsubscribePaneOutput(plugin, Some(10)),
        output(8),
        ScreenInstruction::UnsubscribePaneOutput(plugin, None),
        output(9),
        subscribe(plugin, client, Some(14)),
        output(10),
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, Some(14)),
        output(11),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    mock.teardown(vec![screen_thread]);
    let mut deliveries = received
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginEvent(
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
        (1, plugin, client, Some(11)),
        (3, plugin, client, Some(11)),
        (10, plugin, client, Some(14)),
    ];
    for client in [client, other_client] {
        for n in 1..=5 {
            expected.push((n, plugin, client, Some(10)));
        }
        for n in 1..=4 {
            expected.push((n, plugin, client, Some(12)));
        }
        for n in 7..=8 {
            expected.push((n, plugin, client, Some(13)));
        }
    }
    for n in 1..=8 {
        expected.push((n, plugin, other_client, Some(11)));
        expected.push((n, plugin, client, None));
    }
    for n in 1..=11 {
        expected.push((n, other_plugin, client, Some(11)));
        expected.push((n, other_plugin, other_client, None));
    }
    expected.sort();
    assert_eq!(deliveries, expected);
}
