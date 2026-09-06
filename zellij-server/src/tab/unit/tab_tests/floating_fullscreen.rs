use super::*;

#[test]
pub fn toggle_floating_pane_fullscreen_expands_over_viewport() {
    let mut tab = create_tab_with_two_floating_panes();
    let viewport = *tab.viewport.borrow();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    assert!(tab.is_fullscreen_active(), "Floating fullscreen is active");
    assert!(
        !tab.fullscreen_covers_ui(),
        "Regular floating fullscreen leaves the UI rows"
    );
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(active_pane_id),
        "Fullscreen tracks the active floating pane"
    );
    let geom = floating_pane_geom(&tab, active_pane_id);
    assert_eq!(geom.x, viewport.x, "Fullscreen pane x matches viewport");
    assert_eq!(geom.y, viewport.y, "Fullscreen pane y matches viewport");
    assert_eq!(
        geom.cols.as_usize(),
        viewport.cols,
        "Fullscreen pane cols match viewport cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        viewport.rows,
        "Fullscreen pane rows match viewport rows"
    );
}

#[test]
pub fn toggle_floating_pane_no_ui_fullscreen_covers_whole_display() {
    let mut tab = create_tab_with_two_floating_panes();
    let display_area = *tab.display_area.borrow();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");

    tab.toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));

    assert!(
        tab.is_fullscreen_active(),
        "Floating no-ui fullscreen is active"
    );
    assert!(
        tab.fullscreen_covers_ui(),
        "Floating no-ui fullscreen covers the UI rows"
    );
    let geom = floating_pane_geom(&tab, active_pane_id);
    assert_eq!(geom.x, 0, "No-ui fullscreen pane x is on display edge");
    assert_eq!(geom.y, 0, "No-ui fullscreen pane y is on display edge");
    assert_eq!(
        geom.cols.as_usize(),
        display_area.cols,
        "No-ui fullscreen pane cols match display cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        display_area.rows,
        "No-ui fullscreen pane rows match display rows"
    );
}

#[test]
pub fn toggle_floating_pane_fullscreen_off_restores_geometry() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    let geom_before = floating_pane_geom(&tab, active_pane_id);

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(tab.is_fullscreen_active(), "Fullscreen active after toggle");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    assert!(
        !tab.is_fullscreen_active(),
        "Fullscreen cleared after toggling off"
    );
    assert_eq!(
        floating_pane_geom(&tab, active_pane_id),
        geom_before,
        "Floating pane geometry restored after fullscreen off"
    );
}

#[test]
pub fn regular_floating_fullscreen_switches_to_no_ui() {
    let mut tab = create_tab_with_two_floating_panes();
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(
        tab.is_fullscreen_active() && !tab.fullscreen_covers_ui(),
        "Regular floating fullscreen active"
    );

    tab.toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));
    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen stays active when switching kinds"
    );
    assert!(
        tab.fullscreen_covers_ui(),
        "Fullscreen switched to covering the UI rows"
    );

    tab.toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));
    assert!(
        !tab.is_fullscreen_active(),
        "Fullscreen toggled off entirely"
    );
}

#[test]
pub fn floating_no_ui_fullscreen_downgrades_to_regular_fullscreen() {
    let mut tab = create_tab_with_two_floating_panes();
    let viewport = *tab.viewport.borrow();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));
    assert!(
        tab.is_fullscreen_active() && tab.fullscreen_covers_ui(),
        "Floating no-ui fullscreen is active"
    );

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen stays active after the downgrade"
    );
    assert!(
        !tab.fullscreen_covers_ui(),
        "The downgrade restores the UI rows"
    );
    let geom = floating_pane_geom(&tab, active_pane_id);
    assert_eq!(geom.x, viewport.x, "Pane x matches the viewport");
    assert_eq!(geom.y, viewport.y, "Pane y matches the viewport");
    assert_eq!(
        geom.cols.as_usize(),
        viewport.cols,
        "Pane cols match the viewport cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        viewport.rows,
        "Pane rows match the viewport rows"
    );

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(
        !tab.is_fullscreen_active(),
        "The next regular toggle leaves fullscreen entirely"
    );
}

#[test]
pub fn floating_fullscreen_on_lone_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    let viewport = *tab.viewport.borrow();

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    assert!(
        tab.is_fullscreen_active(),
        "A lone floating pane still enters fullscreen"
    );
    let geom = floating_pane_geom(&tab, PaneId::Terminal(crate::TerminalId::new(2)));
    assert_eq!(
        geom.cols.as_usize(),
        viewport.cols,
        "Lone floating fullscreen pane covers the viewport cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        viewport.rows,
        "Lone floating fullscreen pane covers the viewport rows"
    );
}

#[test]
pub fn floating_and_tiled_fullscreen_are_mutually_exclusive() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(3))),
        "The focused floating pane is fullscreen"
    );
    assert!(
        !tab.tiled_panes.fullscreen_is_active(),
        "No tiled fullscreen while a floating pane is fullscreen"
    );

    tab.hide_floating_panes();
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(
        tab.tiled_panes.fullscreen_is_active(),
        "A tiled pane can be fullscreen after floating is hidden"
    );
    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "No floating fullscreen while a tiled pane is fullscreen"
    );
}

#[test]
pub fn move_focus_while_floating_fullscreen_transfers_fullscreen() {
    let mut tab = create_tab_with_two_floating_panes();
    let first = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");

    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(first),
        "Fullscreen starts on the focused floating pane"
    );

    let _ = tab.move_focus_left(crate::ClientId::new(1));

    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen stays active after moving focus"
    );
    let new_active = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused after the move");
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(new_active),
        "Fullscreen transferred to the newly focused floating pane"
    );
}

#[test]
pub fn resize_while_floating_fullscreen_is_noop() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    let geom_before = floating_pane_geom(&tab, active_pane_id);

    tab.resize(
        crate::ClientId::new(1),
        ResizeStrategy::new(Resize::Increase, None),
    )
    .unwrap();

    assert_eq!(
        floating_pane_geom(&tab, active_pane_id),
        geom_before,
        "Resizing a floating fullscreen pane does not change its geometry"
    );
}

#[test]
pub fn move_pane_while_floating_fullscreen_is_noop() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    let geom_before = floating_pane_geom(&tab, active_pane_id);

    tab.move_active_pane_down(crate::ClientId::new(1));
    tab.move_active_pane_up(crate::ClientId::new(1));
    tab.move_active_pane_left(crate::ClientId::new(1));
    tab.move_active_pane_right(crate::ClientId::new(1));

    assert_eq!(
        floating_pane_geom(&tab, active_pane_id),
        geom_before,
        "Moving a floating fullscreen pane does not change its geometry"
    );
}

#[test]
pub fn closing_floating_fullscreen_pane_clears_state() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(tab.is_fullscreen_active(), "Fullscreen active before close");

    tab.close_pane(active_pane_id, false, None);

    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "Closing the fullscreen floating pane clears fullscreen state"
    );
}

#[test]
pub fn adding_floating_pane_while_fullscreen_unsets_fullscreen() {
    let mut tab = create_tab_with_two_floating_panes();
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(tab.is_fullscreen_active(), "Fullscreen active before add");

    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "Adding a floating pane breaks out of floating fullscreen"
    );
}

#[test]
pub fn hiding_floating_layer_while_fullscreen_unsets_fullscreen() {
    let mut tab = create_tab_with_two_floating_panes();
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(tab.is_fullscreen_active(), "Fullscreen active before hide");

    tab.hide_floating_panes();

    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "Hiding the floating layer clears floating fullscreen"
    );
}

#[test]
pub fn embed_floating_fullscreen_pane_unsets_and_embeds() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));
    assert!(tab.is_fullscreen_active(), "Fullscreen active before embed");

    tab.toggle_pane_embed_or_floating(crate::ClientId::new(1))
        .unwrap();

    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "Embedding clears floating fullscreen state"
    );
    assert!(
        tab.tiled_panes.panes_contain(&active_pane_id),
        "The embedded pane is now a tiled pane"
    );
}

#[test]
pub fn resize_whole_tab_while_floating_fullscreen_preserves_regular() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    let new_size = Size { cols: 80, rows: 30 };
    tab.resize_whole_tab(new_size).unwrap();

    assert!(
        tab.is_fullscreen_active() && !tab.fullscreen_covers_ui(),
        "Regular floating fullscreen preserved across resize"
    );
    let viewport = *tab.viewport.borrow();
    let geom = floating_pane_geom(&tab, active_pane_id);
    assert_eq!(
        geom.cols.as_usize(),
        viewport.cols,
        "Floating fullscreen pane re-expands to the new viewport cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        viewport.rows,
        "Floating fullscreen pane re-expands to the new viewport rows"
    );
}

#[test]
pub fn resize_whole_tab_while_floating_no_ui_fullscreen_preserves_no_ui() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_no_ui_fullscreen(crate::ClientId::new(1));

    let new_size = Size { cols: 80, rows: 30 };
    tab.resize_whole_tab(new_size).unwrap();

    assert!(
        tab.fullscreen_covers_ui(),
        "Floating no-ui fullscreen preserved across resize"
    );
    let geom = floating_pane_geom(&tab, active_pane_id);
    assert_eq!(
        geom.cols.as_usize(),
        new_size.cols,
        "Floating no-ui fullscreen pane re-expands to the new display cols"
    );
    assert_eq!(
        geom.rows.as_usize(),
        new_size.rows,
        "Floating no-ui fullscreen pane re-expands to the new display rows"
    );
}

#[test]
pub fn pane_info_reports_floating_fullscreen() {
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(crate::ClientId::new(1))
        .expect("a floating pane is focused");
    tab.toggle_active_pane_fullscreen(crate::ClientId::new(1));

    let fullscreen_info = tab
        .get_pane_info(active_pane_id)
        .expect("pane info for the fullscreen floating pane");
    assert!(
        fullscreen_info.is_fullscreen,
        "The fullscreen floating pane reports is_fullscreen"
    );
    assert!(
        fullscreen_info.is_floating,
        "The fullscreen pane still reports is_floating"
    );

    let other_pane_id = if active_pane_id == PaneId::Terminal(crate::TerminalId::new(2)) {
        PaneId::Terminal(crate::TerminalId::new(3))
    } else {
        PaneId::Terminal(crate::TerminalId::new(2))
    };
    let other_info = tab
        .get_pane_info(other_pane_id)
        .expect("pane info for the other floating pane");
    assert!(
        !other_info.is_fullscreen,
        "A non-fullscreen floating pane does not report is_fullscreen"
    );
}

#[test]
pub fn toggle_floating_fullscreen_by_pane_id() {
    let mut tab = create_tab_with_two_floating_panes();

    tab.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(2)));
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "By-id fullscreen targets the requested floating pane"
    );

    tab.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(2)));
    assert!(
        !tab.is_fullscreen_active(),
        "By-id toggle off clears floating fullscreen"
    );
}

#[test]
pub fn opening_scrollback_editor_on_fullscreen_floating_pane_retargets_fullscreen() {
    // Fullscreen the floating pane *first*, then open the scrollback editor on
    // it. The editor takes the fullscreen pane's slot and inherits its 100%
    // geom_override, so the fullscreen bookkeeping must follow the swap onto
    // the editor's pane id, and the editor must keep the replaced pane's base
    // geometry to fall back to when fullscreen is toggled off.
    let client_id = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let viewport = *tab.viewport.borrow();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(client_id)
        .expect("a floating pane is focused");
    let original_geom = tab
        .floating_panes
        .get(&active_pane_id)
        .expect("focused floating pane exists")
        .position_and_size();

    tab.toggle_active_pane_fullscreen(client_id);
    assert_eq!(
        tab.floating_panes.fullscreen_pane_id(),
        Some(active_pane_id),
        "fullscreen tracks the original floating pane",
    );

    let editor_pane_id = PaneId::Terminal(crate::TerminalId::new(99));
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    assert_eq!(
        tab.floating_panes.fullscreen_pane_id(),
        Some(editor_pane_id),
        "fullscreen now tracks the editor pane id, not the suppressed one",
    );
    let editor_geom = floating_pane_geom(&tab, editor_pane_id);
    assert_eq!(
        editor_geom.cols.as_usize(),
        viewport.cols,
        "the editor pane covers the viewport cols",
    );
    assert_eq!(
        editor_geom.rows.as_usize(),
        viewport.rows,
        "the editor pane covers the viewport rows",
    );

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "fullscreen is cleared after the second toggle",
    );
    let editor_pane = tab
        .floating_panes
        .get(&editor_pane_id)
        .expect("editor pane is present in floating panes");
    assert!(
        editor_pane.geom_override().is_none(),
        "editor pane no longer carries the fullscreen geom_override",
    );
    assert_eq!(
        editor_pane.position_and_size(),
        original_geom,
        "editor pane falls back to the replaced pane's original geometry",
    );
}

#[test]
pub fn closing_fullscreen_floating_scrollback_editor_restores_geometry() {
    // The other order: open the editor, fullscreen it, then close it. Closing
    // swaps the suppressed pane back in through the same replace path, so the
    // fullscreen bookkeeping and the override must travel back with it.
    let client_id = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let active_pane_id = tab
        .floating_panes
        .active_pane_id(client_id)
        .expect("a floating pane is focused");
    let original_geom = tab
        .floating_panes
        .get(&active_pane_id)
        .expect("focused floating pane exists")
        .position_and_size();

    let editor_pane_id = PaneId::Terminal(crate::TerminalId::new(99));
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    tab.toggle_active_pane_fullscreen(client_id);
    assert_eq!(
        tab.floating_panes.fullscreen_pane_id(),
        Some(editor_pane_id),
        "fullscreen tracks the editor pane",
    );

    tab.close_pane(editor_pane_id, false, None);
    assert_eq!(
        tab.floating_panes.fullscreen_pane_id(),
        Some(active_pane_id),
        "fullscreen now tracks the restored suppressed pane",
    );

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        !tab.floating_panes.fullscreen_is_active(),
        "fullscreen is cleared after the second toggle",
    );
    let restored_pane = tab
        .floating_panes
        .get(&active_pane_id)
        .expect("restored pane is present");
    assert!(
        restored_pane.geom_override().is_none(),
        "restored pane no longer carries the fullscreen geom_override",
    );
    assert_eq!(
        restored_pane.position_and_size(),
        original_geom,
        "restored pane is back to its original geometry",
    );
}
