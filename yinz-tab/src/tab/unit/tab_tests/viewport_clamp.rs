use super::*;

/// A tab shaped like the shipped `classic.kdl`: one reserved row on top, two on the bottom, and
/// one selectable pane between them. Marking the edge panes non-selectable is what makes them ui
/// panes, so `LayoutApplier::offset_viewport` offsets them out of the viewport exactly as it does
/// the real tab bar and status bar.
fn tab_with_three_rows_of_chrome(size: Size) -> Tab {
    let initial_layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                split_size: Some(SplitSize::Fixed(1)),
                ..Default::default()
            },
            TiledPaneLayout::default(),
            TiledPaneLayout {
                split_size: Some(SplitSize::Fixed(2)),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId::new(0)), false);
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId::new(2)), false);
    tab
}

/// The viewport is the screen minus what the ui panes reserve on each edge, so it is smaller than
/// the display area whenever chrome is up. Carrying the resize's delta across it used to subtract
/// past zero and wrap around `usize`, and `set_pane_frames` - which `resize` calls on its way out
/// - then overflowed on `viewport.y + viewport.rows` under the default title-only frames.
#[test]
fn a_screen_smaller_than_the_chrome_leaves_the_viewport_inside_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut tab = tab_with_three_rows_of_chrome(size);
    tab.tiled_panes.set_pane_frames(PaneFrameStyle::Titles);
    assert_eq!(
        *tab.viewport.borrow(),
        Viewport {
            x: 0,
            y: 1,
            cols: 80,
            rows: 17,
        },
        "the chrome should have taken three of the twenty rows"
    );

    tab.tiled_panes.resize(Size { cols: 80, rows: 2 });

    let viewport = *tab.viewport.borrow();
    assert!(
        viewport.y + viewport.rows <= 2,
        "the viewport must stay inside the screen it describes: {viewport:?}"
    );
    assert!(
        viewport.x + viewport.cols <= 80,
        "the viewport must stay inside the screen it describes: {viewport:?}"
    );
    assert_eq!(*tab.display_area.borrow(), Size { cols: 80, rows: 2 });
}

/// The insets survive a resize the screen has room for: that is the arithmetic the signed delta
/// used to do, and the callers that resize to the current display area (`relayout_tiled_panes`,
/// `LayoutApplier::adjust_viewport`) depend on it being exact rather than merely clamped.
#[test]
fn a_resize_the_screen_has_room_for_keeps_the_chrome_insets() {
    let size = Size { cols: 80, rows: 20 };
    let mut tab = tab_with_three_rows_of_chrome(size);
    tab.tiled_panes.set_pane_frames(PaneFrameStyle::Titles);

    tab.tiled_panes.resize(Size { cols: 80, rows: 10 });
    let expected = Viewport {
        x: 0,
        y: 1,
        cols: 80,
        rows: 7,
    };
    assert_eq!(
        *tab.viewport.borrow(),
        expected,
        "one row of chrome on top and two on the bottom, in a ten-row screen"
    );

    let display_area = *tab.display_area.borrow();
    tab.tiled_panes.resize(display_area);
    assert_eq!(
        *tab.viewport.borrow(),
        expected,
        "resizing to the size the screen already is must not move the viewport"
    );
}

/// What a warp of a tab's last content pane leaves behind: chrome only, outside the
/// viewport. A pane put back takes the whole viewport rather than splitting chrome.
#[test]
fn a_pane_added_beside_only_chrome_fills_the_viewport() {
    let mut tab = tab_with_three_rows_of_chrome(Size { cols: 80, rows: 20 });
    let content = PaneId::Terminal(crate::TerminalId::new(1));
    let pane = tab.extract_pane(content, true).expect("content pane");
    let viewport = *tab.viewport.borrow();
    assert!(tab.tiled_panes.has_room_for_new_pane());

    assert!(tab.tiled_panes.insert_pane(content, pane, None).is_none());
    let geom = tab.tiled_panes.get_pane(content).unwrap().current_geom();
    assert_eq!(
        (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize()),
        (viewport.x, viewport.y, viewport.cols, viewport.rows)
    );
}

#[test]
fn a_pane_added_to_an_empty_tab_fills_the_viewport() {
    let size = Size { cols: 80, rows: 20 };
    let mut tab = create_new_tab_with_layout(size, TiledPaneLayout::default());
    let content = PaneId::Terminal(crate::TerminalId::new(0));
    let mut pane = tab.extract_pane(content, true).expect("content pane");
    // Adopted panes start with default geometry, unlike a pane moved within a session.
    pane.set_geom(PaneGeom::default());

    assert!(tab.tiled_panes.insert_pane(content, pane, None).is_none());
    let geom = tab.tiled_panes.get_pane(content).unwrap().current_geom();
    assert_eq!(
        (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize()),
        (0, 0, size.cols, size.rows)
    );
}

/// Chrome that overlaps the viewport is not "nothing there": the shortcut must not lay a
/// pane over it.
#[test]
fn chrome_overlapping_the_viewport_does_not_take_the_full_viewport_shortcut() {
    let mut tab = tab_with_three_rows_of_chrome(Size { cols: 80, rows: 20 });
    let content = PaneId::Terminal(crate::TerminalId::new(1));
    let pane = tab.extract_pane(content, true).expect("content pane");
    let viewport = *tab.viewport.borrow();
    let status_bar = PaneId::Terminal(crate::TerminalId::new(2));
    let status = tab.tiled_panes.get_pane_mut(status_bar).unwrap();
    let mut geom = status.current_geom();
    geom.y = viewport.y + 5;
    status.set_geom(geom);

    // The shortcut always places the pane over the whole viewport. Without it, the
    // only pane left is one row of chrome, which is too small to split, so the
    // pane is handed back.
    assert!(
        tab.tiled_panes.insert_pane(content, pane, None).is_some(),
        "the pane must not be laid over the chrome"
    );
    assert!(tab.tiled_panes.get_pane(content).is_none());
}
