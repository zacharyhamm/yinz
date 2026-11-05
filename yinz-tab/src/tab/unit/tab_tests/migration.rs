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
