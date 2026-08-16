use super::*;

#[test]
pub fn toggle_focused_pane_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_fullscreen(1);
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().x(),
        0,
        "Pane x is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().y(),
        0,
        "Pane y is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .cols(),
        121,
        "Pane cols match fullscreen cols"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .rows(),
        20,
        "Pane rows match fullscreen rows"
    );
    tab.toggle_active_pane_fullscreen(1);
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().x(),
        61,
        "Pane x is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().y(),
        10,
        "Pane y is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .cols(),
        60,
        "Pane cols match fullscreen cols"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .rows(),
        10,
        "Pane rows match fullscreen rows"
    );
    // we don't test if all other panes are hidden because this logic is done in the render
    // function and we already test that in the e2e tests
}

#[test]
pub fn toggle_focused_pane_fullscreen_with_stacked_resizes() {
    // note - this is the default
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_fullscreen(1);
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().x(),
        0,
        "Pane x is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().y(),
        0,
        "Pane y is on screen edge"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .cols(),
        121,
        "Pane cols match fullscreen cols"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .rows(),
        20,
        "Pane rows match fullscreen rows"
    );
    tab.toggle_active_pane_fullscreen(1);
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().x(),
        61,
        "Pane x is back to its original position"
    );
    assert_eq!(
        tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap().y(),
        2,
        "Pane y is back to its original position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .cols(),
        60,
        "Pane cols are back at their original position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(4))
            .unwrap()
            .rows(),
        18,
        "Pane rows are back at their original position"
    );
    // we don't test if all other panes are hidden because this logic is done in the render
    // function and we already test that in the e2e tests
}

#[test]
pub fn resize_whole_tab_while_fullscreen_preserves_fullscreen() {
    // A host-terminal resize (e.g. a font size change) that arrives while a
    // pane is fullscreen must keep the active pane fullscreened, sized to the
    // new display dimensions.
    let initial_size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(initial_size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_fullscreen(1);
    assert!(
        tab.is_fullscreen_active(),
        "Tab is fullscreen before the resize"
    );

    let new_size = Size { cols: 80, rows: 30 };
    tab.resize_whole_tab(new_size).unwrap();

    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen is preserved across a host-terminal resize"
    );
    let active_pane = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(4))
        .expect("Active fullscreen pane is still present");
    assert_eq!(
        active_pane.cols(),
        new_size.cols,
        "Fullscreen pane cols match the new display cols"
    );
    assert_eq!(
        active_pane.rows(),
        new_size.rows,
        "Fullscreen pane rows match the new display rows"
    );
    assert_eq!(active_pane.x(), 0, "Fullscreen pane x is at viewport edge");
    assert_eq!(active_pane.y(), 0, "Fullscreen pane y is at viewport edge");
}

#[test]
pub fn resize_while_fullscreen_updates_hidden_pane_geometry() {
    // When a host-terminal resize arrives while a pane is fullscreen, every
    // hidden pane's geometry must be updated to match the new display area.
    // Otherwise their `inner` cell counts stay sized for the old display and
    // toggling fullscreen off hands the layout solver coordinates that
    // fall outside the viewport, producing layout-solve failures and a
    // corrupt render.
    //
    // The assertion here is the direct invariant: after the resize, every
    // pane that is currently hidden behind the fullscreen pane fits inside
    // the new display area.
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let new_size = Size { cols: 60, rows: 18 };
    let stacked_resize = false;
    let mut tab = create_new_tab(initial_size, stacked_resize);
    for i in 2..6 {
        tab.new_pane(
            PaneId::Terminal(i),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }

    let active_pane_id = tab
        .get_active_pane_id(1)
        .expect("an active pane exists before fullscreen");
    tab.toggle_active_pane_fullscreen(1);
    assert!(tab.is_fullscreen_active(), "Fullscreen is active");

    tab.resize_whole_tab(new_size).unwrap();

    // Collect the panes hidden by the fullscreen state and verify each one
    // already fits the new display area; if any extends beyond it, exiting
    // fullscreen would hand the layout solver an unsatisfiable layout.
    let hidden_pane_ids: Vec<PaneId> = tab
        .tiled_panes
        .panes
        .keys()
        .copied()
        .filter(|id| *id != active_pane_id && tab.tiled_panes.panes_to_hide_contains(*id))
        .collect();
    assert!(
        !hidden_pane_ids.is_empty(),
        "the test setup actually produced hidden panes"
    );
    for pane_id in hidden_pane_ids {
        let pane = tab.tiled_panes.panes.get(&pane_id).unwrap();
        let geom = pane.position_and_size();
        assert!(
            geom.x + geom.cols.as_usize() <= new_size.cols,
            "hidden pane {pane_id:?} fits horizontally after resize: \
             x={}, cols={}, display_cols={}",
            geom.x,
            geom.cols.as_usize(),
            new_size.cols,
        );
        assert!(
            geom.y + geom.rows.as_usize() <= new_size.rows,
            "hidden pane {pane_id:?} fits vertically after resize: \
             y={}, rows={}, display_rows={}",
            geom.y,
            geom.rows.as_usize(),
            new_size.rows,
        );
    }
}

#[test]
pub fn toggle_no_ui_fullscreen_covers_whole_display_and_restores() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(tab.is_fullscreen_active(), "NoUi fullscreen is active");
    assert!(
        tab.fullscreen_covers_ui(),
        "NoUi fullscreen covers the UI rows"
    );
    let active_pane = tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap();
    assert_eq!(active_pane.x(), 0, "Pane x is on display edge");
    assert_eq!(active_pane.y(), 0, "Pane y is on display edge");
    assert_eq!(active_pane.cols(), 121, "Pane cols match display cols");
    assert_eq!(active_pane.rows(), 20, "Pane rows match display rows");
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(!tab.is_fullscreen_active(), "NoUi fullscreen toggled off");
    assert!(
        !tab.fullscreen_covers_ui(),
        "NoUi flag cleared after toggle off"
    );
    let active_pane = tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap();
    assert_eq!(active_pane.x(), 61, "Pane x restored");
    assert_eq!(active_pane.y(), 10, "Pane y restored");
    assert_eq!(active_pane.cols(), 60, "Pane cols restored");
    assert_eq!(active_pane.rows(), 10, "Pane rows restored");
}

#[test]
pub fn regular_fullscreen_switches_to_no_ui_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_fullscreen(1);
    assert!(tab.is_fullscreen_active(), "Regular fullscreen is active");
    assert!(
        !tab.fullscreen_covers_ui(),
        "Regular fullscreen leaves the UI rows"
    );
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen stays active after switching kinds"
    );
    assert!(
        tab.fullscreen_covers_ui(),
        "Fullscreen switched to covering the UI rows"
    );
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        !tab.is_fullscreen_active(),
        "Fullscreen toggled off entirely"
    );
}

#[test]
pub fn no_ui_fullscreen_downgrades_to_regular_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    let viewport = *tab.viewport.borrow();
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        tab.is_fullscreen_active() && tab.fullscreen_covers_ui(),
        "NoUi fullscreen is active"
    );

    tab.toggle_active_pane_fullscreen(1);

    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen stays active after the downgrade"
    );
    assert!(
        !tab.fullscreen_covers_ui(),
        "The downgrade restores the UI rows"
    );
    assert_eq!(
        tab.fullscreen_pane_id(),
        Some(PaneId::Terminal(4)),
        "The same pane remains fullscreen"
    );
    let active_pane = tab.tiled_panes.panes.get(&PaneId::Terminal(4)).unwrap();
    assert_eq!(active_pane.x(), viewport.x, "Pane x matches the viewport");
    assert_eq!(active_pane.y(), viewport.y, "Pane y matches the viewport");
    assert_eq!(
        active_pane.cols(),
        viewport.cols,
        "Pane cols match the viewport cols"
    );
    assert_eq!(
        active_pane.rows(),
        viewport.rows,
        "Pane rows match the viewport rows"
    );

    tab.toggle_active_pane_fullscreen(1);
    assert!(
        !tab.is_fullscreen_active(),
        "The next regular toggle leaves fullscreen entirely"
    );
}

#[test]
pub fn resize_whole_tab_while_no_ui_fullscreen_preserves_no_ui() {
    let initial_size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(initial_size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(i);
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
    }
    tab.toggle_active_pane_no_ui_fullscreen(1);
    assert!(
        tab.fullscreen_covers_ui(),
        "NoUi fullscreen is active before the resize"
    );

    let new_size = Size { cols: 80, rows: 30 };
    tab.resize_whole_tab(new_size).unwrap();

    assert!(
        tab.is_fullscreen_active(),
        "Fullscreen is preserved across a host-terminal resize"
    );
    assert!(
        tab.fullscreen_covers_ui(),
        "NoUi kind is preserved across a host-terminal resize"
    );
    let active_pane = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(4))
        .expect("Active fullscreen pane is still present");
    assert_eq!(
        active_pane.cols(),
        new_size.cols,
        "NoUi fullscreen pane cols match the new display cols"
    );
    assert_eq!(
        active_pane.rows(),
        new_size.rows,
        "NoUi fullscreen pane rows match the new display rows"
    );
    assert_eq!(
        active_pane.x(),
        0,
        "NoUi fullscreen pane x is at display edge"
    );
    assert_eq!(
        active_pane.y(),
        0,
        "NoUi fullscreen pane y is at display edge"
    );
}
