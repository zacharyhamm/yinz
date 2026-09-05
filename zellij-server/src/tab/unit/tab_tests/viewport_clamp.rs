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
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId(0)), false);
    tab.set_pane_selectable(PaneId::Terminal(crate::TerminalId(2)), false);
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
