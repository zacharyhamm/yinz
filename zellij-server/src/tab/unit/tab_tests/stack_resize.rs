use super::*;

/// Three panes in one stack: the stack cannot be laid out in fewer than three rows.
fn tab_with_a_three_pane_stack(size: Size) -> Tab {
    let stack = TiledPaneLayout {
        children_are_stacked: true,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children: vec![stack],
        ..Default::default()
    };
    create_new_tab_with_layout(size, initial_layout)
}

#[test]
fn a_resize_a_stack_cannot_fit_leaves_the_panes_where_they_were() {
    let size = Size { cols: 80, rows: 20 };
    let mut tab = tab_with_a_three_pane_stack(size);
    let before = tiled_pane_geoms(&tab);
    assert_eq!(
        before.len(),
        3,
        "the layout should have produced three panes"
    );
    assert!(
        before.iter().all(|(_, geom)| geom.is_stacked()),
        "every pane should be a member of the stack"
    );

    tab.tiled_panes.resize(Size { cols: 80, rows: 2 });

    assert_eq!(
        tiled_pane_geoms(&tab),
        before,
        "a refused layout must not move or resize any pane"
    );
    let viewport = *tab.viewport.borrow();
    let display_area = *tab.display_area.borrow();
    assert_eq!(
        display_area.rows, 2,
        "the display area describes the screen"
    );
    assert_eq!(
        viewport.rows, display_area.rows,
        "the viewport keeps tracking the display area even when the panes could not follow"
    );
}

#[test]
fn a_resize_after_a_refused_one_lays_the_stack_out_in_the_new_size() {
    let size = Size { cols: 80, rows: 20 };
    let mut tab = tab_with_a_three_pane_stack(size);

    tab.tiled_panes.resize(Size { cols: 80, rows: 2 });
    tab.tiled_panes.resize(Size { cols: 80, rows: 10 });

    let after = tiled_pane_geoms(&tab);
    let stack_rows: usize = after.iter().map(|(_, geom)| geom.rows.as_usize()).sum();
    assert_eq!(stack_rows, 10, "the stack fills the ten rows it was given");
    let viewport = *tab.viewport.borrow();
    assert_eq!(
        viewport.rows, 10,
        "the viewport must not carry the refused resize's delta into the next one"
    );
    assert_eq!(tab.display_area.borrow().rows, 10);
}
