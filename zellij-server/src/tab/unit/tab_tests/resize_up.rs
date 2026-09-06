use super::*;

#[test]
pub fn resize_up_with_pane_above() {
    // ┌───────────┐                ┌───────────┐
    // │           │                │           │
    // │           │                ├───────────┤
    // ├───────────┤ ==resize=up==> │███████████│
    // │███████████│                │███████████│
    // │███████████│                │███████████│
    // └───────────┘                └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        9,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_up_with_pane_below() {
    // ┌───────────┐                ┌───────────┐
    // │███████████│                │███████████│
    // │███████████│                ├───────────┤
    // ├───────────┤ ==resize=up==> │           │
    // │           │                │           │
    // │           │                │           │
    // └───────────┘                └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        9,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "pane 2 row count"
    );
}

#[test]
pub fn resize_up_with_panes_above_and_below() {
    // ┌───────────┐                ┌───────────┐
    // │           │                │           │
    // │           │                ├───────────┤
    // ├───────────┤                │███████████│
    // │███████████│ ==resize=up==> │███████████│
    // │███████████│                │███████████│
    // ├───────────┤                ├───────────┤
    // │           │                │           │
    // │           │                │           │
    // └───────────┘                └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        13,
        "pane 1 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        13,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        22,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        8,
        "pane 3 row count"
    );
}

#[test]
pub fn resize_up_with_multiple_panes_above() {
    //
    // ┌─────┬─────┐                 ┌─────┬─────┐
    // │     │     │                 ├─────┴─────┤
    // ├─────┴─────┤  ==resize=up==> │███████████│
    // │███████████│                 │███████████│
    // └───────────┘                 └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 3 row count"
    );
}

#[test]
pub fn resize_up_with_panes_above_aligned_left_with_current_pane() {
    // ┌─────┬─────┐                  ┌─────┬─────┐
    // │     │     │                  │     ├─────┤
    // ├─────┼─────┤  ==resize=up==>  ├─────┤█████│
    // │     │█████│                  │     │█████│
    // └─────┴─────┘                  └─────┴─────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_up_with_panes_below_aligned_left_with_current_pane() {
    // ┌─────┬─────┐                  ┌─────┬─────┐
    // │     │█████│                  │     │█████│
    // │     │█████│                  │     ├─────┤
    // ├─────┼─────┤  ==resize=up==>  ├─────┤     │
    // │     │     │                  │     │     │
    // │     │     │                  │     │     │
    // └─────┴─────┘                  └─────┴─────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_up_with_panes_above_aligned_right_with_current_pane() {
    // ┌─────┬─────┐                  ┌─────┬─────┐
    // │     │     │                  │     │     │
    // │     │     │                  ├─────┤     │
    // ├─────┼─────┤  ==resize=up==>  │█████├─────┤
    // │█████│     │                  │█████│     │
    // │█████│     │                  │█████│     │
    // └─────┴─────┘                  └─────┴─────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_up_with_panes_below_aligned_right_with_current_pane() {
    // ┌─────┬─────┐                  ┌─────┬─────┐
    // │█████│     │                  │█████│     │
    // │█████│     │                  ├─────┤     │
    // ├─────┼─────┤  ==resize=up==>  │     ├─────┤
    // │     │     │                  │     │     │
    // │     │     │                  │     │     │
    // └─────┴─────┘                  └─────┴─────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 2 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );
}

#[test]
pub fn resize_up_with_panes_above_aligned_left_and_right_with_current_pane() {
    // ┌───┬───┬───┐                  ┌───┬───┬───┐
    // │   │   │   │                  │   │   │   │
    // │   │   │   │                  │   ├───┤   │
    // ├───┼───┼───┤  ==resize=up==>  ├───┤███├───┤
    // │   │███│   │                  │   │███│   │
    // │   │███│   │                  │   │███│   │
    // └───┴───┴───┘                  └───┴───┴───┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(6)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_up_with_panes_below_aligned_left_and_right_with_current_pane() {
    // ┌───┬───┬───┐                  ┌───┬───┬───┐
    // │   │███│   │                  │   │███│   │
    // │   │███│   │                  │   ├───┤   │
    // ├───┼───┼───┤  ==resize=up==>  ├───┤   ├───┤
    // │   │   │   │                  │   │   │   │
    // │   │   │   │                  │   │   │   │
    // └───┴───┴───┘                  └───┴───┴───┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(6)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        30,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_up_with_panes_above_aligned_left_and_right_with_panes_to_the_left_and_right() {
    // ┌─┬───────┬─┐                  ┌─┬───────┬─┐
    // │ │       │ │                  │ │       │ │
    // │ │       │ │                  │ ├─┬───┬─┤ │
    // ├─┼─┬───┬─┼─┤  ==resize=up==>  ├─┤ │███│ ├─┤
    // │ │ │███│ │ │                  │ │ │███│ │ │
    // │ │ │███│ │ │                  │ │ │███│ │ │
    // └─┴─┴───┴─┴─┘                  └─┴─┴───┴─┴─┘
    // █ == focused pane
    let size = Size {
        cols: 122,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(6)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(7)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(8)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        75,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 7 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        8,
        "pane 7 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .x,
        83,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 8 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        8,
        "pane 8 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 8 row count"
    );
}

#[test]
pub fn resize_up_with_panes_below_aligned_left_and_right_with_to_the_left_and_right() {
    // ┌─┬─┬───┬─┬─┐                  ┌─┬─┬───┬─┬─┐
    // │ │ │███│ │ │                  │ │ │███│ │ │
    // │ │ │███│ │ │                  │ ├─┴───┴─┤ │
    // ├─┼─┴───┴─┼─┤  ==resize=up==>  ├─┤       ├─┤
    // │ │       │ │                  │ │       │ │
    // │ │       │ │                  │ │       │ │
    // └─┴───────┴─┘                  └─┴───────┴─┘
    // █ == focused pane
    let size = Size {
        cols: 122,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(6)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(7)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(8)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_up(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 1 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 1 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 1 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane 2 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 3 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        15,
        "pane 3 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 4 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 4 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 4 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .y,
        14,
        "pane 5 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 5 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(5)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane 6 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        31,
        "pane 6 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(6)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .x,
        75,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 7 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        8,
        "pane 7 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(7)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .x,
        83,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 8 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        8,
        "pane 8 column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(8)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane 8 row count"
    );
}

#[test]
pub fn cannot_resize_up_when_pane_above_is_at_minimum_height() {
    // ┌───────────┐                ┌───────────┐
    // │           │                │           │
    // ├───────────┤ ==resize=up==> ├───────────┤
    // │███████████│                │███████████│
    // └───────────┘                └───────────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 10,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_down(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 1 height stayed the same"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 2 height stayed the same"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_1_pane() {
    let size = Size {
        cols: 121,
        rows: 10,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "There is only 1 pane so both coordinates should be 0"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "There is only 1 pane so both coordinates should be 0"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_1_pane_with_stacked_resizes() {
    let size = Size {
        cols: 121,
        rows: 10,
    };
    let stacked_resize = true; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "There is only 1 pane so both coordinates should be 0"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "There is only 1 pane so both coordinates should be 0"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_1_pane_to_left() {
    let size = Size {
        cols: 121,
        rows: 10,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    // should behave like `resize_left_with_pane_to_the_left`
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_2_panes_to_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    // should behave like `resize_left_with_multiple_panes_to_the_left`
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        54,
        "pane 2 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane 2 y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "pane 2 row count"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_1_pane_to_right_1_pane_above() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        9,
        "Pane 3 y coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "Pane 3 x coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "Pane 3 row count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        67,
        "Pane 3 col count"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_1_pane_to_right_1_pane_to_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "Pane 3 y coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "Pane 3 x coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "Pane 3 row count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        36,
        "Pane 3 col count"
    );
}

#[test]
pub fn nondirectional_resize_increase_with_pane_above_aligned_right_with_current_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab_resize_increase(&mut tab, crate::ClientId::new(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "Pane 3 y coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "Pane 3 x coordinate"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "Pane 3 row count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        36,
        "Pane 3 col count"
    );
}

#[test]
pub fn custom_cursor_height_width_ratio() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let character_cell_size = Rc::new(RefCell::new(None));
    let tab = create_new_tab_with_cell_size(size, character_cell_size.clone());
    let initial_cursor_height_width_ratio = tab.tiled_panes.cursor_height_width_ratio();
    *character_cell_size.borrow_mut() = Some(SizeInPixels {
        height: 10,
        width: 4,
    });
    let cursor_height_width_ratio_after_update = tab.tiled_panes.cursor_height_width_ratio();
    assert_eq!(
        initial_cursor_height_width_ratio, None,
        "initially no ratio "
    );
    assert_eq!(
        cursor_height_width_ratio_after_update,
        Some(3),
        "ratio updated successfully"
    ); // 10 / 4 == 2.5, rounded: 3
}
