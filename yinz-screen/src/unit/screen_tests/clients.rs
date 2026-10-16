#[cfg(test)]
const PLUGIN_2: yinz_types::internal_ids::PluginId = yinz_types::internal_ids::PluginId::new(2);

use super::*;
use yinz_types::internal_ids::TabId;

#[test]
fn watchers_resume_incremental_output_when_a_regular_client_returns() {
    let size = Size { cols: 80, rows: 24 };
    let first = ClientId::new(1);
    let followed = ClientId::new(2);
    let watcher = ClientId::new(3);
    for returning in [first, followed] {
        let mut screen = create_new_screen(size, true, true);
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        screen.add_client(followed).unwrap();
        screen.set_followed_client(followed).unwrap();
        screen.add_watcher_client(watcher).unwrap();
        screen.add_client(watcher).unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(size);
        watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        screen.remove_client(first).unwrap();
        screen.remove_client(followed).unwrap();
        watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert_eq!(screen.followed_client_id, Some(followed));
        assert_eq!(screen.departed_followed_tab_id, Some(TabId::new(1)));

        // A one-shot CLI connection can reuse the departed ID without attaching.
        screen.remove_client(followed).unwrap();
        assert_eq!(screen.departed_followed_tab_id, Some(TabId::new(1)));
        screen
            .tabs
            .get_mut(&TabId::new(1))
            .unwrap()
            .handle_pty_bytes(
                crate::TerminalId::new(2),
                b"after-cli-disconnect\r\n".to_vec(),
            )
            .unwrap();
        let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert!(bytes.contains("after-cli-disconnect"), "{bytes:?}");
        assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");

        screen.add_client(returning).unwrap();
        watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert_eq!(screen.followed_client_id, Some(returning));
        assert_eq!(screen.departed_followed_tab_id, None);
        assert_eq!(
            screen.watched_tab_id(),
            screen.active_tab_ids.get(&returning).copied()
        );
        screen
            .tabs
            .get_mut(&TabId::new(1))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(2), b"after-reattach".to_vec())
            .unwrap();
        let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert!(bytes.contains("after-reattach"), "{bytes:?}");
        assert!(
            !bytes.contains("\x1b[2J"),
            "ordinary output must remain incremental"
        );
    }
}

#[test]
fn watchers_retain_the_departed_clients_tab_until_it_closes() {
    let size = Size { cols: 80, rows: 24 };
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    for fallback in [TabId::new(0), TabId::new(2)] {
        let mut screen = create_new_screen(size, true, true);
        new_tab(&mut screen, 1, fallback);
        new_tab(&mut screen, 2, TabId::new(1));
        screen.set_followed_client(followed).unwrap();
        screen.add_watcher_client(watcher).unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(size);
        for (tab_id, terminal, content) in [
            (fallback, 1, "other-tab"),
            (TabId::new(1), 2, "watched-tab"),
        ] {
            screen
                .tabs
                .get_mut(&tab_id)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(terminal), content.as_bytes().to_vec())
                .unwrap();
        }
        watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        screen.remove_client(followed).unwrap();
        assert!(!screen.active_tab_ids.contains_key(&followed));
        for content in ["after-detach", "incremental-update"] {
            screen
                .tabs
                .get_mut(&TabId::new(1))
                .unwrap()
                .handle_pty_bytes(TerminalId::new(2), format!("\r\n{content}").into_bytes())
                .unwrap();
            let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
            let snapshot = format!("{grid:?}");
            assert!(snapshot.contains("watched-tab"), "{snapshot}");
            assert!(snapshot.contains(content), "{snapshot}");
            assert!(!snapshot.contains("other-tab"), "{snapshot}");
            assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");
        }
        screen.close_tab_by_id(TabId::new(1)).unwrap();
        assert_eq!(screen.watched_tab_id(), Some(fallback));
        assert!(screen.watcher_clients[&watcher].should_force_render());
        let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert!(bytes.contains("\x1b[2J"), "{bytes:?}");
        let snapshot = format!("{grid:?}");
        assert!(snapshot.contains("other-tab"), "{snapshot}");
        assert!(!snapshot.contains("watched-tab"), "{snapshot}");
        assert!(!screen.watcher_clients[&watcher].should_force_render());
        assert!(!watcher_frame(&mut screen, &receiver, watcher, &mut grid).contains("\x1b[2J"));
        screen.close_tab_by_id(fallback).unwrap();
        assert!(receiver
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))));
    }
}

#[test]
fn departed_plugin_output_updates_watchers_and_always_releases_its_blocker() {
    let size = Size { cols: 80, rows: 24 };
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let departed = ClientId::new(3);
    let plugin = crate::PluginId::new(2);
    let (mut screen, _capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(plugin), false);
    screen.add_client(departed).unwrap();
    screen.set_followed_client(followed).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.remove_client(departed).unwrap();
    screen.remove_client(followed).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    for (client, content) in [
        (followed, "accepted-plugin"),
        (departed, "discarded-plugin"),
    ] {
        screen.render_blocker.register_blocking_plugin(plugin);
        assert!(screen
            .handle_plugin_render_assets(vec![PluginRenderAsset::new(
                plugin,
                client,
                content.as_bytes().to_vec(),
            )])
            .unwrap()
            .is_empty());
        assert!(!screen.render_blocker.blocking_plugins.contains_key(&plugin));
        assert!(screen.show_plugin_cursor(plugin, client, Some((1, 1))));
    }
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    let snapshot = format!("{grid:?}");
    assert!(snapshot.contains("accepted-plugin"), "{snapshot}");
    assert!(!snapshot.contains("discarded-plugin"), "{snapshot}");
    let pane = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Plugin(plugin))
        .unwrap();
    assert!(pane.cursor_coordinates(Some(followed)).is_some());
    assert!(pane.cursor_coordinates(Some(departed)).is_none());
    assert!(!pane
        .pane_contents(Some(departed), false, None)
        .viewport
        .concat()
        .contains("discarded-plugin"));
}

#[test]
fn retained_plugin_state_is_released_when_unused_or_replaced() {
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let plugin = crate::PluginId::new(2);
    for transition in 0..3 {
        let (mut screen, _capture) =
            create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(plugin), false);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_plugin_bytes(plugin, followed, b"departed-client".to_vec())
            .unwrap();
        screen.set_followed_client(followed).unwrap();
        screen.add_watcher_client(watcher).unwrap();
        screen.remove_client(followed).unwrap();
        screen.show_plugin_cursor(plugin, followed, Some((1, 1)));
        assert!(screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Plugin(plugin))
            .unwrap()
            .pane_contents(Some(followed), false, None)
            .viewport
            .concat()
            .contains("departed-client"));
        match transition {
            0 => screen.remove_watcher_client(watcher),
            1 => screen.set_followed_client(ClientId::new(3)).unwrap(),
            _ => screen.add_client(ClientId::new(3)).unwrap(),
        }
        assert_eq!(screen.departed_followed_tab_id, None);
        let pane = screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Plugin(plugin))
            .unwrap();
        assert!(pane.cursor_coordinates(Some(followed)).is_none());
        assert!(!pane
            .pane_contents(Some(followed), false, None)
            .viewport
            .concat()
            .contains("departed-client"));
    }
}

#[test]
fn late_plugin_output_is_discarded_after_the_last_watcher_leaves() {
    let size = Size { cols: 80, rows: 24 };
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let (mut screen, _capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(PLUGIN_2), false);
    screen
        .focus_pane_with_id(PaneId::Plugin(PLUGIN_2), false, false, followed)
        .unwrap();
    screen
        .handle_plugin_render_assets(vec![PluginRenderAsset::new(
            PLUGIN_2,
            followed,
            b"before-detach".to_vec(),
        )])
        .unwrap();
    assert!(screen.show_plugin_cursor(PLUGIN_2, followed, Some((1, 1))));
    screen.set_followed_client(followed).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.remove_client(followed).unwrap();
    screen.remove_watcher_client(watcher);
    let pane = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Plugin(PLUGIN_2))
        .unwrap();
    assert!(!pane
        .pane_contents(Some(followed), false, None)
        .viewport
        .concat()
        .contains("before-detach"));
    assert_eq!(pane.cursor_coordinates(Some(followed)), None);
    screen
        .handle_plugin_render_assets(vec![PluginRenderAsset::new(
            PLUGIN_2,
            followed,
            b"\r\nplugin-without-watchers".to_vec(),
        )])
        .unwrap();
    assert!(screen.show_plugin_cursor(PLUGIN_2, followed, Some((3, 2))));

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(crate::TerminalId::new(1), b"while-nobody-watched".to_vec())
        .unwrap();
    screen.add_watcher_client(watcher).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(screen.followed_client_id, None);
    assert!(!bytes.contains("plugin-without-watchers"), "{bytes:?}");
    let pane = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Plugin(PLUGIN_2))
        .unwrap();
    assert_eq!(pane.cursor_coordinates(Some(followed)), None);
}

#[test]
fn attaching_terminal_client_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .new_floating_pane(
            PaneId::Terminal(crate::TerminalId::new(3)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .unwrap();

    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(2)),
        Some(&TabId::new(1)),
        "terminal client is on the host tab"
    );
    assert!(
        screen
            .tabs
            .get(&TabId::new(1))
            .unwrap()
            .are_floating_panes_visible(),
        "the floating surface remains visible"
    );
}

#[test]
fn attaching_watcher_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    screen
        .add_watcher_client(crate::ClientId::new(2))
        .expect("TEST");
    screen.add_client(crate::ClientId::new(2)).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&ClientId::new(2)),
        Some(&TabId::new(1)),
        "watcher client mirrors the host tab"
    );
}

#[test]
fn reattaching_the_only_client_tells_the_tab_plugins_they_are_visible_again() {
    // A tab whose last client leaves tells its plugins nobody is looking; a client
    // arriving in that tab has to tell them the opposite, otherwise a plugin that
    // went quiet on `Visible(false)` never wakes up again after a reattach.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(2)),
        false,
    );
    let _ = capture.drain_plugin_updates();

    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");

    let visibility_events: Vec<bool> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(|(pid, _cid, event)| match (pid, event) {
            (Some(PLUGIN_2), Event::Visible(is_visible)) => Some(is_visible),
            _ => None,
        })
        .collect();
    assert_eq!(
        visibility_events,
        vec![false, true],
        "the plugin should be told the tab went off screen and then came back"
    );
}

#[test]
fn reattaching_a_client_restores_floating_pane_visibility_notifications() {
    // While a tab has no clients its floating plugins are not told about the surface
    // being lowered; `add_client` marks the tab visible again, and from then on they
    // must be, otherwise a plugin idling on a timer keeps working while off screen.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    // focused, so that the floating surface actually comes up with the pane
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .new_pane(
            PaneId::Plugin(crate::PluginId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    assert!(
        screen
            .get_active_tab_mut(crate::ClientId::new(1))
            .unwrap()
            .are_floating_panes_visible(),
        "precondition: the floating surface is up"
    );

    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    let _ = capture.drain_plugin_updates();
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .unwrap()
        .hide_floating_panes();

    let visibility_events: Vec<(Option<yinz_types::internal_ids::PluginId>, bool)> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(|(pid, _cid, event)| match event {
            Event::Visible(is_visible) => Some((pid, is_visible)),
            _ => None,
        })
        .collect();
    assert!(
        visibility_events.contains(&(Some(yinz_types::internal_ids::PluginId::new(2)), false)),
        "a floating plugin must still be told when its surface is hidden after a reattach, got {visibility_events:?}"
    );
}

/// A watcher's cursor crop is decided during serialization, in that watcher's copy of the frame
/// alone - the tab's memo of what the host was told knows nothing about it. Recording it is all
/// this does: the next frame reads it and corrects the memo before placing the cursor, so neither
/// direction has to buy a whole repaint to get the cursor stated again.
#[test]
fn a_watchers_cursor_crop_is_recorded_without_asking_for_a_full_frame() {
    let mut watcher_state = crate::screen::WatcherState::new(Size { cols: 40, rows: 10 });
    watcher_state.clear_force_render();

    watcher_state.set_cursor_was_cropped(true);
    assert!(watcher_state.cursor_was_cropped());
    assert!(
        !watcher_state.should_force_render(),
        "the cursor going out of range is hidden again by every later frame's serialization"
    );

    watcher_state.set_cursor_was_cropped(false);
    assert!(!watcher_state.cursor_was_cropped());
    assert!(
        !watcher_state.should_force_render(),
        "and coming back into range is put right by the frame that brings it back"
    );
}

#[test]
fn changing_followed_client_restates_the_watchers_cursor() {
    let size = Size { cols: 80, rows: 24 };
    let a = ClientId::new(1);
    let b = ClientId::new(2);
    let watcher = ClientId::new(3);
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.add_client(b).unwrap();
    screen
        .go_to_tab(yinz_types::internal_ids::TabNumber::new(1), a)
        .unwrap();
    for (tab, terminal, bytes) in [(0, 1, "\x1b[5;5H\x1b[5 q"), (1, 2, "\x1b[8;9H\x1b[3 q")] {
        screen
            .tabs
            .get_mut(&TabId::new(tab))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(terminal), bytes.as_bytes().to_vec())
            .unwrap();
    }
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, size);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    for (followed, goto, shape, visible) in [
        (a, "\x1b[5;5H", "\x1b[5 q", true),
        (b, "\x1b[8;9H", "\x1b[3 q", true),
        (a, "\x1b[5;5H", "\x1b[5 q", true),
        (b, "\x1b[8;9H", "", false),
        (a, "\x1b[5;5H", "\x1b[5 q", true),
    ] {
        if !visible {
            screen
                .tabs
                .get_mut(&TabId::new(1))
                .unwrap()
                .handle_pty_bytes(crate::TerminalId::new(2), b"\x1b[?25l".to_vec())
                .unwrap();
        }
        screen.set_followed_client(followed).unwrap();
        assert!(screen.watcher_clients[&watcher].should_force_render());
        screen.render_to_clients().unwrap();
        let bytes: String = receiver
            .try_iter()
            .filter_map(|(instruction, _)| {
                if let ServerInstruction::Render(Some(mut clients)) = instruction {
                    clients.remove(&watcher)
                } else {
                    None
                }
            })
            .collect();
        assert!(bytes.contains(goto) && bytes.contains(shape), "{bytes:?}");
        assert_eq!(bytes.contains("\x1b[?25h"), visible, "{bytes:?}");
        take_snapshot_and_cursor_coordinates(&bytes, &mut grid);
        let expected = pane_cursor(&mut screen, followed);
        assert_eq!(
            grid.cursor_coordinates(),
            Some((expected.0, expected.1, visible))
        );
    }
}

#[test]
fn watcher_crop_state_changes_only_for_a_delivered_stream() {
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, Size { cols: 40, rows: 10 });
    let receiver = screen_wired_to_server(&mut screen);
    screen
        .watcher_clients
        .get_mut(&watcher)
        .unwrap()
        .set_cursor_was_cropped(true);
    screen
        .watcher_clients
        .get_mut(&watcher)
        .unwrap()
        .clear_force_render();

    let mut output = crate::output::Output::default();
    // A stream for another client must not change the followed client's crop state.
    output.add_post_vte_instruction_to_client(ClientId::new(3), "\x1b[?25l");
    assert!(output.is_dirty());
    screen.render_watcher_output(&output, followed).unwrap();
    assert!(screen.watcher_clients[&watcher].cursor_was_cropped);
    assert!(!screen.watcher_clients[&watcher].should_force_render());
    assert!(receiver.try_recv().is_err());

    // Cursor-only output delivers a stream and updates crop state without pane registration: a
    // pre- or post-VTE instruction puts its client in the frame, which is what lets a held-back
    // tab still deliver its bell and OSC 7 (see `render_pacing`).
    output.add_post_vte_instruction_to_client(followed, "\x1b[?25l");
    output.set_cursor_coordinates(followed, 2, 2);
    screen.render_watcher_output(&output, followed).unwrap();
    assert!(!screen.watcher_clients[&watcher].cursor_was_cropped);
    assert!(!screen.watcher_clients[&watcher].should_force_render());
    assert!(receiver.try_recv().is_ok());
    output.set_cursor_coordinates(followed, 60, 2);
    screen.render_watcher_output(&output, followed).unwrap();
    assert!(screen.watcher_clients[&watcher].cursor_was_cropped);
    assert!(!screen.watcher_clients[&watcher].should_force_render());
}

/// Drive one frame and hand back what the watcher's terminal was sent, with that watcher's own
/// `Grid` advanced by it - so an out-of-range goto is clamped exactly as a real host clamps it.
fn watcher_frame(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    watcher: ClientId,
    grid: &mut Grid,
) -> String {
    screen.render_to_clients().expect("TEST");
    let mut bytes = String::new();
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            if let Some(client_output) = rendered_clients.remove(&watcher) {
                bytes.push_str(&client_output);
            }
        }
    }
    take_snapshot_and_cursor_coordinates(&bytes, grid);
    bytes
}

#[test]
fn watcher_full_repaints_clear_outside_the_session_surface() {
    let client = ClientId::new(1);
    let watcher = ClientId::new(3);
    let size = Size {
        cols: 123,
        rows: 24,
    };
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(2)).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, size);
    screen.set_followed_client(client).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);

    for change in [
        "attach",
        "watcher resize",
        "session shrink",
        "followed client",
        "followed tab",
    ] {
        match change {
            "watcher resize" => screen.set_watcher_size(
                watcher,
                Size {
                    cols: 122,
                    rows: 23,
                },
            ),
            "session shrink" => screen
                .resize_to_screen(Size { cols: 80, rows: 12 })
                .unwrap(),
            "followed client" => screen.set_followed_client(ClientId::new(2)).unwrap(),
            "followed tab" => new_tab(&mut screen, 2, TabId::new(1)),
            _ => {},
        }
        // Seed both the right and bottom margins of the watcher's host terminal.
        take_snapshot_and_cursor_coordinates("\x1b[1;122H X\x1b[23;1Hstale", &mut grid);
        let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert!(bytes.contains("\x1b[2J"), "{change}: {bytes:?}");
        assert!(!grid.dump_screen(false).contains('X'), "{change}");
        assert!(!grid.dump_screen(false).contains("stale"), "{change}");
    }
}

#[test]
fn a_wider_shorter_watchers_incremental_output_preserves_unchanged_rows() {
    let watcher = ClientId::new(2);
    let watcher_size = Size {
        cols: 123,
        rows: 10,
    };
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, watcher_size);
    screen.set_followed_client(ClientId::new(1)).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(watcher_size);

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            b"first-row\x1b[10;1Hunchanged-tenth-row".to_vec(),
        )
        .unwrap();
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(
        grid.dump_screen(false).lines().nth(9),
        Some("unchanged-tenth-row")
    );

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"\x1b[1;1Hupdated-row".to_vec())
        .unwrap();
    let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert!(bytes.contains("updated-row"), "{bytes:?}");
    assert!(!bytes.contains("\x1b[K"), "{bytes:?}");
    assert!(!bytes.contains("\x1b[J"), "{bytes:?}");
    assert!(
        !bytes.contains("unchanged-tenth-row"),
        "only dirty rows are replayed: {bytes:?}"
    );
    assert!(!bytes.contains("\x1b[2J"), "{bytes:?}");
    assert_eq!(grid.dump_screen(false).lines().next(), Some("updated-row"));
    assert_eq!(
        grid.dump_screen(false).lines().nth(9),
        Some("unchanged-tenth-row")
    );
}

#[test]
fn a_larger_watchers_shape_only_update_preserves_content_and_cursor() {
    let watcher = ClientId::new(2);
    let watcher_size = Size {
        cols: 123,
        rows: 22,
    };
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, watcher_size);
    screen
        .update_synchronized_output_support(watcher, true)
        .unwrap();
    screen.set_followed_client(ClientId::new(1)).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(watcher_size);

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"unchanged-content\x1b[4;7H".to_vec())
        .unwrap();
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(grid.cursor_coordinates(), Some((6, 3, true)));
    let original_shape = grid.cursor_shape();

    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(1), b"\x1b[6 q".to_vec())
        .unwrap();
    let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(bytes, "\x1b[6 q");
    assert_eq!(
        grid.dump_screen(false).lines().next(),
        Some("unchanged-content")
    );
    assert!(!bytes.contains("unchanged-content"), "{bytes:?}");
    assert!(
        !bytes.contains("\x1b[4;7H"),
        "cursor position is memoized: {bytes:?}"
    );
    assert_eq!(grid.cursor_coordinates(), Some((6, 3, true)));
    assert_ne!(
        grid.cursor_shape().get_csi_str(),
        original_shape.get_csi_str()
    );
    assert_eq!(grid.cursor_shape().get_csi_str(), "\x1b[6 q");
}

/// A watcher smaller than the session it follows has its cursor hidden during serialization when
/// the followed client's cursor falls outside its size - in that one watcher's byte stream, which
/// the tab's memo of what the host was told cannot see. The frame that brings the cursor back into
/// range has to be the frame that shows it again: nothing schedules another one, so an idle session
/// would otherwise leave the watcher's cursor down for good.
#[test]
fn a_watchers_cropped_cursor_returns_in_the_frame_that_uncrops_it() {
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let watcher_size = Size { cols: 40, rows: 10 };
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_watcher_client(watcher).expect("TEST");
    screen.set_watcher_size(watcher, watcher_size);
    // the whole point of the bug: nothing hides the cursor for the frame on a host that presents
    // frames atomically, so `must_show` has only the memo to go on
    screen
        .update_synchronized_output_support(watcher, true)
        .expect("TEST");
    screen.set_followed_client(followed).expect("TEST");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(watcher_size);

    let put_cursor = |screen: &mut Screen, bytes: &str| {
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(1), bytes.as_bytes().to_vec())
            .unwrap();
    };

    put_cursor(&mut screen, "\x1b[3;3H");
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(
        grid.cursor_coordinates(),
        Some((2, 2, true)),
        "precondition: a cursor inside the watcher's size is shown"
    );

    put_cursor(&mut screen, "\x1b[20;60H");
    let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        bytes.ends_with("\x1b[?25l"),
        "the cursor left this watcher's size, so its stream ends with a hide: {bytes:?}"
    );
    assert!(
        !grid
            .cursor_coordinates()
            .map(|(_, _, visible)| visible)
            .unwrap_or(false),
        "and the watcher's terminal has it down"
    );

    put_cursor(&mut screen, "\x1b[3;3H");
    let bytes = watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        bytes.contains("\x1b[?25h"),
        "the frame that brings the cursor back into range shows it again: {bytes:?}"
    );
    assert!(
        !bytes.contains(" q"),
        "without re-sending the shape, which would restart the host's blink phase: {bytes:?}"
    );
    assert_eq!(
        grid.cursor_coordinates(),
        Some((2, 2, true)),
        "in that very frame - a second render is never scheduled"
    );
}

/// The correction says what the host has done to the cursor, not what the followed pane wants: an
/// application that hid its own cursor while it was out of range must not have it handed back.
#[test]
fn a_cursor_the_app_hid_while_cropped_stays_hidden_when_it_returns() {
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let watcher_size = Size { cols: 40, rows: 10 };
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_watcher_client(watcher).expect("TEST");
    screen.set_watcher_size(watcher, watcher_size);
    screen
        .update_synchronized_output_support(watcher, true)
        .expect("TEST");
    screen.set_followed_client(followed).expect("TEST");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(watcher_size);

    let put_cursor = |screen: &mut Screen, bytes: &str| {
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(1), bytes.as_bytes().to_vec())
            .unwrap();
    };

    put_cursor(&mut screen, "\x1b[20;60H");
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    put_cursor(&mut screen, "\x1b[?25l");
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);

    put_cursor(&mut screen, "\x1b[3;3H");
    watcher_frame(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        !grid
            .cursor_coordinates()
            .map(|(_, _, visible)| visible)
            .unwrap_or(false),
        "the app put this cursor down and nothing here picks it back up"
    );
}

/// One memo entry serves every watcher, so the correction reaches a watcher that was never cropped
/// too. It costs that watcher a `?25h` and a goto it did not need, and must leave its cursor right.
#[test]
fn correcting_a_small_watchers_cursor_leaves_a_larger_watchers_alone() {
    let followed = ClientId::new(1);
    let small = ClientId::new(2);
    let large = ClientId::new(3);
    let small_size = Size { cols: 40, rows: 10 };
    let large_size = Size { cols: 80, rows: 24 };
    let mut screen = create_new_screen(large_size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    for (watcher, size) in [(small, small_size), (large, large_size)] {
        screen.add_watcher_client(watcher).expect("TEST");
        screen.set_watcher_size(watcher, size);
        screen
            .update_synchronized_output_support(watcher, true)
            .expect("TEST");
    }
    screen.set_followed_client(followed).expect("TEST");
    let receiver = screen_wired_to_server(&mut screen);
    let mut small_grid = grid_for_render_snapshot(small_size);
    let mut large_grid = grid_for_render_snapshot(large_size);

    for bytes in ["\x1b[3;3H", "\x1b[20;60H", "\x1b[3;3H"] {
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(crate::TerminalId::new(1), bytes.as_bytes().to_vec())
            .unwrap();
        screen.render_to_clients().expect("TEST");
        while let Ok((instruction, _)) = receiver.try_recv() {
            if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
                if let Some(client_output) = rendered_clients.remove(&small) {
                    take_snapshot_and_cursor_coordinates(&client_output, &mut small_grid);
                }
                if let Some(client_output) = rendered_clients.remove(&large) {
                    take_snapshot_and_cursor_coordinates(&client_output, &mut large_grid);
                }
            }
        }
        let expected = pane_cursor(&mut screen, followed);
        assert_eq!(
            large_grid.cursor_coordinates(),
            Some((expected.0, expected.1, true)),
            "the larger watcher never crops, so its cursor is right in every frame ({bytes:?})"
        );
    }
    assert_eq!(
        small_grid.cursor_coordinates(),
        Some((2, 2, true)),
        "and the smaller one has its cursor back"
    );
}

#[test]
fn watcher_departure_preserves_a_live_source_or_another_watchers_source() {
    for live in [false, true] {
        let source = ClientId::new(1);
        let watcher = ClientId::new(2);
        let remaining = ClientId::new(3);
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
        new_tab(&mut screen, 1, TabId::new(0));
        screen.set_followed_client(source).unwrap();
        screen.add_watcher_client(watcher).unwrap();
        if !live {
            screen.add_watcher_client(remaining).unwrap();
            screen.remove_client(source).unwrap();
        }
        screen.remove_watcher_client(watcher);
        assert_eq!(screen.followed_client_id, Some(source));
        assert_eq!(screen.watched_tab_id(), (!live).then_some(TabId::new(0)));
    }
}

#[test]
fn failed_attach_preserves_retained_plugin_state_until_successful_commit() {
    use crate::screen::thread_main::PendingEvents;
    let size = Size { cols: 80, rows: 24 };
    let attaching_size = Size { cols: 79, rows: 24 };
    let source = ClientId::new(1);
    let watcher = ClientId::new(2);
    let plugin = PLUGIN_2;
    for returning in [source, ClientId::new(3)] {
        for during_reflow in [false, true] {
            let (mut screen, _capture) = create_new_screen_with_theme_capture(size);
            new_tab(&mut screen, 1, TabId::new(0));
            new_tab(&mut screen, 2, TabId::new(1));
            add_pane_to_tab(&mut screen, TabId::new(1), PaneId::Plugin(plugin), false);
            screen.set_followed_client(source).unwrap();
            screen.add_watcher_client(watcher).unwrap();
            screen
                .tabs
                .get_mut(&TabId::new(1))
                .unwrap()
                .handle_plugin_bytes(plugin, source, b"retained-content".to_vec())
                .unwrap();
            screen.show_plugin_cursor(plugin, source, Some((1, 1)));
            screen.remove_client(source).unwrap();
            let receiver = screen_wired_to_server(&mut screen);
            let mut grid = grid_for_render_snapshot(size);
            watcher_frame(&mut screen, &receiver, watcher, &mut grid);
            let mut pending = PendingEvents::default();
            pending.waiting_for_pane.insert(
                PaneId::Plugin(crate::PluginId::new(99)),
                vec![
                    ScreenInstruction::PluginBytes(vec![PluginRenderAsset::new(
                        plugin,
                        source,
                        b"queued".to_vec(),
                    )]),
                    ScreenInstruction::ShowPluginCursor(plugin, source, Some((1, 1))),
                ],
            );
            // Break the active tab's registration, or the other tab's subsequent reflow.
            let failing_tab = TabId::new(if during_reflow { 0 } else { 1 });
            let tab = screen.tabs.get_mut(&failing_tab).unwrap();
            let saved_senders = tab.senders.clone();
            tab.senders.should_silently_fail = false;
            tab.senders.to_pty_writer = None;
            let mut traces = HashMap::new();
            screen
                .attach_client(
                    returning,
                    attaching_size,
                    None,
                    None,
                    &mut traces,
                    &mut pending,
                )
                .unwrap();
            screen.tabs.get_mut(&failing_tab).unwrap().senders = saved_senders;
            if during_reflow {
                assert_eq!(
                    screen.tabs[&TabId::new(0)].get_display_area(),
                    attaching_size
                );
            }
            assert!(!screen.connected_clients.borrow().contains(&returning));
            assert_eq!(screen.followed_client_id, Some(source));
            assert_eq!(screen.departed_followed_tab_id, Some(TabId::new(1)));
            assert_eq!(pending.waiting_for_pane.len(), 1);
            let pane = screen.tabs[&TabId::new(1)]
                .get_pane_with_id(PaneId::Plugin(plugin))
                .unwrap();
            assert_eq!(pane.cursor_coordinates(Some(source)), Some((1, 1, true)));
            let pane = screen.tabs[&TabId::new(1)]
                .get_pane_with_id(PaneId::Plugin(plugin))
                .unwrap();
            assert!(pane
                .pane_contents(Some(source), false, None)
                .viewport
                .concat()
                .contains("retained-content"));
            watcher_frame(&mut screen, &receiver, watcher, &mut grid);
            assert!(format!("{grid:?}").contains("retained-content"));
            screen
                .attach_client(returning, size, None, None, &mut traces, &mut pending)
                .unwrap();
            assert_eq!(screen.followed_client_id, Some(returning));
            assert_eq!(screen.departed_followed_tab_id, None);
            assert!(pending.waiting_for_pane.is_empty());
            let pane = screen.tabs[&TabId::new(1)]
                .get_pane_with_id(PaneId::Plugin(plugin))
                .unwrap();
            assert_eq!(
                pane.cursor_coordinates(Some(source)),
                (returning == source).then_some((1, 1, true))
            );
            assert_eq!(
                pane.pane_contents(Some(source), false, None)
                    .viewport
                    .concat()
                    .contains("retained-content"),
                returning == source
            );
        }
    }
}

#[test]
fn failed_new_tab_registration_rolls_back_without_disconnecting() {
    let client = ClientId::new(1);
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    let receiver = screen_wired_to_server(&mut screen);
    screen
        .new_tab(TabId::new(0), (vec![], vec![]), None, Some(client))
        .unwrap();
    // Layout can complete at the original size, then registration must reflow it.
    screen
        .set_client_size(client, Size { cols: 79, rows: 24 })
        .unwrap();
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.senders.should_silently_fail = false;
    tab.senders.to_pty_writer = None;
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(1), None)],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            client,
            None,
        )
        .unwrap();
    assert!(screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
        .is_some());
    assert!(!screen.connected_clients.borrow().contains(&client));
    assert!(!screen.active_tab_ids.contains_key(&client));
    assert!(!screen.tab_history.contains_key(&client));
    assert!(!screen.client_sizes.contains_key(&client));
    assert!(screen.tabs[&TabId::new(0)].has_no_connected_clients());
    assert_eq!(
        receiver
            .try_iter()
            .filter(|(instruction, _)| matches!(
                instruction, ServerInstruction::ClientExit(id, None) if *id == client
            ))
            .count(),
        0
    );
}

#[test]
fn removal_finishes_after_follow_render_visibility_or_resize_failure() {
    let client = ClientId::new(1);
    let remaining = ClientId::new(2);
    let size = Size { cols: 80, rows: 24 };
    for failure in ["follow", "visibility", "resize"] {
        let (mut screen, _capture) = create_new_screen_with_theme_capture(size);
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        screen.set_followed_client(client).unwrap();
        screen.add_watcher_client(ClientId::new(3)).unwrap();
        if failure != "visibility" {
            screen.add_client(remaining).unwrap();
        }
        add_pane_to_tab(&mut screen, TabId::new(1), PaneId::Plugin(PLUGIN_2), false);
        screen
            .tabs
            .get_mut(&TabId::new(1))
            .unwrap()
            .handle_plugin_bytes(PLUGIN_2, client, b"retained".to_vec())
            .unwrap();
        screen.show_plugin_cursor(PLUGIN_2, client, Some((1, 1)));
        screen.nested_via_client_id = Some(client);
        screen.nested_ancestry = vec!["parent".into()];
        screen.host_descended = true;
        screen
            .mode_info
            .insert(client, screen.default_mode_info.clone());
        screen
            .kitty_host_capabilities
            .borrow_mut()
            .insert(client, true);
        screen
            .sixel_host_capabilities
            .borrow_mut()
            .insert(client, true);
        screen
            .synchronized_output_capabilities
            .borrow_mut()
            .insert(client, true);
        match failure {
            "follow" => {
                // Force the follow switch's render to publish a changed pane-tabs list.
                screen
                    .reported_pane_tabs
                    .push(yinz_types::pane_tabs::PaneTabsInfo {
                        id: yinz_types::pane_tabs::PaneTabsId(1),
                        pane_id: PaneId::Terminal(TerminalId::new(1)),
                        tabs: vec![],
                        active: 0,
                        bar_focused: vec![],
                    });
                screen.bus.senders.should_silently_fail = false;
                screen.bus.senders.to_plugin = None;
                // Later cleanup also fails: keep the render error, but attempt it all.
                screen
                    .set_client_size(remaining, Size { cols: 90, rows: 24 })
                    .unwrap();
                for tab in screen.tabs.values_mut() {
                    tab.senders.should_silently_fail = false;
                    tab.senders.to_plugin = None;
                    tab.senders.to_pty_writer = None;
                }
            },
            "visibility" => {
                for tab in screen.tabs.values_mut() {
                    tab.senders.should_silently_fail = false;
                    tab.senders.to_plugin = None;
                }
            },
            "resize" => {
                screen
                    .set_client_size(remaining, Size { cols: 90, rows: 24 })
                    .unwrap();
                let tab = screen.tabs.get_mut(&TabId::new(1)).unwrap();
                tab.senders.should_silently_fail = false;
                tab.senders.to_pty_writer = None;
            },
            _ => unreachable!(),
        }
        let error = screen.remove_client(client).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains(match failure {
                "follow" => "failed to report pane tabs",
                "visibility" => "failed to set visibility",
                _ => "failed to recompute size",
            }),
            "{failure}: {error}"
        );
        assert!(!screen.connected_clients.borrow().contains(&client));
        assert!(!screen.active_tab_ids.contains_key(&client));
        assert!(!screen.tab_history.contains_key(&client));
        assert!(!screen.client_sizes.contains_key(&client));
        assert!(!screen.mode_info.contains_key(&client));
        assert!(!screen
            .kitty_host_capabilities
            .borrow()
            .contains_key(&client));
        assert!(!screen
            .sixel_host_capabilities
            .borrow()
            .contains_key(&client));
        assert!(!screen
            .synchronized_output_capabilities
            .borrow()
            .contains_key(&client));
        let pane = screen.tabs[&TabId::new(1)]
            .get_pane_with_id(PaneId::Plugin(PLUGIN_2))
            .unwrap();
        assert_eq!(
            pane.cursor_coordinates(Some(client)),
            (failure == "visibility").then_some((1, 1, true))
        );
        assert_eq!(
            pane.pane_contents(Some(client), false, None)
                .viewport
                .concat()
                .contains("retained"),
            failure == "visibility"
        );
        assert_eq!(screen.nested_via_client_id, None);
        assert!(screen.nested_ancestry.is_empty());
        assert!(!screen.host_descended);
        assert!(screen.host_descend_keys.is_empty());
        for tab in screen.tabs.values() {
            assert!(!tab.has_connected_client(&client));
        }
        assert_eq!(
            screen.followed_client_id,
            Some(if failure == "visibility" {
                client
            } else {
                remaining
            })
        );
    }
}

#[test]
fn reattach_registers_plugins_before_updates_and_preserves_immediate_output() {
    use crate::screen::thread_main::PendingEvents;
    let size = Size { cols: 80, rows: 24 };
    let source = ClientId::new(1);
    for returning in [source, ClientId::new(3)] {
        let (mut screen, capture) = create_new_screen_with_theme_capture(size);
        new_tab(&mut screen, 1, TabId::new(0));
        add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(PLUGIN_2), false);
        screen.set_followed_client(source).unwrap();
        screen.add_watcher_client(ClientId::new(2)).unwrap();
        screen.remove_client(source).unwrap();
        capture.drain_plugin_events();
        let (reply_tx, reply_rx) = channels::unbounded();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (first, _) = capture.plugin_rx.recv().unwrap();
                assert!(matches!(first, PluginInstruction::AddClient(id) if id == returning));
                // Plugin replies queue while the screen is still handling attachment.
                reply_tx
                    .send(ScreenInstruction::PluginBytes(vec![
                        PluginRenderAsset::new(PLUGIN_2, returning, b"immediate-output".to_vec()),
                    ]))
                    .unwrap();
                reply_tx
                    .send(ScreenInstruction::ShowPluginCursor(
                        PLUGIN_2,
                        returning,
                        Some((1, 1)),
                    ))
                    .unwrap();
            });
            screen
                .attach_client(
                    returning,
                    size,
                    None,
                    None,
                    &mut HashMap::new(),
                    &mut PendingEvents::default(),
                )
                .unwrap();
        });
        assert!(capture
            .drain_plugin_updates()
            .iter()
            .any(|(_, client, _)| *client == Some(returning)));
        for event in reply_rx.try_iter() {
            match event {
                ScreenInstruction::PluginBytes(assets) => {
                    assert!(screen
                        .handle_plugin_render_assets(assets)
                        .unwrap()
                        .is_empty());
                },
                ScreenInstruction::ShowPluginCursor(plugin, client, cursor) => {
                    assert!(screen.show_plugin_cursor(plugin, client, cursor));
                },
                _ => unreachable!(),
            }
        }
        let pane = screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Plugin(PLUGIN_2))
            .unwrap();
        assert!(pane
            .pane_contents(Some(returning), false, None)
            .viewport
            .concat()
            .contains("immediate-output"));
        assert_eq!(pane.cursor_coordinates(Some(returning)), Some((1, 1, true)));
    }
}

#[test]
fn failed_plugin_registration_rejects_attachment_once() {
    use crate::screen::thread_main::PendingEvents;
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let client = ClientId::new(2);
    let receiver = screen_wired_to_server(&mut screen);
    screen.bus.senders.should_silently_fail = false;
    screen.bus.senders.to_plugin = None;
    let mut traces = HashMap::new();
    screen
        .attach_client(
            client,
            size,
            None,
            None,
            &mut traces,
            &mut PendingEvents::default(),
        )
        .unwrap();
    assert!(!screen.connected_clients.borrow().contains(&client));
    assert!(!screen.client_sizes.contains_key(&client));
    assert!(traces.is_empty());
    assert_eq!(receiver.try_iter().filter(|(instruction, _)| matches!(instruction, ServerInstruction::ClientExit(id, None) if *id == client)).count(), 1);
}

#[test]
fn registration_preserves_reused_source_state_in_every_tab() {
    use crate::screen::thread_main::PendingEvents;
    let size = Size { cols: 80, rows: 24 };
    let source = ClientId::new(1);
    for returning in [source, ClientId::new(3)] {
        for registration in ["direct", "attach", "layout"] {
            let (mut screen, capture) = create_new_screen_with_theme_capture(size);
            for id in 0..2 {
                let tab_id = TabId::new(id);
                let plugin = crate::PluginId::new(id as u32 + 10);
                new_tab(&mut screen, id as u32 + 1, tab_id);
                add_pane_to_tab(&mut screen, tab_id, PaneId::Plugin(plugin), false);
                screen
                    .tabs
                    .get_mut(&tab_id)
                    .unwrap()
                    .handle_plugin_bytes(plugin, source, b"retained-content".to_vec())
                    .unwrap();
                screen.show_plugin_cursor(plugin, source, Some((1, 1)));
            }
            screen.set_followed_client(source).unwrap();
            screen.add_watcher_client(ClientId::new(2)).unwrap();
            screen.remove_client(source).unwrap();
            for id in 0..2 {
                screen
                    .tabs
                    .get_mut(&TabId::new(id))
                    .unwrap()
                    .pending_agent_escapes
                    .insert(
                        (source, PaneId::Plugin(crate::PluginId::new(id as u32 + 10))),
                        std::time::Instant::now(),
                    );
            }
            capture.drain_plugin_events();
            let mut pending = PendingEvents::default();
            let pending_pane = PaneId::Plugin(crate::PluginId::new(99));
            pending.waiting_for_pane.insert(
                pending_pane,
                vec![
                    ScreenInstruction::PluginBytes(vec![PluginRenderAsset::new(
                        crate::PluginId::new(99),
                        source,
                        b"stale-output".to_vec(),
                    )]),
                    ScreenInstruction::ShowPluginCursor(
                        crate::PluginId::new(99),
                        source,
                        Some((2, 2)),
                    ),
                ],
            );
            match registration {
                "direct" => screen.add_client(returning).unwrap(),
                "attach" => screen
                    .attach_client(
                        returning,
                        size,
                        None,
                        None,
                        &mut HashMap::new(),
                        &mut pending,
                    )
                    .unwrap(),
                "layout" => {
                    let tab_id = TabId::new(2);
                    screen
                        .new_tab(tab_id, (vec![], vec![]), None, Some(returning))
                        .unwrap();
                    screen
                        .land_layout(
                            TiledPaneLayout::default(),
                            vec![],
                            vec![(TerminalId::new(3), None)],
                            vec![],
                            vec![],
                            HashMap::new(),
                            tab_id,
                            true,
                            returning,
                            None,
                            None,
                            &mut pending,
                        )
                        .unwrap();
                },
                _ => unreachable!(),
            }
            assert_eq!(screen.followed_client_id, Some(returning), "{registration}");
            assert_eq!(screen.departed_followed_tab_id, None, "{registration}");
            assert!(screen.connected_clients.borrow().contains(&returning));
            let plugin_events: Vec<_> = capture
                .plugin_rx
                .try_iter()
                .map(|(event, _)| event)
                .collect();
            assert_eq!(plugin_events.iter().filter(|event|
                matches!(event, PluginInstruction::AddClient(id) if *id == returning)
            ).count(), usize::from(registration != "layout"), "{registration}");
            if registration != "layout" {
                assert!(
                    matches!(plugin_events.first(), Some(PluginInstruction::AddClient(id)) if *id == returning)
                );
            }
            if registration != "direct" {
                assert!(pending.waiting_for_pane.is_empty(), "{registration}");
            }
            for id in 0..2 {
                assert_eq!(
                    screen.tabs[&TabId::new(id)]
                        .pending_agent_escapes
                        .contains_key(&(
                            source,
                            PaneId::Plugin(crate::PluginId::new(id as u32 + 10))
                        )),
                    returning == source,
                    "{registration}, tab {id}"
                );
                let pane = screen.tabs[&TabId::new(id)]
                    .get_pane_with_id(PaneId::Plugin(crate::PluginId::new(id as u32 + 10)))
                    .unwrap();
                assert_eq!(
                    pane.cursor_coordinates(Some(source)),
                    (returning == source).then_some((1, 1, true)),
                    "{registration}, tab {id}"
                );
                assert_eq!(
                    pane.pane_contents(Some(source), false, None)
                        .viewport
                        .concat()
                        .contains("retained-content"),
                    returning == source,
                    "{registration}, tab {id}"
                );
            }
        }
    }
}

#[test]
fn failed_layout_registration_preserves_departed_source_and_pending_output() {
    use crate::screen::thread_main::PendingEvents;
    let source = ClientId::new(1);
    for returning in [source, ClientId::new(3)] {
        let (mut screen, _capture) =
            create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(PLUGIN_2), false);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_plugin_bytes(PLUGIN_2, source, b"retained-content".to_vec())
            .unwrap();
        screen.show_plugin_cursor(PLUGIN_2, source, Some((1, 1)));
        screen.set_followed_client(source).unwrap();
        screen.add_watcher_client(ClientId::new(2)).unwrap();
        screen.remove_client(source).unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        let tab_id = TabId::new(1);
        screen
            .new_tab(tab_id, (vec![], vec![]), None, Some(returning))
            .unwrap();
        let mut pending = PendingEvents::default();
        pending.waiting_for_pane.insert(
            PaneId::Plugin(PLUGIN_2),
            vec![ScreenInstruction::ShowPluginCursor(
                PLUGIN_2,
                source,
                Some((2, 2)),
            )],
        );
        // The committed layout survives a failure in screen registration's reflow.
        screen
            .set_client_size(returning, Size { cols: 79, rows: 24 })
            .unwrap();
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.senders.should_silently_fail = false;
        tab.senders.to_pty_writer = None;
        screen
            .land_layout(
                TiledPaneLayout::default(),
                vec![],
                vec![(TerminalId::new(2), None)],
                vec![],
                vec![],
                HashMap::new(),
                tab_id,
                false,
                returning,
                None,
                None,
                &mut pending,
            )
            .unwrap();
        assert!(!screen.connected_clients.borrow().contains(&returning));
        assert!(!screen.active_tab_ids.contains_key(&returning));
        assert_eq!(screen.followed_client_id, Some(source));
        assert_eq!(screen.departed_followed_tab_id, Some(TabId::new(0)));
        assert_eq!(pending.waiting_for_pane.len(), 1);
        let pane = screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Plugin(PLUGIN_2))
            .unwrap();
        assert_eq!(pane.cursor_coordinates(Some(source)), Some((1, 1, true)));
        assert!(pane
            .pane_contents(Some(source), false, None)
            .viewport
            .concat()
            .contains("retained-content"));
        assert_eq!(receiver.try_iter().filter(|(instruction, _)|
            matches!(instruction, ServerInstruction::ClientExit(id, None) if *id == returning)
        ).count(), 0);
    }
}

#[test]
fn unattached_disconnect_preserves_pane_state_and_clean_subscriber_snapshots() {
    let source = ClientId::new(1);
    for connection in [source, ClientId::new(99)] {
        let (mut screen, capture) =
            create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.add_watcher_client(ClientId::new(2)).unwrap();
        screen.remove_client(source).unwrap();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.set_guest_modal_on_pane(pane_id, &[connection]);
        tab.pending_agent_escapes
            .insert((connection, pane_id), std::time::Instant::now());
        screen
            .mode_info
            .insert(connection, screen.default_mode_info.clone());
        screen.pane_subscriber_snapshots_dirty = false;
        capture.drain_plugin_events();

        screen.remove_client(connection).unwrap();

        assert!(!screen.pane_subscriber_snapshots_dirty);
        assert!(!screen.mode_info.contains_key(&connection));
        let tab = &screen.tabs[&TabId::new(0)];
        assert!(tab.pane_has_guest_modal_for_client(pane_id, connection));
        assert!(tab
            .pending_agent_escapes
            .contains_key(&(connection, pane_id)));
        assert!(capture.plugin_rx.try_iter().next().is_none());
    }
}

#[test]
fn detached_layout_registers_only_with_the_screen() {
    let client = ClientId::new(3);
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.remove_client(ClientId::new(1)).unwrap();
    capture.drain_plugin_events();
    screen
        .new_tab(TabId::new(1), (vec![], vec![]), None, Some(client))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(2), None)],
            vec![],
            HashMap::new(),
            TabId::new(1),
            true,
            client,
            None,
        )
        .unwrap();
    assert!(screen.is_client_attached(client));
    assert_eq!(screen.followed_client_id, Some(client));
    assert!(screen.tabs[&TabId::new(1)]
        .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
        .is_some());
    assert!(!capture
        .plugin_rx
        .try_iter()
        .any(|(event, _)| matches!(event, PluginInstruction::AddClient(_))));
}

#[test]
fn retiring_a_followed_source_preserves_another_clients_hover() {
    use yinz_types::data::{HighlightLayer, HighlightStyle, RegexHighlight};
    for commit in [false, true] {
        let source = ClientId::new(1);
        let remaining = ClientId::new(3);
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
        new_tab(&mut screen, 1, TabId::new(0));
        add_pane_to_tab(
            &mut screen,
            TabId::new(0),
            PaneId::Terminal(TerminalId::new(2)),
            false,
        );
        screen.add_watcher_client(ClientId::new(2)).unwrap();
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let position = Position::new(5, 5);
        tab.add_client(remaining, None).unwrap();
        for client in [source, remaining] {
            tab.focus_pane_with_id(PaneId::Terminal(TerminalId::new(2)), false, false, client)
                .unwrap();
            tab.handle_mouse_event_with_passthrough(
                &MouseEvent::new_buttonless_motion(position),
                client,
                None,
            )
            .unwrap();
        }
        assert_eq!(tab.query_mouse_hover_pane_id().len(), 2);
        let pane = tab.get_pane_with_id_mut(pane_id).unwrap();
        pane.set_plugin_regex_highlights(
            crate::PluginId::new(1),
            vec![RegexHighlight {
                pattern: "test".into(),
                style: HighlightStyle::None,
                layer: HighlightLayer::Hint,
                context: Default::default(),
                on_hover: true,
                bold: false,
                italic: false,
                underline: false,
                tooltip_text: None,
            }],
            &Default::default(),
        );
        pane.set_hover_position(Some(position));
        screen.remove_client(source).unwrap();
        if commit {
            screen.add_client(remaining).unwrap();
        } else {
            screen.remove_watcher_client(ClientId::new(2));
        }
        let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
        assert!(!tab.query_mouse_hover_pane_id().contains_key(&source));
        assert_eq!(
            tab.query_mouse_hover_pane_id().get(&remaining),
            Some(&pane_id)
        );
        assert!(!tab
            .get_pane_with_id_mut(pane_id)
            .unwrap()
            .set_hover_position(Some(position)));
    }
}
