use super::*;

#[test]
pub fn resize_down_with_pane_above() {
    // ┌───────────┐                  ┌───────────┐
    // │           │                  │           │
    // │           │                  │           │
    // ├───────────┤ ==resize=down==> │           │
    // │███████████│                  ├───────────┤
    // │███████████│                  │███████████│
    // │███████████│                  │███████████│
    // └───────────┘                  └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId(1), None, None)
        .unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .y,
        11,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "pane above row count"
    );
}

#[test]
pub fn resize_down_with_pane_below() {
    // ┌───────────┐                  ┌───────────┐
    // │███████████│                  │███████████│
    // │███████████│                  │███████████│
    // ├───────────┤ ==resize=down==> │███████████│
    // │           │                  ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane below x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .y,
        11,
        "pane below y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane below column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "pane below row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        11,
        "focused pane row count"
    );
}

#[test]
pub fn resize_down_with_panes_above_and_below() {
    // ┌───────────┐                  ┌───────────┐
    // │           │                  │           │
    // │           │                  │           │
    // ├───────────┤                  ├───────────┤
    // │███████████│ ==resize=down==> │███████████│
    // │███████████│                  │███████████│
    // │███████████│                  │███████████│
    // ├───────────┤                  │███████████│
    // │           │                  ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let first_pane_id = PaneId::Terminal(crate::TerminalId(1));
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId(3));
    tab.horizontal_split(new_pane_id_1, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .y,
        15,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        9,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane below x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .y,
        24,
        "pane below y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane below column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        6,
        "pane below row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane above row count"
    );
}

#[test]
pub fn resize_down_with_multiple_panes_above() {
    //
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // ├─────┴─────┤  ==resize=down==>  │     │     │
    // │███████████│                    ├─────┴─────┤
    // │███████████│                    │███████████│
    // └───────────┘                    └───────────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let first_pane_id = PaneId::Terminal(crate::TerminalId(1));
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId(3));
    tab.horizontal_split(new_pane_id_1, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .y,
        16,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_1)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "first pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "first pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&new_pane_id_2)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "first pane above row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "second pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "second pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&first_pane_id)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "second pane above row count"
    );
}

#[test]
pub fn resize_down_with_panes_above_aligned_left_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // │     │     │                    │     │     │
    // ├─────┼─────┤  ==resize=down==>  ├─────┤     │
    // │     │█████│                    │     ├─────┤
    // │     │█████│                    │     │█████│
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane
    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let pane_above_and_left = PaneId::Terminal(crate::TerminalId(1));
    let pane_to_the_left = PaneId::Terminal(crate::TerminalId(2));
    let focused_pane = PaneId::Terminal(crate::TerminalId(3));
    let pane_above = PaneId::Terminal(crate::TerminalId(4));
    tab.horizontal_split(pane_to_the_left, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.vertical_split(focused_pane, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(pane_above, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .y,
        16,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_left)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane above and to the left x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_left)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above and to the left y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_left)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane above and to the left column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_left)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane above and to the left row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane above row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane to the left x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane to the left y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane to the left column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane to the left row count"
    );
}

#[test]
pub fn resize_down_with_panes_below_aligned_left_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │█████│                    │     │█████│
    // │     │█████│                    │     │█████│
    // ├─────┼─────┤  ==resize=down==>  ├─────┤█████│
    // │     │     │                    │     ├─────┤
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let pane_to_the_left = PaneId::Terminal(crate::TerminalId(1));
    let pane_below_and_left = PaneId::Terminal(crate::TerminalId(2));
    let pane_below = PaneId::Terminal(crate::TerminalId(3));
    let focused_pane = PaneId::Terminal(crate::TerminalId(4));
    tab.horizontal_split(pane_below_and_left, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.vertical_split(pane_below, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(focused_pane, None, crate::ClientId(1), None, None)
        .unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane above and to the left x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above and to the left y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane above and to the left column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_left)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane above and to the left row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .y,
        16,
        "pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane above row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_left)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane to the left x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_left)
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane to the left y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_left)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane to the left column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_left)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane to the left row count"
    );
}

#[test]
pub fn resize_down_with_panes_above_aligned_right_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │     │     │                    │     │     │
    // │     │     │                    │     │     │
    // ├─────┼─────┤  ==resize=down==>  │     ├─────┤
    // │█████│     │                    ├─────┤     │
    // │█████│     │                    │█████│     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let pane_above = PaneId::Terminal(crate::TerminalId(1));
    let focused_pane = PaneId::Terminal(crate::TerminalId(2));
    let pane_to_the_right = PaneId::Terminal(crate::TerminalId(3));
    let pane_above_and_right = PaneId::Terminal(crate::TerminalId(4));
    tab.horizontal_split(focused_pane, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.vertical_split(pane_to_the_right, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(pane_above_and_right, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .y,
        16,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane above x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane above column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "pane above row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane to the right x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane to the right y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane to the right column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane to the right row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_right)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane above and to the right x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_right)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane above and to the right y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_right)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane above and to the right column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_above_and_right)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane above and to the right row count"
    );
}

#[test]
pub fn resize_down_with_panes_below_aligned_right_with_current_pane() {
    // ┌─────┬─────┐                    ┌─────┬─────┐
    // │█████│     │                    │█████│     │
    // │█████│     │                    │█████│     │
    // ├─────┼─────┤  ==resize=down==>  │█████├─────┤
    // │     │     │                    ├─────┤     │
    // │     │     │                    │     │     │
    // └─────┴─────┘                    └─────┴─────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 30,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let focused_pane = PaneId::Terminal(crate::TerminalId(1));
    let pane_below = PaneId::Terminal(crate::TerminalId(2));
    let pane_below_and_right = PaneId::Terminal(crate::TerminalId(3));
    let pane_to_the_right = PaneId::Terminal(crate::TerminalId(4));
    tab.horizontal_split(pane_below, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.vertical_split(pane_below_and_right, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(pane_to_the_right, None, crate::ClientId(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "focused pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "focused pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "focused pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&focused_pane)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        16,
        "focused pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .x,
        0,
        "pane below x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .y,
        16,
        "pane below y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "pane below column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        14,
        "pane below row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_right)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane below and to the right x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_right)
            .unwrap()
            .position_and_size()
            .y,
        15,
        "pane below and to the right y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_right)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane below and to the right column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_below_and_right)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane below and to the right row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .x,
        61,
        "pane to the right x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .y,
        0,
        "pane to the right y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "pane to the right column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&pane_to_the_right)
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        15,
        "pane to the right row count"
    );
}

#[test]
pub fn resize_down_with_panes_above_aligned_left_and_right_with_current_pane() {
    // ┌───┬───┬───┐                    ┌───┬───┬───┐
    // │   │   │   │                    │   │   │   │
    // │   │   │   │                    │   │   │   │
    // ├───┼───┼───┤  ==resize=down==>  ├───┤   ├───┤
    // │   │███│   │                    │   ├───┤   │
    // │   │███│   │                    │   │███│   │
    // └───┴───┴───┘                    └───┴───┴───┘
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
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

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
        16,
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
        30,
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
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
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
        30,
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
        30,
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
        16,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
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
        30,
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
        15,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_down_with_panes_below_aligned_left_and_right_with_current_pane() {
    // ┌───┬───┬───┐                    ┌───┬───┬───┐
    // │   │███│   │                    │   │███│   │
    // │   │███│   │                    │   │███│   │
    // ├───┼───┼───┤  ==resize=down==>  ├───┤███├───┤
    // │   │   │   │                    │   ├───┤   │
    // │   │   │   │                    │   │   │   │
    // └───┴───┴───┘                    └───┴───┴───┘
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
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

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
        16,
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
        30,
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
        91,
        "pane 4 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .y,
        15,
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
        30,
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
        30,
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
        16,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
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
        30,
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
        15,
        "pane 6 row count"
    );
}

#[test]
pub fn resize_down_with_panes_above_aligned_left_and_right_with_panes_to_the_left_and_right() {
    // ┌─┬───────┬─┐                    ┌─┬───────┬─┐
    // │ │       │ │                    │ │       │ │
    // │ │       │ │                    │ │       │ │
    // ├─┼─┬───┬─┼─┤  ==resize=down==>  ├─┤       ├─┤
    // │ │ │███│ │ │                    │ ├─┬───┬─┤ │
    // │ │ │███│ │ │                    │ │ │███│ │ │
    // └─┴─┴───┴─┴─┘                    └─┴─┴───┴─┴─┘
    // █ == focused pane

    let size = Size {
        cols: 122,
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
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
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
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(7)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(8)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

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
        60,
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
        60,
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
        60,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
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
        31,
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
        16,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
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
        31,
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

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 5 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .y,
        16,
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
        15,
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
        14,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 6 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .y,
        15,
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
        31,
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
        15,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .x,
        75,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .y,
        16,
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
        8,
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
        14,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .x,
        83,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .y,
        16,
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
        8,
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
        14,
        "pane 8 row count"
    );
}

#[test]
pub fn resize_down_with_panes_below_aligned_left_and_right_with_to_the_left_and_right() {
    // ┌─┬─┬───┬─┬─┐                    ┌─┬─┬───┬─┬─┐
    // │ │ │███│ │ │                    │ │ │███│ │ │
    // │ │ │███│ │ │                    │ │ │███│ │ │
    // ├─┼─┴───┴─┼─┤  ==resize=down==>  ├─┤ │███│ ├─┤
    // │ │       │ │                    │ ├─┴───┴─┤ │
    // │ │       │ │                    │ │       │ │
    // └─┴───────┴─┘                    └─┴───────┴─┘
    // █ == focused pane

    let size = Size {
        cols: 122,
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
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.vertical_split(
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
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(5)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(6)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(crate::ClientId(1)).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(7)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId(8)),
        None,
        crate::ClientId(1),
        None,
        None,
    )
    .unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab.move_focus_left(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

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
        60,
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
        60,
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
        60,
        "pane 3 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(3)))
            .unwrap()
            .position_and_size()
            .y,
        0,
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
        15,
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
        16,
        "pane 3 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(4)))
            .unwrap()
            .position_and_size()
            .x,
        91,
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
        31,
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

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(5)))
            .unwrap()
            .position_and_size()
            .x,
        75,
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
        8,
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
        16,
        "pane 5 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(6)))
            .unwrap()
            .position_and_size()
            .x,
        83,
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
        8,
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
        16,
        "pane 6 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .x,
        60,
        "pane 7 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(7)))
            .unwrap()
            .position_and_size()
            .y,
        16,
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
        31,
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
        14,
        "pane 7 row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .x,
        91,
        "pane 8 x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(8)))
            .unwrap()
            .position_and_size()
            .y,
        15,
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
        31,
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
        15,
        "pane 8 row count"
    );
}

#[test]
pub fn cannot_resize_down_when_pane_below_is_at_minimum_height() {
    // ┌───────────┐                  ┌───────────┐
    // │███████████│                  │███████████│
    // ├───────────┤ ==resize=down==> ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 10,
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
    tab.move_focus_up(crate::ClientId(1)).unwrap();
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        5,
        "pane 2 height stayed the same"
    );
}

#[test]
pub fn cannot_resize_down_when_pane_has_fixed_rows() {
    // ┌───────────┐                  ┌───────────┐
    // │███████████│                  │███████████│
    // ├───────────┤ ==resize=down==> ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 20,
    };

    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Horizontal;
    let mut fixed_child = TiledPaneLayout::default();
    fixed_child.split_size = Some(SplitSize::Fixed(10));
    initial_layout.children = vec![fixed_child, TiledPaneLayout::default()];
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(0)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "pane 1 height stayed the same"
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
        "pane 2 height stayed the same"
    );
}

#[test]
pub fn cannot_resize_down_when_pane_below_has_fixed_rows() {
    // ┌───────────┐                  ┌───────────┐
    // │███████████│                  │███████████│
    // ├───────────┤ ==resize=down==> ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 20,
    };

    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Horizontal;
    let mut fixed_child = TiledPaneLayout::default();
    fixed_child.split_size = Some(SplitSize::Fixed(10));
    initial_layout.children = vec![TiledPaneLayout::default(), fixed_child];
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab_resize_down(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(0)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "pane 1 height stayed the same"
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
        "pane 2 height stayed the same"
    );
}

#[test]
pub fn cannot_resize_up_when_pane_below_has_fixed_rows() {
    // ┌───────────┐                  ┌───────────┐
    // │███████████│                  │███████████│
    // ├───────────┤ ==resize=down==> ├───────────┤
    // │           │                  │           │
    // └───────────┘                  └───────────┘
    // █ == focused pane

    let size = Size {
        cols: 121,
        rows: 20,
    };

    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Horizontal;
    let mut fixed_child = TiledPaneLayout::default();
    fixed_child.split_size = Some(SplitSize::Fixed(10));
    initial_layout.children = vec![TiledPaneLayout::default(), fixed_child];
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab_resize_up(&mut tab, crate::ClientId(1));

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId(0)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "pane 1 height stayed the same"
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
        "pane 2 height stayed the same"
    );
}
