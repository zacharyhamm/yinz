use super::*;

#[test]
pub fn resize_left_with_pane_to_the_left() {
    // ┌─────┬─────┐                    ┌───┬───────┐
    // │     │█████│                    │   │███████│
    // │     │█████│  ==resize=left==>  │   │███████│
    // │     │█████│                    │   │███████│
    // └─────┴─────┘                    └───┴───────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_left_with_pane_to_the_right() {
    // ┌─────┬─────┐                    ┌───┬───────┐
    // │█████│     │                    │███│       │
    // │█████│     │  ==resize=left==>  │███│       │
    // │█████│     │                    │███│       │
    // └─────┴─────┘                    └───┴───────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_left_and_right() {
    // ┌─────┬─────┬─────┐                    ┌─────┬───┬───────┐
    // │     │█████│     │                    │     │███│       │
    // │     │█████│     │  ==resize=left==>  │     │███│       │
    // │     │█████│     │                    │     │███│       │
    // └─────┴─────┴─────┘                    └─────┴───┴───────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        36,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        90,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_left_with_multiple_panes_to_the_left() {
    // ┌─────┬─────┐                    ┌───┬───────┐
    // │     │█████│                    │   │███████│
    // ├─────┤█████│  ==resize=left==>  ├───┤███████│
    // │     │█████│                    │   │███████│
    // └─────┴─────┘                    └───┴───────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_left_aligned_top_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // ├─────┼─────┤  ==resize=left==>  ├───┬─┴─────┤
    // │     │█████│                    │   │███████│
    // └─────┴─────┘                    └───┴───────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_right_aligned_top_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // ├─────┼─────┤  ==resize=left==>  ├───┬─┴─────┤
    // │█████│     │                    │███│       │
    // └─────┴─────┘                    └───┴───────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_left_aligned_bottom_with_current_pane() {
    // ┌─────┬─────┐                    ┌───┬───────┐
    // │     │█████│                    │   │███████│
    // ├─────┼─────┤  ==resize=left==>  ├───┴─┬─────┤
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_right_aligned_bottom_with_current_pane() {
    // ┌─────┬─────┐                    ┌───┬───────┐
    // │█████│     │                    │███│       │
    // ├─────┼─────┤  ==resize=left==>  ├───┴─┬─────┤
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_left_aligned_top_and_bottom_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // ├─────┼─────┤                    ├───┬─┴─────┤
    // │     │█████│  ==resize=left==>  │   │███████│
    // ├─────┼─────┤                    ├───┴─┬─────┤
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_right_aligned_top_and_bottom_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // ├─────┼─────┤                    ├───┬─┴─────┤
    // │█████│     │  ==resize=left==>  │███│       │
    // ├─────┼─────┤                    ├───┴─┬─────┤
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_left_aligned_top_and_bottom_with_panes_above_and_below() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // ├─────┼─────┤                    ├───┬─┴─────┤
    // │     ├─────┤                    │   ├───────┤
    // │     │█████│  ==resize=left==>  │   │███████│
    // │     ├─────┤                    │   ├───────┤
    // ├─────┼─────┤                    ├───┴─┬─────┤
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 70,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(7)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(8)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        35,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        35,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        21,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        56,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        56,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        35,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .y,
        35,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .y,
        46,
        "pane 7 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 7 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .y,
        51,
        "pane 8 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 8 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 8 row count"
    );
}

#[test]
pub fn resize_left_with_panes_to_the_right_aligned_top_and_bottom_with_panes_above_and_below() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // ├─────┼─────┤                    ├───┬─┴─────┤
    // ├─────┤     │                    ├───┤       │
    // │█████│     │  ==resize=left==>  │███│       │
    // ├─────┤     │                    ├───┤       │
    // ├─────┼─────┤                    ├───┴─┬─────┤
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 70,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(7)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(8)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        35,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .y,
        35,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        56,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        56,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        35,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .y,
        35,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        21,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .y,
        46,
        "pane 7 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 7 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .y,
        51,
        "pane 8 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        54,
        "pane 8 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 8 row count"
    );
}

#[test]
pub fn cannot_resize_left_when_pane_to_the_left_is_at_minimum_width() {
    // ┌─┬─┐                    ┌─┬─┐
    // │ │█│                    │ │█│
    // │ │█│  ==resize=left==>  │ │█│
    // │ │█│                    │ │█│
    // └─┴─┘                    └─┴─┘
    // █ == focused pane

    let size = Size { cols: 10, rows: 20 };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_left(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        5,
        "pane 1 columns stayed the same"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        5,
        "pane 2 columns stayed the same"
    );
}
