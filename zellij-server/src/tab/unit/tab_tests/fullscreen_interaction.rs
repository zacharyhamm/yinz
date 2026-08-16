use super::*;

#[test]
pub fn opening_new_pane_while_fullscreen_unsets_fullscreen() {
    let mut tab = create_tab_with_four_panes();
    tab.toggle_active_pane_fullscreen(1);
    assert!(tab.is_fullscreen_active(), "Fullscreen active before split");
    open_pane_five(&mut tab);

    assert!(
        !tab.is_fullscreen_active(),
        "Opening a new pane breaks out of fullscreen"
    );
    assert_eq!(
        tiled_pane_geoms(&tab).len(),
        5,
        "All panes including the new one are tiled"
    );
    assert_panes_tile_the_whole_display(
        &tab,
        Size {
            cols: 121,
            rows: 20,
        },
    );
}

#[test]
pub fn opening_new_pane_while_no_ui_fullscreen_unsets_fullscreen() {
    let mut tab = create_tab_with_four_panes();
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        tab.fullscreen_covers_ui(),
        "NoUi fullscreen active before split"
    );
    open_pane_five(&mut tab);

    assert!(
        !tab.is_fullscreen_active(),
        "Opening a new pane breaks out of no-ui fullscreen"
    );
    assert!(
        !tab.fullscreen_covers_ui(),
        "NoUi flag cleared after breaking out"
    );
    assert_eq!(
        tiled_pane_geoms(&tab).len(),
        5,
        "All panes including the new one are tiled"
    );
    assert_panes_tile_the_whole_display(
        &tab,
        Size {
            cols: 121,
            rows: 20,
        },
    );
}

#[test]
pub fn closing_the_fullscreen_pane_restores_remaining_layout() {
    let mut tab_without_fullscreen = create_tab_with_four_panes();
    tab_without_fullscreen.close_pane(PaneId::Terminal(4), false, None);

    let mut tab = create_tab_with_four_panes();
    tab.toggle_active_pane_fullscreen(1);
    assert!(tab.is_fullscreen_active(), "Fullscreen active before close");
    tab.close_pane(PaneId::Terminal(4), false, None);

    assert!(
        !tab.is_fullscreen_active(),
        "Closing the fullscreen pane leaves fullscreen"
    );
    assert_eq!(
        tiled_pane_geoms(&tab),
        tiled_pane_geoms(&tab_without_fullscreen),
        "Remaining panes match a close that never went through fullscreen"
    );
}

#[test]
pub fn closing_the_no_ui_fullscreen_pane_restores_remaining_layout() {
    let mut tab_without_fullscreen = create_tab_with_four_panes();
    tab_without_fullscreen.close_pane(PaneId::Terminal(4), false, None);

    let mut tab = create_tab_with_four_panes();
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        tab.fullscreen_covers_ui(),
        "NoUi fullscreen active before close"
    );
    tab.close_pane(PaneId::Terminal(4), false, None);

    assert!(
        !tab.is_fullscreen_active(),
        "Closing the no-ui fullscreen pane leaves fullscreen"
    );
    assert!(
        !tab.fullscreen_covers_ui(),
        "NoUi flag cleared after the close"
    );
    assert_eq!(
        tiled_pane_geoms(&tab),
        tiled_pane_geoms(&tab_without_fullscreen),
        "Remaining panes match a close that never went through no-ui fullscreen"
    );
}

#[test]
pub fn closing_fullscreen_scrollback_editor_restores_consistent_layout() {
    // Replacing a pane (e.g. opening or closing a scrollback editor) swaps
    // the pane id occupying its tiled slot. If the replaced pane was the
    // fullscreen pane, the fullscreen bookkeeping must follow the swap so
    // that toggling fullscreen off later resets the geom_override on the
    // pane that actually carries it. Otherwise the restored pane keeps the
    // 100% override, the previously-hidden panes come back into a layout
    // that overlaps it, and the screen renders incorrectly.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let client_id = 1;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        tab.new_pane(
            PaneId::Terminal(i),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client_id),
            None,
        )
        .unwrap();
    }
    let active_pane_id = tab
        .get_active_pane_id(client_id)
        .expect("active pane exists before opening editor");

    let editor_pane_id = PaneId::Terminal(99);
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(editor_pane_id),
        "editor pane is now active",
    );

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        tab.is_fullscreen_active(),
        "fullscreen is active on the editor pane",
    );
    assert_eq!(
        tab.tiled_panes.fullscreen_pane_id(),
        Some(editor_pane_id),
        "fullscreen tracks the editor pane id",
    );

    // Close the editor: this restores the originally-suppressed pane in the
    // editor's slot. Fullscreen bookkeeping must retarget to the restored
    // pane id so subsequent fullscreen-off cleanup hits the right pane.
    tab.close_pane(editor_pane_id, false, None);
    assert!(
        tab.is_fullscreen_active(),
        "fullscreen is preserved after the editor is closed",
    );
    assert_eq!(
        tab.tiled_panes.fullscreen_pane_id(),
        Some(active_pane_id),
        "fullscreen now tracks the restored suppressed pane",
    );

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        !tab.is_fullscreen_active(),
        "fullscreen is cleared after the second toggle",
    );
    assert_eq!(
        tab.tiled_panes.panes_to_hide_count(),
        0,
        "no panes remain hidden after exiting fullscreen",
    );
    let restored_pane = tab
        .tiled_panes
        .panes
        .get(&active_pane_id)
        .expect("restored pane is present");
    assert!(
        restored_pane.geom_override().is_none(),
        "restored pane no longer carries the fullscreen geom_override",
    );
    for pane in tab.tiled_panes.panes.values() {
        let geom = pane.position_and_size();
        assert!(
            geom.x + geom.cols.as_usize() <= size.cols,
            "pane fits horizontally after exiting fullscreen: \
             x={}, cols={}, display_cols={}",
            geom.x,
            geom.cols.as_usize(),
            size.cols,
        );
        assert!(
            geom.y + geom.rows.as_usize() <= size.rows,
            "pane fits vertically after exiting fullscreen: \
             y={}, rows={}, display_rows={}",
            geom.y,
            geom.rows.as_usize(),
            size.rows,
        );
    }
}

#[test]
pub fn opening_scrollback_editor_on_fullscreen_pane_retargets_fullscreen() {
    // Reverse-direction variant: fullscreen the pane *first*, then open the
    // scrollback editor on it. The editor takes the fullscreen pane's slot
    // and inherits its 100% geom_override, so the fullscreen bookkeeping
    // must follow the swap onto the editor's pane id. Otherwise toggling
    // fullscreen off later cannot reset the override on the editor.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let client_id = 1;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        tab.new_pane(
            PaneId::Terminal(i),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client_id),
            None,
        )
        .unwrap();
    }
    let active_pane_id = tab
        .get_active_pane_id(client_id)
        .expect("active pane exists");

    tab.toggle_active_pane_fullscreen(client_id);
    assert_eq!(
        tab.tiled_panes.fullscreen_pane_id(),
        Some(active_pane_id),
        "fullscreen tracks the original pane",
    );

    let editor_pane_id = PaneId::Terminal(99);
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    assert!(
        tab.is_fullscreen_active(),
        "fullscreen state survives the editor swap",
    );
    assert_eq!(
        tab.tiled_panes.fullscreen_pane_id(),
        Some(editor_pane_id),
        "fullscreen now tracks the editor pane id, not the suppressed one",
    );

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        !tab.is_fullscreen_active(),
        "fullscreen is cleared after the second toggle",
    );
    let editor_pane = tab
        .tiled_panes
        .panes
        .get(&editor_pane_id)
        .expect("editor pane is present in tiled panes");
    assert!(
        editor_pane.geom_override().is_none(),
        "editor pane no longer carries the fullscreen geom_override",
    );
    assert_eq!(
        tab.tiled_panes.panes_to_hide_count(),
        0,
        "no panes remain hidden after exiting fullscreen",
    );
    for pane in tab.tiled_panes.panes.values() {
        let geom = pane.position_and_size();
        assert!(
            geom.x + geom.cols.as_usize() <= size.cols
                && geom.y + geom.rows.as_usize() <= size.rows,
            "pane fits inside the display area: \
             x={}, y={}, cols={}, rows={}, display={}x{}",
            geom.x,
            geom.y,
            geom.cols.as_usize(),
            geom.rows.as_usize(),
            size.cols,
            size.rows,
        );
    }
}

#[test]
fn switch_to_next_pane_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let stacked_resize = true;
    let mut active_tab = create_new_tab(size, stacked_resize);

    active_tab
        .new_pane(
            PaneId::Terminal(1),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(4),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab.toggle_active_pane_fullscreen(1);

    // order is now 1 ->2 -> 3 -> 4 due to how new panes are inserted

    active_tab.switch_next_pane_fullscreen(1);
    active_tab.switch_next_pane_fullscreen(1);
    active_tab.switch_next_pane_fullscreen(1);
    active_tab.switch_next_pane_fullscreen(1);

    // position should now be back in terminal 4.

    assert_eq!(
        active_tab.get_active_pane_id(1).unwrap(),
        PaneId::Terminal(4),
        "Active pane did not switch in fullscreen mode"
    );
}

#[test]
fn switch_to_prev_pane_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut active_tab = create_new_tab(size, stacked_resize);

    //testing four consecutive switches in fullscreen mode

    active_tab
        .new_pane(
            PaneId::Terminal(1),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(4),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab.toggle_active_pane_fullscreen(1);
    // order is now 1 2 3 4

    active_tab.switch_prev_pane_fullscreen(1);
    active_tab.switch_prev_pane_fullscreen(1);
    active_tab.switch_prev_pane_fullscreen(1);
    active_tab.switch_prev_pane_fullscreen(1);

    // the position should now be in Terminal 4.

    assert_eq!(
        active_tab.get_active_pane_id(1).unwrap(),
        PaneId::Terminal(4),
        "Active pane did not switch in fullscreen mode"
    );
}

#[test]
fn switch_to_last_pane_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut active_tab = create_new_tab(size, stacked_resize);

    //testing four consecutive switches in fullscreen mode

    active_tab
        .new_pane(
            PaneId::Terminal(1),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(4),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    active_tab.toggle_active_pane_fullscreen(1);

    // order is now 1 2 3 4, current active is Terminal 4

    active_tab.switch_last_pane_fullscreen(1);

    // the position should now be in Terminal 3

    assert_eq!(
        active_tab.get_active_pane_id(1).unwrap(),
        PaneId::Terminal(3),
        "Active pane did not switch to the last pane in fullscreen mode"
    );

    active_tab.switch_last_pane_fullscreen(1);

    // the position should now be back in Terminal 4

    assert_eq!(
        active_tab.get_active_pane_id(1).unwrap(),
        PaneId::Terminal(4),
        "Active pane did not switch to the last pane in fullscreen mode"
    );
}
