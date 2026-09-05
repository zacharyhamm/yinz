use super::*;

#[test]
fn open_new_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(screen.tabs.len(), 2, "Screen now has two tabs");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab switched to new tab"
    );
}

#[test]
pub fn switch_to_prev_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab switched to previous tab"
    );
}

#[test]
pub fn switch_to_next_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    screen
        .switch_tab_next(None, true, crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab switched to next tab"
    );
}

#[test]
pub fn switch_to_tab_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);

    assert!(
        !screen
            .switch_active_tab_name("Tab #1".to_string(), crate::ClientId(1))
            .expect("TEST"),
        "Active tab switched to tab by name"
    );
    assert!(
        screen
            .switch_active_tab_name("Tab #2".to_string(), crate::ClientId(1))
            .expect("TEST"),
        "Active tab switched to tab by name"
    );
    assert!(
        screen
            .switch_active_tab_name("Tab #3".to_string(), crate::ClientId(1))
            .expect("TEST"),
        "Active tab switched to tab by name"
    );
}

#[test]
pub fn close_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.close_tab(crate::ClientId(1)).expect("TEST");

    assert_eq!(screen.tabs.len(), 1, "Only one tab left");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab switched to previous tab"
    );
}

#[test]
pub fn close_the_middle_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    screen.close_tab(crate::ClientId(1)).expect("TEST");

    assert_eq!(screen.tabs.len(), 2, "Two tabs left");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab switched to previous tab"
    );
}

#[test]
fn move_focus_left_at_left_screen_edge_changes_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab switched to previous"
    );
}

#[test]
fn basic_move_of_active_tab_to_left() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1
    );

    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

#[test]
fn move_of_active_tab_to_left_when_there_is_only_one_tab() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0
    );

    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

#[test]
fn move_of_active_tab_to_left_multiple_times() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2
    );

    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");
    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab moved to left twice"
    );
}

#[test]
fn wrapping_move_of_active_tab_to_left() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0
    );

    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab moved to left until wrapped around"
    );
}

#[test]
fn basic_move_of_active_tab_to_right() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0
    );

    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab moved to right"
    );
}

#[test]
fn move_of_active_tab_to_right_when_there_is_only_one_tab() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0
    );

    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

#[test]
fn move_of_active_tab_to_right_multiple_times() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");
    screen
        .move_focus_left_or_previous_tab(crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0
    );

    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");
    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab moved to right twice"
    );
}

#[test]
fn wrapping_move_of_active_tab_to_right() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2
    );

    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab moved to right until wrapped around"
    );
}

#[test]
fn tab_id_remains_stable_after_switch() {
    // Test that tab IDs remain stable when switching tabs, only positions change
    let mut screen = create_fixed_size_screen();

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    // Verify initial state: IDs should be 0, 1, 2
    let initial_tab_ids: Vec<usize> = screen.tabs.keys().copied().collect();
    assert_eq!(
        initial_tab_ids,
        vec![0, 1, 2],
        "Initial tab IDs should be 0, 1, 2"
    );

    // Verify initial positions match IDs
    assert_eq!(screen.tabs.get(&0).unwrap().id, 0);
    assert_eq!(screen.tabs.get(&0).unwrap().position, 0);
    assert_eq!(screen.tabs.get(&1).unwrap().id, 1);
    assert_eq!(screen.tabs.get(&1).unwrap().position, 1);
    assert_eq!(screen.tabs.get(&2).unwrap().id, 2);
    assert_eq!(screen.tabs.get(&2).unwrap().position, 2);

    // Move active tab (position 2, ID 2) to right, which wraps to position 0 and rotates the
    // others right by one (IDs stay stable, only positions change)
    screen
        .move_active_tab_to_right(crate::ClientId(1))
        .expect("TEST");

    // Verify BTreeMap keys (IDs) remain unchanged
    let after_switch_tab_ids: Vec<usize> = screen.tabs.keys().copied().collect();
    assert_eq!(
        after_switch_tab_ids,
        vec![0, 1, 2],
        "Tab IDs in BTreeMap should remain 0, 1, 2 after switch"
    );

    // Verify IDs remain stable but positions rotate
    // Tab 0: was at position 0, rotated to position 1
    assert_eq!(
        screen.tabs.get(&0).unwrap().id,
        0,
        "Tab with ID 0 should still have ID 0"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().position,
        1,
        "Tab with ID 0 should now be at position 1"
    );

    // Tab 1: was at position 1, rotated to position 2
    assert_eq!(
        screen.tabs.get(&1).unwrap().id,
        1,
        "Tab with ID 1 should still have ID 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().position,
        2,
        "Tab with ID 1 should now be at position 2"
    );

    // Tab 2: was at position 2, rotated over the edge to position 0
    assert_eq!(
        screen.tabs.get(&2).unwrap().id,
        2,
        "Tab with ID 2 should still have ID 2"
    );
    assert_eq!(
        screen.tabs.get(&2).unwrap().position,
        0,
        "Tab with ID 2 should now be at position 0"
    );

    // Verify that lookup by position works correctly after the rotation
    let tab_at_pos_0 = screen.tabs.values().find(|t| t.position == 0).unwrap();
    assert_eq!(tab_at_pos_0.id, 2, "Tab at position 0 should have ID 2");

    let tab_at_pos_1 = screen.tabs.values().find(|t| t.position == 1).unwrap();
    assert_eq!(tab_at_pos_1.id, 0, "Tab at position 1 should have ID 0");

    let tab_at_pos_2 = screen.tabs.values().find(|t| t.position == 2).unwrap();
    assert_eq!(tab_at_pos_2.id, 1, "Tab at position 2 should have ID 1");
}

#[test]
fn move_focus_right_at_right_screen_edge_changes_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    screen
        .move_focus_right_or_next_tab(crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab switched to next"
    );
}

#[test]
pub fn toggle_to_previous_tab_simple() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.go_to_tab(1, crate::ClientId(1)).expect("TEST");
    screen.go_to_tab(2, crate::ClientId(1)).expect("TEST");

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
}

#[test]
pub fn toggle_to_previous_tab_create_tabs_only() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1],
        "Tab history is invalid"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 2],
        "Tab history is invalid"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1],
        "Tab history is invalid"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
}

#[test]
pub fn toggle_to_previous_tab_delete() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    new_tab(&mut screen, 4, 3);

    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        3,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        3,
        "Active tab toggler to previous tab"
    );

    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 1, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 3, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );

    screen.close_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(crate::ClientId(1)).expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&ClientId(1)).unwrap(),
        &[0, 2],
        "Tab history is invalid"
    );
}

#[test]
fn switch_to_tab_with_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    {
        let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(crate::TerminalId(2)),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(crate::ClientId(1)),
                None,
            )
            .unwrap();
        active_tab.toggle_active_pane_fullscreen(crate::ClientId(1));
    }
    new_tab(&mut screen, 2, 2);

    screen
        .switch_tab_prev(None, true, crate::ClientId(1))
        .expect("TEST");

    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        0,
        "Active tab switched to previous"
    );
    assert_eq!(
        screen
            .get_active_tab(crate::ClientId(1))
            .unwrap()
            .get_active_pane_id(crate::ClientId(1))
            .unwrap(),
        PaneId::Terminal(crate::TerminalId(2)),
        "Active pane is still the fullscreen pane"
    );
}

#[test]
fn update_screen_pixel_dimensions() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let initial_pixel_dimensions = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        crate::ClientId(1),
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5,
            }),
            text_area_size: None,
        },
    );
    let pixel_dimensions_after_first_update = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        crate::ClientId(1),
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
    );
    let pixel_dimensions_after_second_update = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        crate::ClientId(1),
        PixelDimensions {
            character_cell_size: None,
            text_area_size: None,
        },
    );
    let pixel_dimensions_after_third_update = screen.pixel_dimensions;
    assert_eq!(
        initial_pixel_dimensions,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: None
        },
        "Initial pixel dimensions empty"
    );
    assert_eq!(
        pixel_dimensions_after_first_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: None
        },
        "character_cell_size updated properly",
    );
    assert_eq!(
        pixel_dimensions_after_second_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
        "text_area_size updated properly without overriding character_cell_size",
    );
    assert_eq!(
        pixel_dimensions_after_third_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
        "empty update does not delete existing data",
    );
}

#[test]
fn character_cell_size_is_derived_from_the_reporting_client_size() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(size, true, true);
    let client_id = ClientId(2);
    screen.set_client_size(
        client_id,
        Size {
            cols: 100,
            rows: 50,
        },
    );

    screen.update_pixel_dimensions(
        client_id,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 1050,
                width: 900,
            }),
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            height: 21,
            width: 9
        }),
        "The reported text area is divided by the grid size of the client that reported it"
    );
}

#[test]
fn character_cell_size_is_not_derived_for_a_client_of_unknown_size() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(size, true, true);

    screen.update_pixel_dimensions(
        crate::ClientId(2),
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 1050,
                width: 900,
            }),
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        None,
        "Without the reporting client's grid size the pixel report cannot be interpreted"
    );
}

#[test]
fn attach_after_first_tab_closed() {
    // ensure https://github.com/zellij-org/zellij/issues/1645 is fixed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    {
        let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(crate::TerminalId(2)),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(crate::ClientId(1)),
                None,
            )
            .unwrap();
        active_tab.toggle_active_pane_fullscreen(crate::ClientId(1));
    }
    new_tab(&mut screen, 2, 1);

    screen.close_tab_by_id(0).expect("TEST");
    screen.remove_client(crate::ClientId(1)).expect("TEST");
    screen.add_client(crate::ClientId(1)).expect("TEST");
}

#[test]
fn layout_metadata_lists_tabs_in_position_order() {
    // Tabs are keyed by creation id, but a user who reorders them expects a
    // dumped or resurrected layout to come back in the order they arranged.
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    screen.tabs.get_mut(&0).expect("TEST").name = "first".to_string();
    new_tab(&mut screen, 2, 1);
    screen.tabs.get_mut(&1).expect("TEST").name = "second".to_string();
    new_tab(&mut screen, 3, 2);
    screen.tabs.get_mut(&2).expect("TEST").name = "third".to_string();

    // the active tab is the third one; moving it left makes the positions
    // first, third, second while the ids stay 0, 1, 2
    screen
        .move_active_tab_to_left(crate::ClientId(1))
        .expect("TEST");
    assert_eq!(
        screen.get_active_tab(crate::ClientId(1)).unwrap().position,
        1
    );

    let layout_metadata = screen.get_layout_metadata(None, None).to_layout_metadata();
    let tab_names: Vec<Option<String>> = layout_metadata
        .tabs
        .iter()
        .map(|tab| tab.name.clone())
        .collect();
    assert_eq!(
        tab_names,
        vec![
            Some("first".to_string()),
            Some("third".to_string()),
            Some("second".to_string()),
        ],
        "the dumped layout should follow tab positions, not creation order"
    );
}
