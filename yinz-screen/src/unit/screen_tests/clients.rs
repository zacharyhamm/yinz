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
        screen.remove_watcher_client(watcher);
        assert_eq!(screen.departed_followed_tab_id, Some(TabId::new(1)));
        screen.add_watcher_client(watcher).unwrap();
        watcher_frame(&mut screen, &receiver, watcher, &mut grid);
        assert!(format!("{grid:?}").contains("incremental-update"));

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
fn retained_plugin_state_is_freed_when_following_ends_or_the_id_is_reused() {
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
            _ => screen.add_client(followed).unwrap(),
        }
        assert_eq!(
            screen.departed_followed_tab_id,
            (transition == 0).then_some(TabId::new(0))
        );
        assert!(!screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PaneId::Plugin(plugin))
            .unwrap()
            .pane_contents(Some(followed), false, None)
            .viewport
            .concat()
            .contains("departed-client"));
    }
}

#[test]
fn a_returning_watcher_can_render_after_the_last_watchers_state_was_freed() {
    let size = Size { cols: 80, rows: 24 };
    let followed = ClientId::new(1);
    let watcher = ClientId::new(2);
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.set_followed_client(followed).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.remove_client(followed).unwrap();
    screen.remove_watcher_client(watcher);

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
    assert!(bytes.contains("while-nobody-watched"), "{bytes:?}");
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
    screen
        .render_watcher_output(&output, followed, None)
        .unwrap();
    assert!(screen.watcher_clients[&watcher].cursor_was_cropped);
    assert!(!screen.watcher_clients[&watcher].should_force_render());
    assert!(receiver.try_recv().is_err());

    // Cursor-only output delivers a stream and updates crop state without pane registration: a
    // pre- or post-VTE instruction puts its client in the frame, which is what lets a held-back
    // tab still deliver its bell and OSC 7 (see `render_pacing`).
    output.add_post_vte_instruction_to_client(followed, "\x1b[?25l");
    output.set_cursor_coordinates(followed, 2, 2);
    screen
        .render_watcher_output(&output, followed, None)
        .unwrap();
    assert!(!screen.watcher_clients[&watcher].cursor_was_cropped);
    assert!(!screen.watcher_clients[&watcher].should_force_render());
    assert!(receiver.try_recv().is_ok());
    output.set_cursor_coordinates(followed, 60, 2);
    screen
        .render_watcher_output(&output, followed, None)
        .unwrap();
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
