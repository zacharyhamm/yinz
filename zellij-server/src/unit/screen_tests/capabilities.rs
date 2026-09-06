#[cfg(test)]
const CLIENT_1: zellij_types::internal_ids::ClientId = zellij_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const PLUGIN_77: zellij_types::internal_ids::PluginId =
    zellij_types::internal_ids::PluginId::new(77);

use super::*;
use zellij_types::internal_ids::TabId;

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
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&true)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
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
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
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
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert!(
        active_pane.drain_messages_to_pty().is_empty(),
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
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
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
            .kitty_host_capabilities
            .borrow()
            .get(&ClientId::new(2)),
        None,
        "a departed client must not be recorded"
    );
    assert!(
        !screen.mode_info.contains_key(&ClientId::new(2)),
        "a departed client must not come back as a phantom in the mode_info map"
    );
    // ...and the aggregate still reflects only the client that is actually here.
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
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
    screen.update_sixel_support(crate::ClientId::new(1), true);
    assert_eq!(
        screen
            .sixel_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&true)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
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
    screen.update_sixel_support(crate::ClientId::new(1), false);
    assert_eq!(
        screen
            .sixel_host_capabilities
            .borrow()
            .get(&ClientId::new(1)),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
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
    screen.update_sixel_support(crate::ClientId::new(1), false);
    screen.update_sixel_support(crate::ClientId::new(2), true);
    screen.remove_client(crate::ClientId::new(2)).expect("TEST");
    let active_tab = screen.get_active_tab_mut(crate::ClientId::new(1)).unwrap();
    let active_pane = active_tab
        .get_active_pane_mut(crate::ClientId::new(1))
        .unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
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
        zellij_test_support::default_timeout(),
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
                character_cell_size: Some(SizeInPixels {
                    height: 21,
                    width: 8,
                }),
                text_area_size: None,
            },
        ));

    received_pty_instructions.wait_until_described(
        "every existing pty to be resized again with the reported pixel dimensions",
        zellij_test_support::default_timeout(),
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
