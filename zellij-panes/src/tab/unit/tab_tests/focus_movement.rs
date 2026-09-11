use super::*;

#[test]
pub fn move_focus_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane is the bottom one"
    );
}

#[test]
pub fn move_focus_down_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane is the top one"
    );
}

#[test]
pub fn move_focus_up_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane is the left one"
    );
}

#[test]
pub fn move_focus_left_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane x position"
    );
}

#[test]
pub fn move_focus_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane is the right one"
    );
}

#[test]
pub fn move_focus_right_to_the_most_recently_used_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane x position"
    );
}

#[test]
pub fn move_active_pane_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_down(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane is the bottom one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane is the bottom one"
    );
}

#[test]
pub fn move_active_pane_down_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_down(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        10,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_active_pane_up(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane is the top one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane is the top one"
    );
}

#[test]
pub fn move_active_pane_up_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.horizontal_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_up(crate::ClientId::new(1)).unwrap();
    tab.vertical_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_down(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_up(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        0,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        91,
        "Active pane x position"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_active_pane_left(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane is the left one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane is the left one"
    );
}

#[test]
pub fn move_active_pane_left_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_right(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_left(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        0,
        "Active pane x position"
    );

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "Active pane PaneId"
    );
}

#[test]
pub fn move_active_pane_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_right(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane is the right one"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane is the right one"
    );
}

#[test]
pub fn move_active_pane_right_to_the_most_recently_used_position() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.vertical_split(new_pane_id_1, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_2, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.horizontal_split(new_pane_id_3, None, crate::ClientId::new(1), None, None)
        .unwrap();
    tab.move_focus_left(crate::ClientId::new(1)).unwrap();
    tab.move_active_pane_right(crate::ClientId::new(1));

    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().y(),
        15,
        "Active pane y position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().x(),
        61,
        "Active pane x position"
    );
    assert_eq!(
        tab.get_active_pane(crate::ClientId::new(1)).unwrap().pid(),
        PaneId::Terminal(crate::TerminalId::new(1)),
        "Active pane Paneid"
    );
}
