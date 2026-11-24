#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const PLUGIN_77: yinz_types::internal_ids::PluginId = yinz_types::internal_ids::PluginId::new(77);

use super::*;
use yinz_types::internal_ids::TabId;

#[test]
fn kitty_probe_before_first_layout_reaches_plugins() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    // Startup records the client's size before spawning panes. It joins connected_clients
    // only when the first layout lands, which can take longer than the host's probe reply.
    assert!(!screen.connected_clients_contains(&CLIENT_1));
    screen.update_kitty_graphics_support(CLIENT_1, true);
    assert_eq!(
        screen.core.kitty_host_capabilities.borrow().get(&CLIENT_1),
        Some(&true)
    );

    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(&mut screen, TabId::new(0), PaneId::Plugin(PLUGIN_77), false);
    capture.drain_plugin_updates();
    screen
        .get_active_tab_mut(CLIENT_1)
        .expect("TEST")
        .update_input_modes()
        .expect("TEST");

    assert!(capture
        .drain_plugin_updates()
        .into_iter()
        .any(|(plugin, client, event)| matches!(
            (plugin, client, event),
            (Some(PLUGIN_77), Some(CLIENT_1), Event::ModeUpdate(mode_info))
                if mode_info.capabilities.kitty_graphics
        )));
}

#[test]
fn kitty_query_replies_ok_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);
    assert_eq!(
        screen
            .core
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&true)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert_eq!(
        active_pane.terminal_mut().unwrap().drain_messages_to_pty(),
        vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]
    );
}

#[test]
fn kitty_query_replies_enotsupported_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.update_kitty_graphics_support(crate::ClientId::new(1), false);
    assert_eq!(
        screen
            .core
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.terminal_mut().unwrap().drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn kitty_query_is_ignored_when_the_protocol_is_disabled_in_the_config() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen_with_kitty_graphics(size, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);
    assert_eq!(
        screen
            .core
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&false),
        "a capable host must still be recorded as incapable when the protocol is disabled"
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert!(
        active_pane
            .terminal_mut()
            .unwrap()
            .drain_messages_to_pty()
            .is_empty(),
        "a disabled protocol must not reply to queries at all"
    );
}

#[test]
fn kitty_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen.update_kitty_graphics_support(crate::ClientId::new(1), false);
    screen.update_kitty_graphics_support(crate::ClientId::new(2), true);
    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.terminal_mut().unwrap().drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn plugins_receive_kitty_support_per_client() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(77)),
        false,
    );
    capture.drain_plugin_updates();

    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);
    screen.update_kitty_graphics_support(crate::ClientId::new(2), false);

    let capabilities: BTreeMap<ClientId, bool> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(PLUGIN_77), Some(client_id), Event::ModeUpdate(mode_info)) => {
                    Some((client_id, mode_info.capabilities.kitty_graphics))
                },
                _ => None,
            },
        )
        .collect();
    assert_eq!(
        capabilities,
        BTreeMap::from([(ClientId::new(1), true), (ClientId::new(2), false)])
    );
}

#[test]
fn plugins_receive_kitty_support_after_a_client_id_is_reused() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(77)),
        false,
    );

    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);
    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    capture.drain_plugin_updates();

    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);

    assert!(capture.drain_plugin_updates().into_iter().any(
        |(plugin_id, client_id, event)| matches!(
            (plugin_id, client_id, event),
            (Some(PLUGIN_77), Some(CLIENT_1), Event::ModeUpdate(mode_info))
                if mode_info.capabilities.kitty_graphics
        )
    ));
}

/// The probe reply can outlive its client: the deadline that resolves it fires on the client's
/// own thread, so a `KittyGraphicsSupport` can reach Screen after `RemoveClient`. Recording it
/// would resurrect exactly the per-client entries `remove_client` deletes.
#[test]
fn a_capability_reply_for_a_departed_client_is_ignored() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen.update_kitty_graphics_support(crate::ClientId::new(1), false);
    screen.remove_client(crate::ClientId::new(2)).expect("TEST");

    screen.update_kitty_graphics_support(crate::ClientId::new(2), true);

    assert_eq!(
        screen
            .core
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        None,
        "a departed client must not be recorded"
    );
    assert!(
        !screen.core.mode_info.contains_key(&ClientId::new(2)),
        "a departed client must not come back as a phantom in the mode_info map"
    );
    // ...and the aggregate still reflects only the client that is actually here.
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.terminal_mut().unwrap().drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"), "{reply:?}");
}

/// Client ids are recycled. The kitty capability describes the terminal on the other end, not the
/// id, so a client attaching over a transport that cannot show images must not be told it can by
/// the kitty terminal that held the id before it - which is what put the hall-of-mirrors wand on
/// a mosh client's screen.
#[test]
fn a_reused_client_id_does_not_inherit_the_previous_clients_kitty_capability() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    add_pane_to_tab(
        &mut screen,
        TabId::new(0),
        PaneId::Plugin(crate::PluginId::new(77)),
        false,
    );

    screen.update_kitty_graphics_support(crate::ClientId::new(1), true);
    screen.remove_client(crate::ClientId::new(1)).expect("TEST");
    screen.add_client(crate::ClientId::new(1)).expect("TEST");
    capture.drain_plugin_updates();

    // What the server enqueues right after `AddClient`, and the point at which a stale entry
    // would be cloned and fanned out to every plugin.
    screen
        .change_mode(InputMode::Normal, None, crate::ClientId::new(1))
        .expect("TEST");

    let reported: Vec<bool> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(PLUGIN_77), Some(CLIENT_1), Event::ModeUpdate(mode_info)) => {
                    Some(mode_info.capabilities.kitty_graphics)
                },
                _ => None,
            },
        )
        .collect();
    assert!(!reported.is_empty(), "the new client was told nothing");
    assert!(
        reported.iter().all(|supported| !supported),
        "a recycled id inherited the last holder's capability: {reported:?}"
    );
}

#[test]
fn primary_da_advertises_sixel_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .update_sixel_support(crate::ClientId::new(1), true)
        .unwrap();
    assert_eq!(
        screen
            .core
            .sixel_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&true)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.terminal_mut().unwrap().drain_messages_to_pty(),
        vec![b"\x1b[?62;4;52c".to_vec()]
    );
}

#[test]
fn primary_da_omits_sixel_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .update_sixel_support(crate::ClientId::new(1), false)
        .unwrap();
    assert_eq!(
        screen
            .core
            .sixel_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.terminal_mut().unwrap().drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

#[test]
fn sixel_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(crate::ClientId::new(2)).expect("TEST");
    screen
        .update_sixel_support(crate::ClientId::new(1), false)
        .unwrap();
    screen
        .update_sixel_support(crate::ClientId::new(2), true)
        .unwrap();
    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.terminal_mut().unwrap().drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

fn resize_pty_pixel_dimensions(
    instruction: &PtyWriteInstruction,
) -> Option<(u32, Option<u16>, Option<u16>)> {
    match instruction {
        PtyWriteInstruction::ResizePty(terminal_id, _cols, _rows, width, height) => {
            Some((terminal_id.get(), *width, *height))
        },
        _ => None,
    }
}

/// The terminal ids whose latest `ResizePty` carried pixel dimensions, and those whose latest
/// one did not.
fn terminal_ids_by_pixel_dimensions(
    instructions: &[PtyWriteInstruction],
) -> (HashSet<u32>, HashSet<u32>) {
    let mut latest: HashMap<u32, bool> = HashMap::new();
    for (terminal_id, width, height) in instructions.iter().filter_map(resize_pty_pixel_dimensions)
    {
        latest.insert(terminal_id, width.is_some() && height.is_some());
    }
    let with_pixels = latest
        .iter()
        .filter(|(_, has_pixels)| **has_pixels)
        .map(|(terminal_id, _)| *terminal_id)
        .collect();
    let without_pixels = latest
        .iter()
        .filter(|(_, has_pixels)| !**has_pixels)
        .map(|(terminal_id, _)| *terminal_id)
        .collect();
    (with_pixels, without_pixels)
}

#[test]
fn resizing_a_larger_client_updates_pty_pixels_on_an_unchanged_surface() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let larger_client = ClientId::new(2);
    screen.add_client(larger_client).unwrap();
    screen
        .set_client_size(
            larger_client,
            Size {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();
    let larger_report = PixelDimensions {
        character_cell_size: Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
        ..Default::default()
    };
    for (client, report) in [
        (larger_client, larger_report),
        (
            CLIENT_1,
            PixelDimensions {
                character_cell_size: Some(SizeInPixels {
                    width: 8,
                    height: 16,
                }),
                ..Default::default()
            },
        ),
    ] {
        assert!(screen.update_pixel_dimensions(client, report));
        screen.resize_pty_all_panes().unwrap();
    }
    let mut sizes = HashMap::new();
    let mut latest_sizes = || {
        while let Ok((instruction, _)) = capture.pty_writer_rx.try_recv() {
            if let PtyWriteInstruction::ResizePty(id, cols, rows, width, height) = instruction {
                sizes.insert(id.get(), (cols, rows, width, height));
            }
        }
        sizes.clone()
    };
    assert_eq!(
        latest_sizes(),
        HashMap::from([
            (1, (80, 20, Some(640), Some(320))),
            (2, (80, 20, Some(640), Some(320))),
        ])
    );

    screen
        .set_client_size(
            larger_client,
            Size {
                cols: 120,
                rows: 40,
            },
        )
        .unwrap();
    screen.resize_to_clients().unwrap();
    assert_eq!(screen.surface_size(), size);
    let expected = HashMap::from([
        (1, (80, 20, Some(800), Some(400))),
        (2, (80, 20, Some(800), Some(400))),
    ]);
    assert_eq!(latest_sizes(), expected);

    assert!(!screen.update_pixel_dimensions(larger_client, larger_report));
    assert_eq!(latest_sizes(), expected);
}

#[test]
pub fn reported_pixel_dimensions_are_applied_to_existing_ptys() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );

    // The layout's panes are created - and their ptys sized - before the host has reported
    // its pixel dimensions, so those first resizes carry none.
    let ptys_sized_without_pixels: HashSet<u32> = received_pty_instructions.wait_until(
        "the layout's ptys to be sized without pixel dimensions",
        yinz_test_support::default_timeout(),
        |instructions| {
            let (with_pixels, without_pixels) = terminal_ids_by_pixel_dimensions(instructions);
            assert!(
                with_pixels.is_empty(),
                "no pty can have pixel dimensions before the host reports any: {with_pixels:?}"
            );
            (!without_pixels.is_empty()).then_some(without_pixels)
        },
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::TerminalPixelDimensions(
            mock_screen.main_client_id,
            PixelDimensions {
                text_area_grid_size: None,
                character_cell_size: Some(SizeInPixels {
                    height: 21,
                    width: 8,
                }),
                text_area_size: None,
            },
        ));

    received_pty_instructions.wait_until_described(
        "every existing pty to be resized again with the reported pixel dimensions",
        yinz_test_support::default_timeout(),
        |instructions| {
            let (with_pixels, _) = terminal_ids_by_pixel_dimensions(instructions);
            ptys_sized_without_pixels
                .is_subset(&with_pixels)
                .then_some(())
        },
        |instructions| {
            format!(
                "{} pty instruction(s); resizes: {:?}",
                instructions.len(),
                instructions
                    .iter()
                    .filter_map(resize_pty_pixel_dimensions)
                    .collect::<Vec<_>>()
            )
        },
    );
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
}

/// The synchronised-output capability is sent as the client attaches, and takes the same route
/// through the server's main loop as `AddClient` does - so it can still be recorded for a client
/// Screen has not caught up with, and must survive the wait rather than be discarded.
#[test]
fn synchronized_output_support_reported_before_the_client_is_added_still_takes_effect() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));

    screen
        .update_synchronized_output_support(ClientId::new(2), true)
        .expect("TEST");
    screen.add_client(ClientId::new(2)).expect("TEST");
    assert_eq!(
        screen
            .core
            .synchronized_output_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        Some(&true),
        "the capability the client sent on its way in has to survive the wait"
    );
}

/// The other half: client ids are recycled, so a departed terminal's answer must not be left for
/// whoever inherits its id.
#[test]
fn synchronized_output_support_does_not_outlive_the_client_that_reported_it() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(2)).expect("TEST");
    screen
        .update_synchronized_output_support(ClientId::new(2), true)
        .expect("TEST");
    screen.remove_client(ClientId::new(2)).expect("TEST");
    assert_eq!(
        screen
            .core
            .synchronized_output_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        None,
        "the departed terminal's answer must not be left for the next one"
    );
}

/// A watcher reports its own synchronised-output support like any other client, but is never in
/// `connected_clients` - so it is only recorded at all because the write is unguarded, and only
/// dropped because `remove_watcher_client` says so.
#[test]
fn a_watchers_synchronized_output_support_is_recorded_and_dropped_with_the_watcher() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(1)).expect("TEST");
    screen.add_watcher_client(ClientId::new(2)).expect("TEST");

    screen
        .update_synchronized_output_support(ClientId::new(2), true)
        .expect("TEST");
    assert_eq!(
        screen
            .core
            .synchronized_output_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        Some(&true),
        "a watcher is not in `connected_clients`, and its answer must be kept all the same"
    );

    screen.remove_watcher_client(ClientId::new(2));
    assert_eq!(
        screen
            .core
            .synchronized_output_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        None,
        "and it goes when the watcher does, because ids are recycled"
    );
}

/// The watchers share one byte stream, hidden according to what every one of their hosts can do -
/// so one watcher's late answer changes the frame all of them get.
#[test]
fn a_watcher_reporting_synchronized_output_support_asks_for_a_full_frame() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(1)).expect("TEST");
    screen.add_watcher_client(ClientId::new(2)).expect("TEST");
    screen.add_watcher_client(ClientId::new(3)).expect("TEST");
    for watcher_state in screen.core.watcher_clients.values_mut() {
        watcher_state.clear_force_render();
    }

    screen
        .update_synchronized_output_support(ClientId::new(2), true)
        .expect("TEST");
    assert!(
        screen
            .core
            .watcher_clients
            .values()
            .all(|watcher_state| watcher_state.should_force_render()),
        "every watcher's next frame is decided by this answer, so every watcher is repainted"
    );
}

#[test]
fn late_kitty_capability_repaints_retained_images_for_each_client() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(ClientId::new(2)).unwrap();
    *screen.core.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 23,
        height: 43,
    });
    let receiver = screen_wired_to_server(&mut screen);
    screen
        .get_active_tab_mut(CLIENT_1)
        .unwrap()
        .get_active_pane_mut(CLIENT_1)
        .unwrap()
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(b"\x1b_Ga=T,q=2,f=24,s=2,v=2,m=0;////////////////\x1b\\".to_vec());
    screen.render_to_clients().unwrap();
    while receiver.try_recv().is_ok() {}
    for client in [CLIENT_1, ClientId::new(2)] {
        screen.update_kitty_graphics_support(client, true);
        screen.render_to_clients().unwrap();
        let mut output = String::new();
        while let Ok((instruction, _)) = receiver.try_recv() {
            if let ServerInstruction::Render(Some(mut clients)) = instruction {
                output.push_str(&clients.remove(&client).unwrap_or_default());
            }
        }
        assert!(
            output.contains("\x1b_Ga=t,"),
            "no transmission for {client}"
        );
        assert!(output.contains("\x1b_Ga=p,"), "no placement for {client}");
    }
}

#[test]
fn cell_pixel_geometry_is_per_client_and_updates_after_reports_resizes_and_reconnects() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    let second = ClientId::new(2);
    screen.add_client(second).unwrap();
    screen
        .set_client_size(
            second,
            Size {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();
    assert_eq!(screen.cell_pixel_size(CLIENT_1), None);
    assert_eq!(screen.cell_pixel_size(second), None);
    capture.drain_plugin_updates();
    let direct = SizeInPixels {
        width: 9,
        height: 18,
    };
    screen.update_pixel_dimensions(
        CLIENT_1,
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: Some(direct),
            text_area_size: None,
        },
    );
    screen.update_pixel_dimensions(
        second,
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                width: 1000,
                height: 600,
            }),
        },
    );
    assert_eq!(screen.cell_pixel_size(CLIENT_1), Some(direct));
    assert_eq!(
        screen.cell_pixel_size(second),
        Some(SizeInPixels {
            width: 10,
            height: 20
        })
    );
    screen
        .set_client_size(second, Size { cols: 50, rows: 20 })
        .unwrap();
    assert_eq!(
        screen.cell_pixel_size(second),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "a resize cannot pair the new grid with a stale area report"
    );
    screen.update_pixel_dimensions(
        second,
        PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 1000,
                height: 600,
            }),
            text_area_grid_size: Some(Size { cols: 50, rows: 20 }),
            character_cell_size: None,
        },
    );
    assert_eq!(
        screen.cell_pixel_size(second),
        Some(SizeInPixels {
            width: 20,
            height: 30
        })
    );
    let events: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter(|(_, _, event)| matches!(event, Event::CellPixelSizeChanged(_)))
        .collect();
    assert_eq!(
        events,
        vec![
            (
                None,
                Some(CLIENT_1),
                Event::CellPixelSizeChanged(Some(direct))
            ),
            (
                None,
                Some(second),
                Event::CellPixelSizeChanged(Some(SizeInPixels {
                    width: 10,
                    height: 20
                }))
            ),
            (
                None,
                Some(second),
                Event::CellPixelSizeChanged(Some(SizeInPixels {
                    width: 20,
                    height: 30
                }))
            ),
        ]
    );
    screen.report_cell_pixel_size(Some(PLUGIN_77), second);
    assert!(capture.drain_plugin_updates().contains(&(
        Some(PLUGIN_77),
        Some(second),
        Event::CellPixelSizeChanged(screen.cell_pixel_size(second))
    )));
    screen.update_pixel_dimensions(
        CLIENT_1,
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: Some(SizeInPixels {
                width: 0,
                height: 18,
            }),
            text_area_size: None,
        },
    );
    assert_eq!(screen.cell_pixel_size(CLIENT_1), Some(direct));
    assert!(capture.drain_plugin_updates().is_empty());
    screen.remove_client(second).unwrap();
    screen.add_client(second).unwrap();
    screen
        .set_client_size(
            second,
            Size {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();
    assert_eq!(screen.cell_pixel_size(second), None);
    assert!(capture.drain_plugin_updates().contains(&(
        None,
        Some(second),
        Event::CellPixelSizeChanged(None)
    )));
}

#[test]
fn cell_pixel_geometry_waits_for_client_size_and_never_invents_dimensions() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    let client = ClientId::new(9);
    screen.update_pixel_dimensions(
        client,
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 400,
            }),
        },
    );
    assert_eq!(screen.cell_pixel_size(client), None);
    screen
        .set_client_size(client, Size { cols: 0, rows: 20 })
        .unwrap();
    assert_eq!(screen.cell_pixel_size(client), None);
    screen
        .set_client_size(client, Size { cols: 80, rows: 20 })
        .unwrap();
    assert_eq!(
        screen.cell_pixel_size(client),
        Some(SizeInPixels {
            width: 10,
            height: 20
        })
    );
    screen
        .set_client_size(
            client,
            Size {
                cols: 801,
                rows: 20,
            },
        )
        .unwrap();
    assert_eq!(
        screen.cell_pixel_size(client),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "a later grid cannot reinterpret an old report"
    );
    let direct = SizeInPixels {
        width: 12,
        height: 24,
    };
    screen.update_pixel_dimensions(
        client,
        PixelDimensions {
            text_area_grid_size: None,
            character_cell_size: Some(direct),
            text_area_size: None,
        },
    );
    screen
        .set_client_size(
            client,
            Size {
                cols: 100,
                rows: 50,
            },
        )
        .unwrap();
    assert_eq!(screen.cell_pixel_size(client), Some(direct));
}

#[test]
fn paired_pixel_reports_are_independent_of_resize_message_order() {
    for report_first in [true, false] {
        let (mut screen, capture) =
            create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        screen.update_pixel_dimensions(
            CLIENT_1,
            PixelDimensions {
                text_area_size: Some(SizeInPixels {
                    width: 800,
                    height: 400,
                }),
                text_area_grid_size: Some(Size { cols: 80, rows: 20 }),
                character_cell_size: None,
            },
        );
        capture.drain_plugin_updates();
        let resized = Size {
            cols: 100,
            rows: 30,
        };
        let report = PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 1200,
                height: 720,
            }),
            text_area_grid_size: Some(resized),
            character_cell_size: None,
        };
        let cell = SizeInPixels {
            width: 12,
            height: 24,
        };
        if report_first {
            screen.update_pixel_dimensions(CLIENT_1, report);
            assert_eq!(screen.cell_pixel_size(CLIENT_1), Some(cell));
            screen.set_client_size(CLIENT_1, resized).unwrap();
        } else {
            screen.set_client_size(CLIENT_1, resized).unwrap();
            assert_eq!(
                screen.cell_pixel_size(CLIENT_1),
                Some(SizeInPixels {
                    width: 10,
                    height: 20
                })
            );
            assert!(capture.drain_plugin_updates().is_empty());
            screen.update_pixel_dimensions(CLIENT_1, report);
        }
        assert_eq!(screen.cell_pixel_size(CLIENT_1), Some(cell));
        assert_eq!(
            capture.drain_plugin_updates(),
            vec![(
                None,
                Some(CLIENT_1),
                Event::CellPixelSizeChanged(Some(cell))
            )]
        );
        let direct = SizeInPixels {
            width: 11,
            height: 23,
        };
        screen.update_pixel_dimensions(
            CLIENT_1,
            PixelDimensions {
                character_cell_size: Some(direct),
                ..Default::default()
            },
        );
        capture.drain_plugin_updates();
        screen.update_pixel_dimensions(CLIENT_1, report);
        assert_eq!(
            screen.cell_pixel_size(CLIENT_1),
            Some(direct),
            "a direct cell report takes precedence over area division"
        );
        assert!(capture.drain_plugin_updates().is_empty());
    }
}

#[test]
fn geometry_learned_before_registration_is_reported_after_add_client() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    capture.drain_plugin_events();
    let arriving = ClientId::new(2);
    let cell = SizeInPixels {
        width: 12,
        height: 24,
    };
    screen.update_pixel_dimensions(
        arriving,
        PixelDimensions {
            character_cell_size: Some(cell),
            ..Default::default()
        },
    );
    assert!(
        capture.drain_plugin_events().is_empty(),
        "pixel reports cannot reach plugins before client registration"
    );
    screen.add_client(arriving).unwrap();
    let events: Vec<_> = capture
        .plugin_rx
        .try_iter()
        .map(|(event, _)| event)
        .collect();
    assert!(matches!(events.first(), Some(PluginInstruction::AddClient(id)) if *id == arriving));
    assert!(events
        .iter()
        .any(|event| matches!(event, PluginInstruction::Update(updates)
        if updates.contains(&(None, Some(arriving), Event::CellPixelSizeChanged(Some(cell)))))));
}

#[test]
fn watcher_geometry_is_cleared_before_its_id_is_reused_by_an_interactive_client() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    let reused = ClientId::new(2);
    screen.add_watcher_client(reused).unwrap();
    screen.update_pixel_dimensions(
        reused,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                width: 12,
                height: 24,
            }),
            ..Default::default()
        },
    );
    assert!(screen.cell_pixel_size(reused).is_some());
    screen.remove_watcher_client(reused);
    assert_eq!(screen.cell_pixel_size(reused), None);
    assert!(!screen.core.client_pixel_dimensions.contains_key(&reused));
    capture.drain_plugin_updates();
    screen.add_client(reused).unwrap();
    screen
        .set_client_size(reused, Size { cols: 80, rows: 20 })
        .unwrap();
    assert_eq!(screen.cell_pixel_size(reused), None);
    assert!(capture.drain_plugin_updates().contains(&(
        None,
        Some(reused),
        Event::CellPixelSizeChanged(None)
    )));
}

#[test]
fn terminal_placement_reports_content_geometry_with_known_or_unknown_pixels() {
    use yinz_bus::pty_writer::PtyWriteInstruction;
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 40,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    let (tx, rx) = channels::unbounded();
    screen.core.bus.senders.to_pty_writer = Some(SenderWithContext::new(tx));
    let pane = screen.core.tabs[&TabId::new(0)]
        .get_active_pane(CLIENT_1)
        .unwrap();
    let PaneId::Terminal(id) = pane.pid() else {
        panic!("terminal");
    };
    let cols = pane.get_content_columns() as u16;
    let rows = pane.get_content_rows() as u16;
    for cell in [
        None,
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    ] {
        *screen.core.character_cell_size.borrow_mut() = cell;
        let mut pending = HashMap::from([(id, 42)]);
        screen
            .confirm_terminal_placements(&mut pending, &Default::default())
            .unwrap();
        assert!(pending.is_empty());
        let (ack, _) = rx.recv().unwrap();
        let PtyWriteInstruction::TerminalPlaced {
            terminal_id,
            generation,
            placement: Some((size, tab_id)),
        } = ack
        else {
            panic!("expected dimensions");
        };
        assert_eq!((terminal_id, generation, tab_id), (id, 42, TabId::new(0)));
        assert_eq!((size.cols, size.rows), (cols, rows));
        assert_eq!(size.width_in_pixels, cell.map(|_| cols * 10));
        assert_eq!(size.height_in_pixels, cell.map(|_| rows * 20));
    }
}

#[test]
fn collapsed_stack_placement_has_usable_startup_geometry_without_changing_content() {
    use yinz_bus::pty_writer::PtyWriteInstruction;
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 40,
        },
        true,
        true,
    );
    *screen.core.stacked_pane_list.borrow_mut() = false;
    screen.core.pane_frame_style = PaneFrameStyle::Full;
    let tab_id = TabId::new(0);
    screen
        .new_tab(tab_id, (vec![], vec![]), None, Some(CLIENT_1))
        .unwrap();
    screen
        .apply_layout(
            yinz_config::kdl::layout::from_kdl(
                "layout { pane stacked=true { pane; pane; }; }",
                None,
                None,
                None,
            )
            .unwrap()
            .new_tab()
            .0,
            vec![],
            vec![(TerminalId::new(1), None), (TerminalId::new(2), None)],
            vec![],
            HashMap::new(),
            tab_id,
            true,
            CLIENT_1,
            None,
        )
        .unwrap();
    let pane_id = [TerminalId::new(1), TerminalId::new(2)]
        .into_iter()
        .find(|id| {
            screen.core.tabs[&tab_id]
                .get_pane_with_id(PaneId::Terminal(*id))
                .unwrap()
                .get_content_rows()
                == 0
        })
        .expect("a collapsed member with no content rows");
    let cols = screen.core.tabs[&tab_id]
        .get_pane_with_id(PaneId::Terminal(pane_id))
        .unwrap()
        .get_content_columns() as u16;
    let (tx, rx) = channels::unbounded();
    screen.core.bus.senders.to_pty_writer = Some(SenderWithContext::new(tx));
    for cell in [
        None,
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    ] {
        *screen.core.character_cell_size.borrow_mut() = cell;
        let mut pending = HashMap::from([(pane_id, 42)]);
        screen
            .confirm_terminal_placements(&mut pending, &Default::default())
            .unwrap();
        assert!(pending.is_empty());
        let (ack, _) = rx.try_recv().unwrap();
        let PtyWriteInstruction::TerminalPlaced {
            terminal_id,
            generation: 42,
            placement: Some((size, placed_tab)),
        } = ack
        else {
            panic!("expected startup dimensions");
        };
        assert_eq!((terminal_id, placed_tab), (pane_id, tab_id));
        assert_eq!((size.cols, size.rows), (cols, 1));
        assert_eq!(size.width_in_pixels, cell.map(|_| cols * 10));
        assert_eq!(size.height_in_pixels, cell.map(|_| 20));
        assert_eq!(
            screen.core.tabs[&tab_id]
                .get_pane_with_id(PaneId::Terminal(pane_id))
                .unwrap()
                .get_content_rows(),
            0
        );
    }
    screen
        .core
        .tabs
        .get_mut(&tab_id)
        .unwrap()
        .focus_pane_with_id(PaneId::Terminal(pane_id), false, false, CLIENT_1)
        .unwrap();
    assert!(
        screen.core.tabs[&tab_id]
            .get_pane_with_id(PaneId::Terminal(pane_id))
            .unwrap()
            .get_content_rows()
            > 0
    );
}

#[test]
fn cell_pixel_geometry_adjusted_metrics_update_without_grid_changes_for_each_client() {
    for report_first in [true, false] {
        let (mut screen, capture) =
            create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
        new_tab(&mut screen, 1, TabId::new(0));
        let second = ClientId::new(2);
        screen.add_client(second).unwrap();
        screen
            .set_client_size(second, Size { cols: 80, rows: 20 })
            .unwrap();
        for (client, cell) in [
            (
                CLIENT_1,
                SizeInPixels {
                    width: 12,
                    height: 25,
                },
            ),
            (
                second,
                SizeInPixels {
                    width: 16,
                    height: 30,
                },
            ),
        ] {
            for cell in [
                SizeInPixels {
                    width: 8,
                    height: 20,
                },
                cell,
            ] {
                let size = Size { cols: 80, rows: 20 };
                let report = PixelDimensions {
                    character_cell_size: Some(cell),
                    text_area_size: Some(SizeInPixels {
                        width: 80 * cell.width,
                        height: 20 * cell.height,
                    }),
                    text_area_grid_size: Some(size),
                };
                if report_first {
                    screen.update_pixel_dimensions(client, report);
                    screen.set_client_size(client, size).unwrap();
                } else {
                    screen.set_client_size(client, size).unwrap();
                    screen.update_pixel_dimensions(client, report);
                }
                assert_eq!(screen.cell_pixel_size(client), Some(cell));
                assert!(capture.drain_plugin_updates().contains(&(
                    None,
                    Some(client),
                    Event::CellPixelSizeChanged(Some(cell))
                )));
            }
        }
        assert_eq!(
            screen.cell_pixel_size(CLIENT_1),
            Some(SizeInPixels {
                width: 12,
                height: 25
            })
        );
        assert_eq!(
            screen.cell_pixel_size(second),
            Some(SizeInPixels {
                width: 16,
                height: 30
            })
        );
    }
}

#[test]
fn render_negotiation_legacy_toggle_preserves_sequence_until_detach() {
    use yinz_types::render_negotiation::{Decision, Offer, Request};
    let size = Size { cols: 80, rows: 24 };
    let (mut screen, _, receiver) = create_new_screen_with_capture(size, true, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let client = CLIENT_1;
    screen.legacy_render_support(client, true);
    screen.render_to_clients().unwrap();
    let high = screen.core.native_render_clients[&client].seq;
    assert!(high > 0);
    screen.legacy_render_support(client, false);
    screen.legacy_render_support(client, true);
    screen.render_to_clients().unwrap();
    assert!(receiver.try_iter().any(|(event, _)| {
        matches!(event, ServerInstruction::RenderFrames(frames) if frames.get(&client).is_some_and(|bytes| {
            let view = yinz_types::structured_render::decode(bytes).unwrap();
            view.header().seq == high + 1 && view.header().full_repaint()
        }))
    }));
    screen.legacy_render_support(client, false);
    screen.remove_client(client).unwrap();
    screen.add_client(client).unwrap();
    screen.set_client_size(client, size).unwrap();
    screen.legacy_render_support(client, false);
    screen
        .negotiate_render(client, Request::Offer(Offer::reader(None)))
        .unwrap();
    assert!(receiver.try_iter().any(|(event, _)| matches!(event, ServerInstruction::NativeRenderResult(id, Decision::Selected(_)) if id == client)));
}

#[test]
fn render_negotiation_profiles_survive_reload_withdraw_and_detach() {
    use yinz_types::render_negotiation::{Decision, Offer, Request};
    let size = Size { cols: 80, rows: 24 };
    let (mut screen, _, receiver) = create_new_screen_with_capture(size, true, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    let graphics = CLIENT_1;
    let text = ClientId::new(2);
    let ansi = ClientId::new(3);
    for id in [text, ansi] {
        screen.add_client(id).unwrap();
        screen.set_client_size(id, size).unwrap();
    }
    screen.update_kitty_graphics_support(ansi, true);
    screen.update_kitty_graphics_support(graphics, false);
    let mut limited = Offer::reader(None);
    limited.features = 0;
    screen
        .negotiate_render(text, Request::Offer(limited.clone()))
        .unwrap();
    screen
        .negotiate_render(graphics, Request::Offer(Offer::reader(None)))
        .unwrap();
    screen.render_to_clients().unwrap();
    let mut selected = std::collections::HashSet::new();
    for (event, _) in receiver.try_iter() {
        match event {
            ServerInstruction::NativeRenderResult(id, Decision::Selected(_)) => {
                selected.insert(id);
            },
            ServerInstruction::RenderFrames(frames) => {
                for (id, bytes) in frames {
                    assert!(selected.contains(&id));
                    let frame = yinz_types::structured_render::decode(&bytes).unwrap();
                    assert_eq!(frame.geometry().is_some(), id == graphics);
                }
            },
            _ => {},
        }
    }
    assert_eq!(selected.len(), 2);
    for enabled in [false, true] {
        let mut config = test_screen_config();
        config.support_kitty_graphics_protocol = enabled;
        screen
            .reconfigure(config, Default::default(), graphics)
            .unwrap();
        let caps = screen.core.kitty_host_capabilities.borrow();
        assert_eq!(
            (caps[&graphics], caps[&text], caps[&ansi]),
            (enabled, false, enabled)
        );
    }
    let high = screen.core.native_render_clients[&graphics].seq;
    screen
        .negotiate_render(graphics, Request::Withdraw)
        .unwrap();
    assert!(receiver.try_iter().any(|(event,_)| matches!(event,ServerInstruction::NativeRenderResult(id,Decision::Withdrawn(h)) if id == graphics && h == high)));
    screen.legacy_render_support(graphics, true);
    screen
        .negotiate_render(graphics, Request::Offer(limited))
        .unwrap();
    assert!(!screen.core.native_render_clients.contains_key(&graphics));
    assert!(screen.core.native_render_clients.contains_key(&text));
    assert!(!screen.core.kitty_host_capabilities.borrow()[&graphics]);
    screen.remove_client(graphics).unwrap();
    assert!(!screen.core.retired_native_clients.contains_key(&graphics));
}

#[test]
fn render_negotiation_deferred_image_retry_marks_open_panes_in_any_tab() {
    use yinz_types::render_negotiation::{Offer, Request};
    let size = Size { cols: 80, rows: 24 };
    let (mut screen, _, receiver) = create_new_screen_with_capture(size, true, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let client = CLIENT_1;
    screen
        .negotiate_render(client, Request::Offer(Offer::reader(None)))
        .unwrap();
    // Every negotiated viewer owns a budget, even without a shared root.
    let state = &screen.core.native_render_clients[&client];
    assert!(state.media_budget().is_some());
    screen.render_to_clients().unwrap();
    receiver.try_iter().for_each(drop);
    let [background, active] = [1, 2].map(|id| PaneId::Terminal(TerminalId::new(id)));
    let should_render = |screen: &mut Screen, pane: PaneId| {
        screen
            .core
            .tab_containing_pane_mut(pane)
            .and_then(|tab| tab.get_pane_with_id_mut(pane))
            .map(|pane| pane.should_render())
    };
    for pane in [background, active] {
        screen
            .core
            .tab_containing_pane_mut(pane)
            .and_then(|tab| tab.get_pane_with_id_mut(pane))
            .unwrap()
            .set_should_render(false);
    }
    // Nothing to retry, or only closed panes: no render is requested.
    assert!(!screen.retry_deferred_panes([]));
    assert!(!screen.retry_deferred_panes([PaneId::Terminal(TerminalId::new(99))]));
    assert_eq!(should_render(&mut screen, background), Some(false));
    // A pane in a tab the viewer is not looking at is still marked in its own tab.
    assert!(screen.retry_deferred_panes([background, PaneId::Terminal(TerminalId::new(99))]));
    assert_eq!(should_render(&mut screen, background), Some(true));
    assert_eq!(should_render(&mut screen, active), Some(false));
    // An ACK that releases nothing retries nothing and sends no frame.
    let seq = screen.core.native_render_clients[&client].seq;
    screen.acknowledge_native_frame(client, seq).unwrap();
    screen
        .acknowledge_native_frame(ClientId::new(9), seq)
        .unwrap();
    assert!(!receiver
        .try_iter()
        .any(|(event, _)| matches!(event, ServerInstruction::RenderFrames(_))));
    assert_eq!(should_render(&mut screen, active), Some(false));
}

#[test]
fn watcher_sixel_change_repaints_and_removal_forgets_the_answer() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    let watcher = ClientId::new(2);
    let followed = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.add_client(followed).unwrap();
    screen.update_sixel_support(followed, false).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    for supported in [true, false, true] {
        screen
            .core
            .watcher_clients
            .get_mut(&watcher)
            .unwrap()
            .clear_force_render();
        screen.update_sixel_support(watcher, supported).unwrap();
        assert!(screen.core.watcher_clients[&watcher].should_force_render());
        assert_eq!(
            screen.core.sixel_host_capabilities.borrow()[&watcher],
            supported
        );
        assert!(!screen.core.sixel_host_capabilities.borrow()[&followed]);
    }
    screen.remove_watcher_client(watcher);
    screen.add_watcher_client(watcher).unwrap();
    assert!(!screen.core.sixel_probe_answers.contains_key(&watcher));
    assert!(!screen
        .core
        .sixel_host_capabilities
        .borrow()
        .contains_key(&watcher));
}

#[test]
fn attachment_generation_superseded_reports_cannot_override_shared_colors() {
    use crate::screen::thread_main::{process_instruction, EventLoopState};
    use yinz_types::{colors::PaletteColor, render_negotiation::AttachmentGeneration};
    let (mut screen, _capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    let owners = screen.core.bus.senders.session.clone();
    owners.register_attachment(CLIENT_1);
    let old = ClientId::for_attachment(2, AttachmentGeneration([1; 16]));
    let new = ClientId::for_attachment(2, AttachmentGeneration([2; 16]));
    let reports = |owner, color: &str| {
        [
            ScreenInstruction::TerminalForegroundColor(owner, color.into()),
            ScreenInstruction::TerminalBackgroundColor(owner, color.into()),
            ScreenInstruction::TerminalColorRegisters(owner, vec![(17, color.into())]),
        ]
    };
    owners.register_attachment(old);
    let delayed = reports(old, "rgb:aaaa/bbbb/cccc");
    owners.register_attachment(new);
    let mut state = EventLoopState::default();
    for (owner, color, rgb) in [
        (new, "rgb:1111/2222/3333", (17, 34, 51)),
        (CLIENT_1, "rgb:4444/5555/6666", (68, 85, 102)),
    ] {
        for instruction in reports(owner, color).into_iter().chain(delayed.clone()) {
            assert!(process_instruction(&mut screen, &mut state, instruction)
                .unwrap()
                .is_continue());
        }
        let palette = screen.core.terminal_emulator_colors.borrow();
        assert_eq!(palette.fg, PaletteColor::Rgb(rgb));
        assert_eq!(palette.bg, PaletteColor::Rgb(rgb));
        assert_eq!(
            screen.core.terminal_emulator_color_codes.borrow()[&17],
            color
        );
    }
}

/// A zlib-compressed RGBA Kitty upload of a `side`-pixel square, placed at the cursor.
fn kitty_rgba_upload(id: u32, side: u32, shade: u8) -> Vec<u8> {
    use base64::Engine;
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(&[shade, 72, 13, 255].repeat((side * side) as usize))
        .unwrap();
    let payload = base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap());
    let mut bytes = Vec::new();
    let parts: Vec<_> = payload.as_bytes().chunks(4096).collect();
    for (index, part) in parts.iter().enumerate() {
        let more = u8::from(index + 1 < parts.len());
        let head = if index == 0 {
            format!("a=T,f=32,s={side},v={side},i={id},o=z,C=1,")
        } else {
            String::new()
        };
        bytes.extend_from_slice(format!("\x1b_G{head}q=2,m={more};").as_bytes());
        bytes.extend_from_slice(part);
        bytes.extend_from_slice(b"\x1b\\");
    }
    bytes
}

/// Chunk (id, offset, length) records, cancels and completed transfer ids in `client`'s
/// frames, and the last frame's sequence number.
#[derive(Debug, Default)]
struct Transfers {
    chunks: Vec<(u64, u32, usize)>,
    cancels: Vec<u64>,
    completed: Vec<u64>,
    last: Option<u64>,
}

fn drain_transfers(receiver: &ServerReceiver, client: ClientId) -> Transfers {
    use yinz_types::structured_render::{decode, GraphicsMedium, GraphicsRecord};
    let mut seen = Transfers::default();
    for (event, _) in receiver.try_iter() {
        let ServerInstruction::RenderFrames(frames) = event else {
            continue;
        };
        let Some(bytes) = frames.get(&client) else {
            continue;
        };
        let view = decode(bytes).unwrap();
        seen.last = Some(view.header().seq);
        for record in view.graphics() {
            match record {
                GraphicsRecord::TransferChunk {
                    transfer_id,
                    offset,
                    data,
                    ..
                } => seen.chunks.push((transfer_id, offset, data.len())),
                GraphicsRecord::TransferCancel { transfer_id } => seen.cancels.push(transfer_id),
                GraphicsRecord::Residency {
                    medium: GraphicsMedium::Transfer,
                    data,
                    ..
                } => seen
                    .completed
                    .push(u64::from_le_bytes(data.as_slice().try_into().unwrap())),
                GraphicsRecord::Residency { medium, .. } => {
                    assert_ne!(
                        medium,
                        GraphicsMedium::SharedFile,
                        "portable frame named a path"
                    )
                },
                _ => {},
            }
        }
    }
    seen
}

/// A forwarded viewer with a 16 MiB transfer half sent and a 9 MiB one waiting, unstarted,
/// for the ACK of the frame that carried the first half. The second image spans frames, so
/// it cannot ride whole past the closed window.
fn portable_viewer_mid_transfer() -> (Screen, ServerReceiver, u64) {
    use yinz_runtime::native_media::PORTABLE_FRAME_MEDIA_BYTES;
    use yinz_types::render_negotiation::{Offer, Request, CHUNKED};
    let size = Size { cols: 80, rows: 24 };
    let (mut screen, _, receiver) = create_new_screen_with_capture(size, true, true, true, false);
    new_tab(&mut screen, 1, TabId::new(0));
    *screen.core.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 8,
        height: 16,
    });
    screen
        .negotiate_render(CLIENT_1, Request::Offer(Offer::reader(None)))
        .unwrap();
    assert_ne!(
        screen.core.native_render_clients[&CLIENT_1].agreement.media & CHUNKED,
        0
    );
    screen.render_to_clients().unwrap();
    receiver.try_iter().for_each(drop);
    let pane = |screen: &mut Screen| {
        screen
            .get_active_tab_mut(CLIENT_1)
            .unwrap()
            .get_active_pane_mut(CLIENT_1)
            .unwrap()
            .terminal_mut()
            .unwrap()
            .handle_pty_bytes(b"\x1b[1;1H".to_vec());
    };
    pane(&mut screen);
    screen
        .get_active_tab_mut(CLIENT_1)
        .unwrap()
        .get_active_pane_mut(CLIENT_1)
        .unwrap()
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes(kitty_rgba_upload(1, 2048, 1));
    screen.render_to_clients().unwrap();
    let first = drain_transfers(&receiver, CLIENT_1);
    assert_eq!(
        first.chunks,
        [(1, 0, PORTABLE_FRAME_MEDIA_BYTES)],
        "{first:?}"
    );
    let high = first.last.unwrap();
    screen
        .get_active_tab_mut(CLIENT_1)
        .unwrap()
        .get_active_pane_mut(CLIENT_1)
        .unwrap()
        .terminal_mut()
        .unwrap()
        .handle_pty_bytes([b"\x1b[20;1H".to_vec(), kitty_rgba_upload(2, SECOND, 2)].concat());
    screen.render_to_clients().unwrap();
    let waiting = drain_transfers(&receiver, CLIENT_1);
    assert!(
        waiting.chunks.is_empty(),
        "the window is closed: {waiting:?}"
    );
    let usage = screen.core.native_render_clients[&CLIENT_1].media_usage();
    assert_eq!(
        (usage.entries, usage.bytes),
        (2, 2048 * 2048 * 4 + SECOND_BYTES)
    );
    (screen, receiver, high)
}

/// The side of the second image in `portable_viewer_mid_transfer`, larger than one frame's
/// media allowance.
const SECOND: u32 = 1536;
const SECOND_BYTES: usize = (SECOND * SECOND * 4) as usize;

#[test]
fn render_negotiation_portable_ack_opens_the_window_and_continues_transfers() {
    use yinz_runtime::native_media::PORTABLE_FRAME_MEDIA_BYTES;
    let (mut screen, receiver, high) = portable_viewer_mid_transfer();
    // A stale ACK opens nothing and renders nothing.
    screen.acknowledge_native_frame(CLIENT_1, high - 1).unwrap();
    screen.render_to_clients().unwrap();
    assert!(drain_transfers(&receiver, CLIENT_1).chunks.is_empty());
    // The ACK of the chunk frame re-renders the waiting pane, which carries the rest.
    screen.acknowledge_native_frame(CLIENT_1, high).unwrap();
    screen.render_to_clients().unwrap();
    let rest = drain_transfers(&receiver, CLIENT_1);
    assert_eq!(
        rest.chunks,
        [(
            1,
            PORTABLE_FRAME_MEDIA_BYTES as u32,
            PORTABLE_FRAME_MEDIA_BYTES
        )]
    );
    assert_eq!(rest.completed, [1]);
    // The second image starts on the next opened window, with the next id, and
    // completes on the one after.
    screen
        .acknowledge_native_frame(CLIENT_1, rest.last.unwrap())
        .unwrap();
    screen.render_to_clients().unwrap();
    let second = drain_transfers(&receiver, CLIENT_1);
    assert_eq!(second.chunks, [(2, 0, PORTABLE_FRAME_MEDIA_BYTES)]);
    assert!(second.completed.is_empty());
    screen
        .acknowledge_native_frame(CLIENT_1, second.last.unwrap())
        .unwrap();
    screen.render_to_clients().unwrap();
    let last = drain_transfers(&receiver, CLIENT_1);
    assert_eq!(
        last.chunks,
        [(
            2,
            PORTABLE_FRAME_MEDIA_BYTES as u32,
            SECOND_BYTES - PORTABLE_FRAME_MEDIA_BYTES
        )]
    );
    assert_eq!(last.completed, [2]);
    screen
        .acknowledge_native_frame(CLIENT_1, last.last.unwrap())
        .unwrap();
    let usage = screen.core.native_render_clients[&CLIENT_1].media_usage();
    assert_eq!((usage.entries, usage.bytes), (0, 0));
}

#[test]
fn render_negotiation_withdraw_mid_transfer_releases_leases_on_ack() {
    use yinz_types::render_negotiation::{Decision, Offer, Request};
    let (mut screen, receiver, _) = portable_viewer_mid_transfer();
    let withdrawn_at = screen.core.native_render_clients[&CLIENT_1].seq;
    screen
        .negotiate_render(CLIENT_1, Request::Withdraw)
        .unwrap();
    assert!(receiver.try_iter().any(|(event, _)| matches!(
        event,
        ServerInstruction::NativeRenderResult(id, Decision::Withdrawn(h)) if id == CLIENT_1 && h == withdrawn_at
    )));
    // The unstarted transfer is released at once; the started one waits for ACK(H).
    let retired = |screen: &Screen| screen.core.retired_native_clients[&CLIENT_1].media_usage();
    assert_eq!(
        (retired(&screen).entries, retired(&screen).bytes),
        (1, 2048 * 2048 * 4)
    );
    // A reattached viewer starts its own transfers under its own budget.
    let next = ClientId::new(2);
    screen.add_client(next).unwrap();
    screen
        .set_client_size(next, Size { cols: 80, rows: 24 })
        .unwrap();
    screen
        .negotiate_render(next, Request::Offer(Offer::reader(None)))
        .unwrap();
    screen.render_to_clients().unwrap();
    let reattached = drain_transfers(&receiver, next);
    assert_eq!(
        reattached.chunks.first().map(|c| (c.0, c.1)),
        Some((1, 0)),
        "{reattached:?}"
    );
    let fresh = |screen: &Screen| screen.core.native_render_clients[&next].media_usage();
    let held = fresh(&screen);
    assert!(held.entries >= 1, "{held:?}");
    screen
        .acknowledge_native_frame(CLIENT_1, withdrawn_at + 1)
        .unwrap();
    assert_eq!(retired(&screen).entries, 1);
    screen
        .acknowledge_native_frame(CLIENT_1, withdrawn_at)
        .unwrap();
    assert_eq!(retired(&screen).entries, 0);
    assert_eq!(
        fresh(&screen),
        held,
        "the old attachment's ACK released the new one's"
    );
    // Withdraw then close releases on removal instead.
    let (mut screen, _receiver, _) = portable_viewer_mid_transfer();
    screen
        .negotiate_render(CLIENT_1, Request::Withdraw)
        .unwrap();
    let budget = screen.core.retired_native_clients[&CLIENT_1]
        .media_budget()
        .cloned()
        .unwrap();
    assert_eq!(budget.usage().entries, 1);
    screen.remove_client(CLIENT_1).unwrap();
    assert_eq!(budget.usage().entries, 0);
}
