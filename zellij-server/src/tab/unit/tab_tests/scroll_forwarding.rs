use super::*;

#[test]
pub fn scroll_terminal_up_forwards_sgr_when_pane_tracks_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let rx = install_pty_writer_capture(&mut tab);
    tab.handle_pty_bytes(crate::TerminalId(1), b"\x1b[?1000h\x1b[?1006h".to_vec())
        .unwrap();

    tab.scroll_terminal_up(crate::TerminalId(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId(1));
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
    tab.handle_pty_bytes(crate::TerminalId(1), b"\x1b[?1000h\x1b[?1006h".to_vec())
        .unwrap();

    tab.scroll_terminal_down(crate::TerminalId(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId(1));
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
    tab.handle_pty_bytes(crate::TerminalId(1), b"\x1b[?1049h".to_vec())
        .unwrap();

    tab.scroll_terminal_up(crate::TerminalId(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId(1));
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
    tab.handle_pty_bytes(crate::TerminalId(1), b"\x1b[?1049h".to_vec())
        .unwrap();

    tab.scroll_terminal_down(crate::TerminalId(1)).unwrap();

    let writes = drain_pty_writer(&rx);
    assert!(!writes.is_empty(), "expected at least one pty write");
    match &writes[0] {
        PtyWriteInstruction::Write(bytes, terminal_id, _) => {
            assert_eq!(*terminal_id, crate::TerminalId(1));
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

    tab.scroll_terminal_up(crate::TerminalId(1)).unwrap();

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

    tab.scroll_terminal_down(crate::TerminalId(1)).unwrap();

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

    tab.scroll_terminal_up(crate::TerminalId(999)).unwrap();

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

    tab.scroll_terminal_down(crate::TerminalId(999)).unwrap();

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
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            crate::ClientId(1),
            None,
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, ExtractedPanePlacement::Tiled),
        AcceptedPane::Accepted
    ));
    assert!(destination.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId(2))));
    assert!(!destination.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId(2))));
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
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            false,
            NewPanePlacement::Floating(None),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, ExtractedPanePlacement::Floating),
        AcceptedPane::Accepted
    ));
    assert!(destination.pane_id_is_floating(&PaneId::Terminal(crate::TerminalId(2))));
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
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            false,
            NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    // read before the extraction, which is what forgets it
    let placement = source.placement_for_extraction(PaneId::Terminal(crate::TerminalId(2)));
    assert!(matches!(placement, ExtractedPanePlacement::Overlay(..)));
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId(2)), true)
        .unwrap();

    assert!(matches!(
        destination.accept_extracted_pane(pane, placement),
        AcceptedPane::Accepted
    ));
    assert!(destination.pane_is_overlay(PaneId::Terminal(crate::TerminalId(2))));
    assert!(
        destination
            .overlay_panes
            .is_modal(&PaneId::Terminal(crate::TerminalId(2))),
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
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            crate::ClientId(1),
            None,
            None,
        )
        .unwrap();
    let pane = source
        .extract_pane(PaneId::Terminal(crate::TerminalId(2)), true)
        .unwrap();

    // a tab too small to hold anything more than what it already has
    let mut destination = create_new_tab(Size { cols: 10, rows: 3 }, true);
    while destination.tiled_panes.has_room_for_new_pane() {
        let next = PaneId::Terminal(crate::TerminalId(
            destination.get_all_pane_ids().len() as u32 + 100,
        ));
        destination
            .vertical_split(next, None, crate::ClientId(1), None, None)
            .unwrap();
    }

    match destination.accept_extracted_pane(pane, ExtractedPanePlacement::Tiled) {
        AcceptedPane::NoRoom(pane) => assert_eq!(
            pane.pid(),
            PaneId::Terminal(crate::TerminalId(2)),
            "the pane comes back intact, so the caller can put it back"
        ),
        _ => panic!("a full tab must refuse the pane rather than swallow it"),
    }
    assert!(!destination.has_pane_with_pid(&PaneId::Terminal(crate::TerminalId(2))));
}
