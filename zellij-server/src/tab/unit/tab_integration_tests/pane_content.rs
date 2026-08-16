use super::*;

#[test]
fn cannot_float_only_embedded_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    tab.handle_pty_bytes(
        1,
        Vec::from("\n\n\n                   I am an embedded pane".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn replacing_existing_wide_characters() {
    // this is a real world use case using ncmpcpp with wide characters and scrolling
    // the reason we don't break it down is that it exposes quite a few edge cases with wide
    // characters that we should handle properly
    let size = Size {
        cols: 238,
        rows: 48,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    let pane_content = read_fixture("ncmpcpp-wide-chars");
    tab.handle_pty_bytes(1, pane_content).unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn rename_embedded_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    tab.handle_pty_bytes(
        1,
        Vec::from("\n\n\n                   I am an embedded pane".as_bytes()),
    )
    .unwrap();
    tab.update_active_pane_name("Renamed empedded pane".as_bytes().to_vec(), client_id)
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn rename_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am a floating pane".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
    tab.update_active_pane_name("Renamed floating pane".as_bytes().to_vec(), client_id)
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn wide_characters_in_left_title_side() {
    // this test makes sure the title doesn't overflow when it has wide characters
    let size = Size {
        cols: 238,
        rows: 48,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    let pane_content = read_fixture("title-wide-chars");
    tab.handle_pty_bytes(1, pane_content).unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn save_cursor_position_across_resizes() {
    // the save cursor position ANSI instruction (CSI s) needs to point to the same character after we
    // resize the pane
    let size = Size { cols: 100, rows: 5 };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    tab.handle_pty_bytes(
        1,
        Vec::from("\n\n\rI am some text\n\rI am another line of text\n\rLet's save the cursor position here \u{1b}[sI should be ovewritten".as_bytes()),
    ).unwrap();

    // We check cursor and saved cursor are handled separately by:
    // 1. moving real cursor up two lines
    let _ = tab.handle_pty_bytes(1, Vec::from("\u{1b}[2A".as_bytes()));
    // 2. resizing so real cursor gets lost above the viewport, which resets it to row 0
    // The saved cursor ends up on row 1, allowing detection if it (incorrectly) gets reset too
    tab.resize_whole_tab(Size { cols: 35, rows: 4 }).unwrap();

    // Now overwrite
    tab.handle_pty_bytes(1, Vec::from("\u{1b}[uthis overwrote me!".as_bytes()))
        .unwrap();

    tab.resize_whole_tab(Size { cols: 100, rows: 3 }).unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_with_sixel_image() {
    let new_pane_id = PaneId::Terminal(2);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let mut tab = create_new_tab_with_sixel_support(size, sixel_image_store.clone());
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let sixel_host_capabilities = Rc::new(RefCell::new(HashMap::new()));
    sixel_host_capabilities.borrow_mut().insert(client_id, true);
    let mut output = Output::new(
        sixel_image_store.clone(),
        character_cell_size,
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        sixel_host_capabilities,
    );

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let fixture = read_fixture("sixel-image-500px.six");
    tab.handle_pty_bytes(2, fixture).unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot_with_sixel(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
        sixel_image_store,
    );

    assert_snapshot!(snapshot);
}

#[test]
fn floating_pane_above_sixel_image() {
    let new_pane_id = PaneId::Terminal(2);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let mut tab = create_new_tab_with_sixel_support(size, sixel_image_store.clone());
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let sixel_host_capabilities = Rc::new(RefCell::new(HashMap::new()));
    sixel_host_capabilities.borrow_mut().insert(client_id, true);
    let mut output = Output::new(
        sixel_image_store.clone(),
        character_cell_size,
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        sixel_host_capabilities,
    );

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let fixture = read_fixture("sixel-image-500px.six");
    tab.handle_pty_bytes(1, fixture).unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot_with_sixel(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
        sixel_image_store,
    );

    assert_snapshot!(snapshot);
}

#[test]
fn suppress_tiled_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.replace_active_pane_with_editor_pane(new_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn suppress_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let editor_pane_id = PaneId::Terminal(3);
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn close_suppressing_tiled_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.replace_active_pane_with_editor_pane(new_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(1, Vec::from("\n\n\nI am the original pane".as_bytes()))
        .unwrap();
    tab.close_pane(new_pane_id, false, None);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn close_suppressing_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let editor_pane_id = PaneId::Terminal(3);
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am the original pane".as_bytes()))
        .unwrap();
    tab.close_pane(editor_pane_id, false, None);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn suppress_tiled_pane_float_it_and_close() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.replace_active_pane_with_editor_pane(new_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(1, Vec::from("\n\n\nI am the original pane".as_bytes()))
        .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
    tab.close_pane(new_pane_id, false, None);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn suppress_floating_pane_embed_it_and_close_it() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let editor_pane_id = PaneId::Terminal(3);
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am the original pane".as_bytes()))
        .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
    tab.close_pane(editor_pane_id, false, None);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn resize_whole_tab_while_tiled_pane_is_suppressed() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.replace_active_pane_with_editor_pane(new_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(2, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.resize_whole_tab(Size {
        cols: 100,
        rows: 10,
    })
    .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn resize_whole_tab_while_floting_pane_is_suppressed() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let editor_pane_id = PaneId::Terminal(3);
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.replace_active_pane_with_editor_pane(editor_pane_id, client_id)
        .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\n\n\nI am an editor pane".as_bytes()))
        .unwrap();
    tab.resize_whole_tab(Size {
        cols: 100,
        rows: 10,
    })
    .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn enter_search_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mode_info = ModeInfo {
        mode: InputMode::Search,
        ..Default::default()
    };
    let mut tab = create_new_tab(size, mode_info);
    let mut output = Output::default();
    let pane_content = read_fixture("grid_copy");
    tab.handle_pty_bytes(1, pane_content).unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_tab_nothing_highlighted", snapshot);

    // Pane title should show 'tortor' as search term
    // Only lines containing 'tortor' get marked as render-targets, so
    // only those are updated (search-styling is not visible here).
    tab.update_search_term("tortor".as_bytes().to_vec(), client_id)
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_tab_highlight_tortor", snapshot);

    // Pane title should show search modifiers
    tab.toggle_search_wrap(client_id);
    tab.toggle_search_whole_words(client_id);
    tab.toggle_search_case_sensitivity(client_id);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_tab_highlight_tortor_modified", snapshot);

    // And only the search term again
    tab.toggle_search_wrap(client_id);
    tab.toggle_search_whole_words(client_id);
    tab.toggle_search_case_sensitivity(client_id);

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_tab_highlight_tortor", snapshot);
}

#[test]
fn enter_search_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mode_info = ModeInfo {
        mode: InputMode::Search,
        ..Default::default()
    };
    let mut tab = create_new_tab(size, mode_info);
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    let pane_content = read_fixture("grid_copy");
    tab.handle_pty_bytes(2, pane_content).unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_floating_tab_nothing_highlighted", snapshot);

    // Only the line inside the floating tab which contain 'fring' should be in the new snapshot
    tab.update_search_term("fring".as_bytes().to_vec(), client_id)
        .unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!("search_floating_tab_highlight_fring", snapshot);
}

#[test]
fn pane_in_sgr_button_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1002;1006h"); // button event tracking (1002) with SGR encoding (1006)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[<0;71;5M".to_string(),  // SGR left click
            "\u{1b}[<32;72;9M".to_string(), // SGR left click (hold)
            "\u{1b}[<0;75;7m".to_string(),  // SGR left button release
            "\u{1b}[<2;71;5M".to_string(),  // SGR right click
            "\u{1b}[<34;72;9M".to_string(), // SGR right click (hold)
            "\u{1b}[<2;75;7m".to_string(),  // SGR right button release
            "\u{1b}[<1;71;5M".to_string(),  // SGR middle click
            "\u{1b}[<33;72;9M".to_string(), // SGR middle click (hold)
            "\u{1b}[<1;75;7m".to_string(),  // SGR middle button release
            "\u{1b}[<64;71;5M".to_string(), // SGR scroll up
            "\u{1b}[<65;71;5M".to_string(), // SGR scroll down
        ]
    );
}

#[test]
fn pane_in_sgr_normal_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1000;1006h"); // normal event tracking (1000) with sgr encoding (1006)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[<0;71;5M".to_string(), // SGR left click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[<0;75;7m".to_string(), // SGR left button release
            "\u{1b}[<2;71;5M".to_string(), // SGR right click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[<2;75;7m".to_string(), // SGR right button release
            "\u{1b}[<1;71;5M".to_string(), // SGR middle click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[<1;75;7m".to_string(),  // SGR middle button release
            "\u{1b}[<64;71;5M".to_string(), // SGR scroll up
            "\u{1b}[<65;71;5M".to_string(), // SGR scroll down
        ]
    );
}

#[test]
fn pane_in_sgr_any_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1003;1006h"); // any event tracking (1003) with SGR encoding (1006)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_buttonless_motion(Position::new(9, 72)),
        client_id,
    )
    .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[<0;71;5M".to_string(),  // SGR left click
            "\u{1b}[<32;72;9M".to_string(), // SGR left click (hold)
            "\u{1b}[<0;75;7m".to_string(),  // SGR left button release
            "\u{1b}[<2;71;5M".to_string(),  // SGR right click
            "\u{1b}[<34;72;9M".to_string(), // SGR right click (hold)
            "\u{1b}[<2;75;7m".to_string(),  // SGR right button release
            "\u{1b}[<1;71;5M".to_string(),  // SGR middle click
            "\u{1b}[<33;72;9M".to_string(), // SGR middle click (hold)
            "\u{1b}[<1;75;7m".to_string(),  // SGR middle button release
            "\u{1b}[<64;71;5M".to_string(), // SGR scroll up
            "\u{1b}[<65;71;5M".to_string(), // SGR scroll down
            "\u{1b}[<35;72;9M".to_string(), // SGR buttonless motion
        ]
    );
}

#[test]
fn pane_in_utf8_button_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1002;1005h"); // button event tracking (1002) with utf8 encoding (1005)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[M g%".to_string(),  // utf8 left click
            "\u{1b}[M@h)".to_string(),  // utf8 left click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 left button release
            "\u{1b}[M\"g%".to_string(), // utf8 right click
            "\u{1b}[MBh)".to_string(),  // utf8 right click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 right button release
            "\u{1b}[M!g%".to_string(),  // utf8 middle click
            "\u{1b}[MAh)".to_string(),  // utf8 middle click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 middle click release
            "\u{1b}[M`g%".to_string(),  // utf8 scroll up
            "\u{1b}[Mag%".to_string(),  // utf8 scroll down
        ]
    );
}

#[test]
fn pane_in_utf8_normal_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1000;1005h"); // normal event tracking (1000) with sgr encoding (1006)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[M g%".to_string(), // utf8 left click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[M#k'".to_string(),  // utf8 left button release
            "\u{1b}[M\"g%".to_string(), // utf8 right click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[M#k'".to_string(), // utf8 right button release
            "\u{1b}[M!g%".to_string(), // utf8 middle click
            // no hold event here, as hold events are not reported in normal mode
            "\u{1b}[M#k'".to_string(), // utf8 middle click release
            "\u{1b}[M`g%".to_string(), // utf8 scroll up
            "\u{1b}[Mag%".to_string(), // utf8 scroll down
        ]
    );
}

#[test]
fn pane_in_utf8_any_event_tracking_mouse_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let sgr_mouse_mode_any_button = String::from("\u{1b}[?1003;1005h"); // any event tracking (1002) with utf8 encoding (1005)
    tab.handle_pty_bytes(1, sgr_mouse_mode_any_button.as_bytes().to_vec())
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_right_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_right_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_right_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_middle_motion_event(Position::new(9, 72)),
        client_id,
    )
    .unwrap();
    // tab.handle_middle_mouse_release(&Position::new(7, 75), client_id)
    tab.handle_mouse_event(
        &MouseEvent::new_middle_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_scrollwheel_up(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(5, 71), 1, client_id)
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_buttonless_motion(Position::new(9, 72)),
        client_id,
    )
    .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec![
            "\u{1b}[M g%".to_string(),  // utf8 left click
            "\u{1b}[M@h)".to_string(),  // utf8 left click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 left button release
            "\u{1b}[M\"g%".to_string(), // utf8 right click
            "\u{1b}[MBh)".to_string(),  // utf8 right click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 right button release
            "\u{1b}[M!g%".to_string(),  // utf8 middle click
            "\u{1b}[MAh)".to_string(),  // utf8 middle click (hold)
            "\u{1b}[M#k'".to_string(),  // utf8 middle click release
            "\u{1b}[M`g%".to_string(),  // utf8 scroll up
            "\u{1b}[Mag%".to_string(),  // utf8 scroll down
            "\u{1b}[MCh)".to_string(),  // urf8 buttonless motion
        ]
    );
}
