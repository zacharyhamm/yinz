use super::*;

#[test]
pub fn scroll_terminal_up_forwards_sgr_when_pane_tracks_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        b"\x1b[?1000h\x1b[?1006h".to_vec(),
    )
    .unwrap();

    tab.scroll_terminal_up(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId::new(1));
            let s = String::from_utf8_lossy(bytes);
            assert!(
                s.starts_with("\u{1b}[<64;"),
                "expected SGR wheel-up (button 64) prefix, got {s:?}"
            );
        },
        other => panic!("expected PtyWriteInstruction::Write, got {other:?}"),
    }
}

#[test]
pub fn scroll_terminal_down_forwards_sgr_when_pane_tracks_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        b"\x1b[?1000h\x1b[?1006h".to_vec(),
    )
    .unwrap();

    tab.scroll_terminal_down(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId::new(1));
            let s = String::from_utf8_lossy(bytes);
            assert!(
                s.starts_with("\u{1b}[<65;"),
                "expected SGR wheel-down (button 65) prefix, got {s:?}"
            );
        },
        other => panic!("expected PtyWriteInstruction::Write, got {other:?}"),
    }
}

#[test]
pub fn scroll_terminal_up_emits_arrow_up_in_alt_mode_without_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);
    tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[?1049h".to_vec())
        .unwrap();

    tab.scroll_terminal_up(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId::new(1));
            assert_eq!(bytes.as_slice(), b"\x1b[A");
        },
        other => panic!("expected PtyWriteInstruction::Write, got {other:?}"),
    }
}

#[test]
pub fn scroll_terminal_down_emits_arrow_down_in_alt_mode_without_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);
    tab.handle_pty_bytes(crate::TerminalId::new(1), b"\x1b[?1049h".to_vec())
        .unwrap();

    tab.scroll_terminal_down(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId::new(1));
            assert_eq!(bytes.as_slice(), b"\x1b[B");
        },
        other => panic!("expected PtyWriteInstruction::Write, got {other:?}"),
    }
}

#[test]
pub fn scroll_terminal_up_walks_scrollback_in_regular_mode_no_pty_write() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);

    tab.scroll_terminal_up(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(
        writes.is_empty(),
        "expected no pty writes in regular mode, got {writes:?}"
    );
}

#[test]
pub fn scroll_terminal_down_walks_scrollback_in_regular_mode_no_pty_write() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);

    tab.scroll_terminal_down(crate::TerminalId::new(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(
        writes.is_empty(),
        "expected no pty writes in regular mode, got {writes:?}"
    );
}

#[test]
pub fn scroll_terminal_up_nonexistent_pane_id_is_a_noop() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);

    tab.scroll_terminal_up(crate::TerminalId::new(999)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(writes.is_empty());
}

#[test]
pub fn scroll_terminal_down_nonexistent_pane_id_is_a_noop() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);

    tab.scroll_terminal_down(crate::TerminalId::new(999))
        .unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(writes.is_empty());
}

#[test]
fn accept_extracted_pane_takes_a_tiled_pane_in() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut source = create_new_tab(size, true);
    let mut destination = create_new_tab(size, true);
    source
        .vertical_split(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId::new(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, ExtractedPanePlacement::Tiled),
        AcceptedPane::Accepted
    ));
    assert!(destination.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(!destination.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId::new(2))));
}

#[test]
fn accept_extracted_pane_keeps_a_floating_pane_floating() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut source = create_new_tab(size, true);
    let mut destination = create_new_tab(size, true);
    source
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            false,
            NewPanePlacement::Floating(None),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId::new(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, ExtractedPanePlacement::Floating),
        AcceptedPane::Accepted
    ));
    assert!(destination.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId::new(2))));
}

/// A modal that moves between tabs has to arrive as a modal. Overlay-ness lives in the
/// collection the pane leaves, not on the pane, so it only survives the trip if the caller
/// reads the placement before extracting and hands it back in here.
#[test]
fn accept_extracted_pane_keeps_an_overlay_an_overlay() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut source = create_new_tab(size, true);
    let mut destination = create_new_tab(size, true);
    source
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            false,
            NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    // read before the extraction, which is what forgets it
    let placement = source.placement_for_extraction(PaneId::Terminal(crate::TerminalId::new(2)));
    assert!(matches!(placement, ExtractedPanePlacement::Overlay(..)));
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId::new(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, placement),
        AcceptedPane::Accepted
    ));
    assert!(destination.pane_is_overlay(PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(
        destination
            .overlay_panes
            .is_modal(&PaneId::Terminal(crate::TerminalId::new(2))),
        "a modal arrives as a modal, not as a dismissible overlay"
    );
    assert!(
        !destination.are_floating_panes_visible(),
        "an overlay arriving does not raise the destination's floating layer"
    );
}

/// The whole reason this method exists: `add_tiled_pane` would drop the box here and
/// return `Ok(())`, destroying a pane that has already left the tab it came from.
#[test]
fn accept_extracted_pane_hands_the_pane_back_when_the_tab_is_full() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut source = create_new_tab(size, true);
    source
        .vertical_split(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            crate::ClientId::new(1),
            None,
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId::new(2)), true)
        .unwrap();

    // a tab too small to hold anything more than what it already has
    let mut destination = create_new_tab(Size { cols: 10, rows: 5 }, true);
    while destination.tiled_panes.has_room_for_new_pane() {
        let next = PaneId::Terminal(crate::TerminalId::new(
            destination.get_all_pane_ids().len() as u32 + 100,
        ));
        destination
            .vertical_split(next, None, None, crate::ClientId::new(1), None, None)
            .unwrap();
    }

    match destination.accept_extracted_pane(pane, ExtractedPanePlacement::Tiled) {
        AcceptedPane::NoRoom(pane) => assert_eq!(
            pane.pid(),
            PaneId::Terminal(crate::TerminalId::new(2)),
            "the pane comes back intact, so the caller can put it back"
        ),
        _ => panic!("a full tab must refuse the pane rather than swallow it"),
    }
    assert!(!destination.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId::new(2))));
}

#[test]
fn suspended_input_resumes_only_the_addressed_terminal() {
    use crate::pty::PtyInstruction;
    use yinz_runtime::channels;
    let client = ClientId::new(1);
    let mut tab = create_new_tab(
        Size {
            cols: 120,
            rows: 20,
        },
        true,
    );
    let id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.new_pane(
        id,
        None,
        None,
        false,
        false,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    let writer = install_pty_writer_capture(&mut tab);
    let (tx, pty) = channels::unbounded();
    tab.senders.to_pty = Some(SenderWithContext::new(tx));
    let (tx, screen) = channels::unbounded();
    tab.senders.to_screen = Some(SenderWithContext::new(tx));
    tab.set_pane_suspended(id, true).unwrap();
    for byte in *b"x\r " {
        tab.write_to_pane_id(&None, vec![byte], false, id, Some(client), None)
            .unwrap();
        if byte != b'x' {
            assert!(
                matches!(pty.try_recv().unwrap().0, PtyInstruction::ResumeSuspendedPane(target) if target == id)
            );
        }
        assert!(
            pty.try_recv().is_err() && writer.try_recv().is_err() && screen.try_recv().is_err()
        );
    }
    tab.set_pane_suspended(id, false).unwrap();
    tab.write_to_pane_id(&None, vec![b'x'], false, id, Some(client), None)
        .unwrap();
    assert!(
        matches!(writer.try_recv().unwrap().0, PtyWriteInstruction::Write(bytes, target, _) if bytes == b"x" && PaneId::Terminal(target) == id)
    );
}

/// One viewport reset moves at most twice the scroll buffer size in rows, and deeply wrapped
/// history can be further away than that. Every return-to-bottom action must leave queued output
/// queued until the viewport arrives: parsing it into the scrolled viewport consumes it there and
/// it never reaches the live one.
#[test]
fn bottom_actions_keep_output_queued_until_a_bounded_reset_reaches_the_bottom() {
    let terminal_id = crate::TerminalId::new(1);
    let pane_id = PaneId::Terminal(terminal_id);
    for action in ["active pane", "terminal ID", "pane ID", "clear scroll"] {
        let scroll_to_bottom = |tab: &mut Tab| match action {
            "active pane" => tab
                .scroll_active_terminal_to_bottom(ClientId::new(1))
                .unwrap(),
            "terminal ID" => tab.scroll_terminal_to_bottom(terminal_id),
            "pane ID" => tab.scroll_to_bottom_by_pane_id(pane_id).unwrap(),
            _ => assert!(tab.clear_active_terminal_scroll(ClientId::new(1)).unwrap()),
        };
        let mut tab = create_new_tab(Size { cols: 22, rows: 6 }, true);
        let (cols, rows) = tab
            .get_pane_with_id(pane_id)
            .map(|pane| (pane.get_content_columns(), pane.get_content_rows()))
            .unwrap();
        // Building the pane's grid initialized the buffer size this bound comes from.
        let reset_limit = *yinz_runtime::consts::SCROLL_BUFFER_SIZE.get().unwrap() * 2;
        // The buffer holds logical lines, so wrapping each over three rows keeps it deep enough.
        let wrapped_line = [vec![b'X'; cols * 3], b"\r\n".to_vec()].concat();
        tab.handle_pty_bytes(terminal_id, wrapped_line.repeat(reset_limit / 3 + rows + 4))
            .unwrap();
        let excess = 3;
        tab.get_terminal_mut(pane_id)
            .unwrap()
            .scroll_up(reset_limit + excess);
        tab.handle_pty_bytes(terminal_id, b"queued output".to_vec())
            .unwrap();
        assert!(tab.pending_vte_events.contains_key(&terminal_id));

        scroll_to_bottom(&mut tab);
        let pane = tab.get_pane_with_id(pane_id).unwrap();
        assert!(
            pane.terminal().unwrap().is_scrolled(),
            "{action}: one reset must stop {excess} rows short of the bottom"
        );
        assert!(
            tab.pending_vte_events.contains_key(&terminal_id)
                && !pane.dump_screen(true, None).contains("queued output"),
            "{action}: output was fed into a scrolled viewport"
        );

        scroll_to_bottom(&mut tab);
        let pane = tab.get_pane_with_id(pane_id).unwrap();
        assert!(
            !pane.terminal().unwrap().is_scrolled(),
            "{action}: a second reset reaches the bottom"
        );
        assert!(!tab.pending_vte_events.contains_key(&terminal_id));
        let viewport = pane.dump_screen(false, None);
        assert!(
            viewport.lines().last() == Some("queued output"),
            "{action}: queued output is missing from the live viewport: {viewport:?}"
        );
    }
}
