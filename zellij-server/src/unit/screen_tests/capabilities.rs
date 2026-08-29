use super::*;

#[test]
fn kitty_query_replies_ok_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(screen.kitty_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, false);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false),
        "a capable host must still be recorded as incapable when the protocol is disabled"
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
    new_tab(&mut screen, 1, 0);
    screen.add_client(2).expect("TEST");
    screen.update_kitty_graphics_support(1, false);
    screen.update_kitty_graphics_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn primary_da_advertises_sixel_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, true);
    assert_eq!(screen.sixel_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, false);
    assert_eq!(
        screen.sixel_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
    new_tab(&mut screen, 1, 0);
    screen.add_client(2).expect("TEST");
    screen.update_sixel_support(1, false);
    screen.update_sixel_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
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
            Some((*terminal_id, *width, *height))
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
