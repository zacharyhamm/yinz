use super::*;

#[test]
pub fn mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = 1;
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
}

#[test]
pub fn disabled_mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = 1;
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.set_advanced_hover_effects(false);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
}

#[test]
fn group_panes_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(2)]),
        "Pane Id added to client's pane group"
    );

    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

#[test]
fn mouse_focus_clears_bell_on_focused_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    let new_pane_id = PaneId::Terminal(2);
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        // Split horizontally: pane 1 on top, pane 2 on bottom; focus moves to pane 2
        active_tab
            .horizontal_split(new_pane_id, None, client_id, None, None)
            .unwrap();
        // Move focus back up to pane 1 so pane 2 is unfocused
        active_tab.move_focus_up(client_id).unwrap();
        // Plant a bell on the unfocused pane 2
        active_tab.handle_pty_bytes(2, vec![7u8]).unwrap();
        active_tab.check_and_handle_bell_notifications(false);
        assert!(
            active_tab.panes_with_pending_bell.contains(&new_pane_id),
            "Pane 2 should have a pending bell before the click"
        );
        assert!(
            active_tab.tab_has_pending_bell,
            "Tab should have a pending bell before the click"
        );
    }

    // Click somewhere in pane 2 (bottom half of the screen)
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(15, 60)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(15, 60)),
        client_id,
    );

    let active_tab = screen.get_active_tab(client_id).unwrap();
    assert_eq!(
        active_tab.get_active_pane_id(client_id),
        Some(new_pane_id),
        "Mouse click should have focused pane 2"
    );
    assert!(
        !active_tab.panes_with_pending_bell.contains(&new_pane_id),
        "Bell on pane 2 should be cleared after mouse focus"
    );
    assert!(
        !active_tab.tab_has_pending_bell,
        "Tab bell should be cleared after the last pane bell is cleared"
    );
}

#[test]
fn nested_guest_fullscreen_moves_from_one_pane_to_another() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let pane_one = PaneId::Terminal(1);
    let pane_two = PaneId::Terminal(2);
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        active_tab
            .horizontal_split(pane_two, None, client_id, None, None)
            .unwrap();
    }

    screen.apply_nested_guest_fullscreen(pane_one, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_one),
            "the first guest pane is fullscreen"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested_fullscreen_panes,
        [pane_one].into_iter().collect(),
        "only the first guest pane is tracked as fullscreen"
    );

    screen.apply_nested_guest_fullscreen(pane_two, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_two),
            "fullscreen moved to the second guest pane"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested_fullscreen_panes,
        [pane_two].into_iter().collect(),
        "the first guest pane is no longer tracked as fullscreen, only the second is"
    );

    screen.apply_nested_guest_fullscreen(pane_two, false);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            None,
            "unsetting fullscreen restores the tiled layout"
        );
    }
    assert!(
        screen.nested_fullscreen_panes.is_empty(),
        "no guest panes are tracked as fullscreen after unsetting"
    );
}

#[test]
fn group_panes_with_keyboard() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(2)]),
        "Pane Id added to client's pane group"
    );

    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

#[test]
fn group_panes_following_focus() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(i),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![PaneId::Terminal(4), PaneId::Terminal(3)]),
            "Pane Id of focused pane and newly focused pane above added to pane group"
        );
    }
    {
        let _ = screen.toggle_group_marking(client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(screen.current_pane_group.borrow().clone_inner().get(&client_id), Some(&vec![PaneId::Terminal(4), PaneId::Terminal(3)]), "Pane Id of newly focused pane not added to group after the group marking was toggled off");
    }
}

#[test]
fn break_group_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(i),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![
                PaneId::Terminal(4),
                PaneId::Terminal(3),
                PaneId::Terminal(2)
            ]),
            "Group contains 3 panes"
        );
    }

    screen.handle_mouse_event(
        MouseEvent::new_right_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );
    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Group cleared by mouse event"
    );
}
