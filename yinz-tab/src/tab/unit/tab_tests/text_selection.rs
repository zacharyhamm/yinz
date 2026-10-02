use super::*;

// a keyboard text selection belongs to the pane it was started on: changing focus while
// Select mode is open used to leave that pane's cursor pinned to the selection forever,
// because the mode-exit finished the selection on whichever pane was active by then.
#[test]
pub fn a_keyboard_selection_is_finished_on_the_pane_it_started_on() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, true);
    let first_pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let second_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(second_pane_id, None, None, client_id, None, None)
        .unwrap();
    tab.move_focus_up(client_id).unwrap();

    let pty_cursor = tab
        .get_pane_with_id(first_pane_id)
        .unwrap()
        .cursor_coordinates(Some(client_id));

    tab.start_text_selection(client_id);
    tab.move_text_selection(Direction::Down, client_id);
    let selection_cursor = tab
        .get_pane_with_id(first_pane_id)
        .unwrap()
        .cursor_coordinates(Some(client_id));
    assert_ne!(
        selection_cursor, pty_cursor,
        "the cursor marks the end being moved while the selection is live"
    );

    let second_pane_pty_cursor = tab
        .get_pane_with_id(second_pane_id)
        .unwrap()
        .cursor_coordinates(Some(client_id));

    tab.move_focus_down(client_id).unwrap();
    tab.move_text_selection(Direction::Down, client_id);
    assert_eq!(
        tab.get_pane_with_id(second_pane_id)
            .unwrap()
            .cursor_coordinates(Some(client_id)),
        second_pane_pty_cursor,
        "movement keeps going to the pane the selection started on"
    );

    tab.finish_text_selection(client_id);
    assert_eq!(
        tab.get_pane_with_id(first_pane_id)
            .unwrap()
            .cursor_coordinates(Some(client_id)),
        pty_cursor,
        "the pane the selection started on gets its cursor back"
    );
}
