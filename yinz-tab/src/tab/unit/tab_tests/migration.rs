use super::*;

#[test]
fn migration_drains_scrolled_output_in_order_and_removes_empty_entries() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 10 }, false);
    let pid = TerminalId::new(1);
    let id = PaneId::Terminal(pid);
    tab.handle_pty_bytes(pid, b"line\r\n".repeat(30)).unwrap();
    tab.scroll_terminal_up(pid).unwrap();
    assert!(tab.get_pane_with_id(id).unwrap().is_scrolled());
    tab.handle_pty_bytes(pid, b"FIRST-".to_vec()).unwrap();
    tab.handle_pty_bytes(pid, b"SECOND".to_vec()).unwrap();
    assert_eq!(tab.pending_vte_events[&pid].len(), 2);
    tab.prepare_migration().unwrap();
    assert!(!tab.get_pane_with_id(id).unwrap().is_scrolled());
    assert!(tab.pending_vte_events.is_empty());
    tab.migration_state().unwrap();
    assert!(tab
        .get_pane_with_id_mut(id)
        .unwrap()
        .dump_screen(true, None)
        .contains("FIRST-SECOND"));
    // Reproduce the original failure independently of active scrolling.
    tab.pending_vte_events.insert(pid, vec![]);
    tab.process_pending_vte_events(pid).unwrap();
    assert!(tab.pending_vte_events.is_empty());
    tab.migration_state().unwrap();
}

#[test]
fn migration_waits_for_respawn_and_layout_completion() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 10 }, false);
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.pending_command_respawns
        .insert(id, (RunCommand::default(), false));
    tab.prepare_migration().unwrap();
    assert!(tab.migration_pending());
    tab.pending_command_respawns.clear();
    tab.is_pending = true;
    tab.handle_pty_bytes(TerminalId::new(1), b"queued".to_vec())
        .unwrap();
    assert!(tab.migration_pending());
    tab.is_pending = false;
    tab.apply_layout(
        TiledPaneLayout::default(),
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        ClientId::new(1),
        None,
    )
    .unwrap();
    assert!(!tab.migration_pending());
    tab.prepare_migration().unwrap();
    tab.migration_state().unwrap();
}

#[test]
fn migration_finishes_mouse_resize_at_applied_geometry() {
    use crate::tab::mouse_handler::{PaneResizeState, ResizeDragTarget};
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, false);
    let id = PaneId::Terminal(TerminalId::new(1));
    let geom = tab.get_pane_with_id(id).unwrap().position_and_size();
    tab.pane_being_resized_with_mouse = Some(PaneResizeState {
        pane_id: id,
        edge: yinz_panes::pane::PaneEdge::Right,
        start_position: yinz_types::position::Position::new(0, 0),
        start_geom: geom,
        target: ResizeDragTarget::Tiled,
    });
    tab.prepare_migration().unwrap();
    assert!(tab.pane_being_resized_with_mouse.is_none());
    assert_eq!(tab.get_pane_with_id(id).unwrap().position_and_size(), geom);
    tab.migration_state().unwrap();
}

#[test]
fn migration_keeps_overlay_terminal_output_and_ownership() {
    use yinz_types::data::NewPanePlacement;
    let mut tab = create_new_tab(Size { cols: 80, rows: 24 }, false);
    let id = PaneId::Terminal(TerminalId::new(2));
    tab.new_pane(
        id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(TerminalId::new(2), b"overlay terminal survives".to_vec())
        .unwrap();
    assert!(tab.pane_is_overlay(id));
    tab.prepare_migration().unwrap();
    assert!(tab.prepare_migration_overlays().is_empty());
    assert!(!tab.pane_is_overlay(id));
    assert!(tab.warp_pane_ids(None).unwrap().contains(&id));
    assert!(tab
        .get_pane_with_id_mut(id)
        .unwrap()
        .dump_screen(true, None)
        .contains("overlay terminal survives"));
}

/// A second tiled terminal, scrolled up with two chunks of output queued behind the scroll.
fn tab_with_scrolled_second_pane() -> (Tab, TerminalId) {
    let mut tab = create_new_tab(Size { cols: 80, rows: 20 }, false);
    let pid = TerminalId::new(2);
    tab.new_pane(
        PaneId::Terminal(pid),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(pid, b"line\r\n".repeat(30)).unwrap();
    tab.scroll_terminal_up(pid).unwrap();
    assert!(tab
        .get_pane_with_id(PaneId::Terminal(pid))
        .unwrap()
        .is_scrolled());
    tab.handle_pty_bytes(pid, b"QUEUED-".to_vec()).unwrap();
    tab.handle_pty_bytes(pid, b"OUTPUT".to_vec()).unwrap();
    assert_eq!(tab.pending_vte_events[&pid].len(), 2);
    (tab, pid)
}

#[test]
fn closing_a_scrolled_pane_leaves_no_queued_output_to_block_migration() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    tab.pids_waiting_resize.insert(pid);
    tab.close_pane(PaneId::Terminal(pid), false, None);
    assert!(!tab.has_pane_with_pid(&PaneId::Terminal(pid)));
    assert!(tab.pending_vte_events.is_empty());
    assert!(!tab.pids_waiting_resize.contains(&pid));
    tab.migration_state().unwrap();
}

#[test]
fn closing_a_scrolled_pane_explicitly_leaves_no_queued_output() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    tab.pids_waiting_resize.insert(pid);
    assert_eq!(
        tab.close_pane_explicitly(PaneId::Terminal(pid), None),
        vec![PaneId::Terminal(pid)]
    );
    assert!(tab.pending_vte_events.is_empty());
    assert!(!tab.pids_waiting_resize.contains(&pid));
    tab.migration_state().unwrap();
}

#[test]
fn extracting_a_scrolled_pane_takes_its_queued_output_along() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    let pane = tab
        .extract_pane_leaving_tab(PaneId::Terminal(pid), true)
        .unwrap();
    assert!(tab.pending_vte_events.is_empty());
    tab.migration_state().unwrap();
    assert!(!pane.is_scrolled());
    assert!(pane.dump_screen(true, None).contains("QUEUED-OUTPUT"));
}

#[test]
fn floating_a_scrolled_pane_keeps_its_scroll_and_queued_output() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    tab.toggle_pane_embed_or_floating_for_pane_id(PaneId::Terminal(pid), None)
        .unwrap();
    assert!(tab.pane_id_is_floating(&PaneId::Terminal(pid)));
    assert!(tab
        .get_pane_with_id(PaneId::Terminal(pid))
        .unwrap()
        .is_scrolled());
    assert_eq!(tab.pending_vte_events[&pid].len(), 2);
}

#[test]
fn migration_drops_queued_output_of_a_pane_that_has_left() {
    let mut tab = create_new_tab(Size { cols: 80, rows: 10 }, false);
    tab.pending_vte_events
        .insert(TerminalId::new(9), vec![b"orphan".to_vec()]);
    assert!(tab.migration_state().is_err());
    tab.prepare_migration().unwrap();
    assert!(tab.pending_vte_events.is_empty());
    tab.migration_state().unwrap();
}

#[test]
fn replacing_a_scrolled_pane_leaves_no_queued_output() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    tab.pids_waiting_resize.insert(pid);
    let replacement = tab
        .build_pane(PaneId::Terminal(TerminalId::new(3)), None, None, None)
        .unwrap();
    tab.close_pane_and_replace_with_other_pane(PaneId::Terminal(pid), replacement, None);
    assert!(!tab.has_pane_with_pid(&PaneId::Terminal(pid)));
    assert!(tab.pending_vte_events.is_empty());
    assert!(!tab.pids_waiting_resize.contains(&pid));
    tab.migration_state().unwrap();
}

#[test]
fn suppressed_panes_leaving_a_closing_tab_take_their_queued_output_along() {
    let (mut tab, pid) = tab_with_scrolled_second_pane();
    tab.suppress_pane_and_replace_with_pid(
        PaneId::Terminal(pid),
        PaneId::Terminal(TerminalId::new(3)),
        yinz_bus::pty::ReplacedPane::Suppress,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(tab.suppressed_panes.contains_key(&PaneId::Terminal(pid)));
    assert_eq!(tab.pending_vte_events[&pid].len(), 2);
    let mut extracted = tab.extract_suppressed_panes();
    assert!(tab.pending_vte_events.is_empty());
    let pane = extracted
        .remove(&PaneId::Terminal(pid))
        .expect("the suppressed pane leaves with the closing tab")
        .pane;
    assert!(!pane.is_scrolled());
    assert!(pane.dump_screen(true, None).contains("QUEUED-OUTPUT"));
}

#[test]
fn osc_status_publication_follows_parsing_including_buffered_replay() {
    use yinz_bus::screen::ScreenInstruction;
    for delivery in ["immediate", "scrolled", "forwarded"] {
        let mut tab = create_new_tab(Size { cols: 80, rows: 10 }, false);
        let pid = TerminalId::new(1);
        let id = PaneId::Terminal(pid);
        let (tx, rx) = unbounded();
        tab.senders.to_screen = Some(SenderWithContext::new(tx));
        let requests = || {
            rx.try_iter()
                .filter(|(instruction, _)| {
                    matches!(instruction, ScreenInstruction::RequestStateUpdateForPlugins)
                })
                .count()
        };
        if delivery == "scrolled" {
            tab.handle_pty_bytes(pid, b"line\r\n".repeat(30)).unwrap();
            tab.scroll_terminal_up(pid).unwrap();
            assert!(tab.get_pane_with_id(id).unwrap().is_scrolled());
        } else if delivery == "forwarded" {
            tab.get_pane_with_id_mut(id)
                .unwrap()
                .terminal_io_mut()
                .unwrap()
                .arm_forward_pause();
        }
        assert_eq!(requests(), 0);
        tab.handle_pty_bytes(pid, b"\x1b]7501;state=blocked\x07".to_vec())
            .unwrap();
        if delivery != "immediate" {
            assert_eq!(requests(), 0, "{delivery}: buffered bytes are not parsed");
            assert!(tab
                .get_pane_with_id(id)
                .unwrap()
                .program_status()
                .current
                .is_none());
            if delivery == "scrolled" {
                tab.get_pane_with_id_mut(id).unwrap().clear_scroll();
                tab.process_pending_vte_events(pid).unwrap();
            } else {
                tab.resume_pane_after_forward(pid, vec![]).unwrap();
            }
        }
        assert_eq!(requests(), 1, "{delivery}");
        assert!(tab
            .get_pane_with_id(id)
            .unwrap()
            .program_status()
            .current
            .is_some());
        tab.handle_pty_bytes(pid, b"ordinary output".to_vec())
            .unwrap();
        assert_eq!(requests(), 0);
    }
}
