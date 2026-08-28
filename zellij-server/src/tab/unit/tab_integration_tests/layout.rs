use super::*;

#[test]
fn tab_with_basic_layout() {
    let layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane
                pane split_direction="Horizontal" {
                    pane
                    pane
                }
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
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
fn tab_with_layout_that_has_floating_panes() {
    let layout = r#"
        layout {
            pane
            floating_panes {
                pane
                pane
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
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
fn titles_frame_style_with_fixed_size_pane() {
    let layout = r#"
        layout {
            pane split_direction="horizontal" {
                pane name="foo" {
                    size 7
                }
                pane
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.handle_pty_bytes(0, Vec::from("I am the fixed size pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(1, Vec::from("I am the flexible pane".as_bytes()))
        .unwrap();
    let mut output = Output::default();
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
fn tab_with_nested_layout() {
    let layout = r#"
        layout {
            pane_template name="top-and-vertical-sandwich" {
                pane
                vertical-sandwich {
                    pane
                }
            }
            pane_template name="vertical-sandwich" split_direction="vertical" {
                pane
                children
                pane
            }
            pane_template name="nested-vertical-sandwich" split_direction="vertical" {
                pane
                top-and-vertical-sandwich
                pane
            }
            nested-vertical-sandwich
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
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
fn tab_with_nested_uneven_layout() {
    let layout = r#"
        layout {
            pane_template name="horizontal-with-vertical-top" {
                pane split_direction="Vertical" {
                    pane
                    children
                }
                pane
            }
            horizontal-with-vertical-top name="my tab" {
                pane
                pane
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
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
fn pane_bracketed_paste_ignored_when_not_in_bracketed_paste_mode() {
    // regression test for: https://github.com/zellij-org/zellij/issues/1687
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id: u16 = 1;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let bracketed_paste_start = vec![27, 91, 50, 48, 48, 126]; // \u{1b}[200~
    let bracketed_paste_end = vec![27, 91, 50, 48, 49, 126]; // \u{1b}[201
    tab.write_to_active_terminal(&None, bracketed_paste_start, false, client_id)
        .unwrap();
    tab.write_to_active_terminal(&None, "test".as_bytes().to_vec(), false, client_id)
        .unwrap();
    tab.write_to_active_terminal(&None, bracketed_paste_end, false, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(pty_instruction_bus.clone_output(), vec!["", "test", ""]);
}

#[test]
fn pane_faux_scrolling_in_alternate_mode() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id: u16 = 1;
    let lines_to_scroll = 3;

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let enable_alternate_screen = String::from("\u{1b}[?1049h"); // CSI ? 1049 h -> switch to the Alternate Screen Buffer
    let set_application_mode = String::from("\u{1b}[?1h");

    // no output since alternate scren not active yet
    tab.handle_scrollwheel_up(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();

    tab.handle_pty_bytes(1, enable_alternate_screen.as_bytes().to_vec())
        .unwrap();
    // CSI A * lines_to_scroll, CSI B * lines_to_scroll
    tab.handle_scrollwheel_up(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();

    tab.handle_pty_bytes(1, set_application_mode.as_bytes().to_vec())
        .unwrap();
    // SS3 A * lines_to_scroll, SS3 B * lines_to_scroll
    tab.handle_scrollwheel_up(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();

    pty_instruction_bus.exit();

    let mut expected: Vec<&str> = Vec::new();
    expected.append(&mut vec!["\u{1b}[A"; lines_to_scroll]);
    expected.append(&mut vec!["\u{1b}[B"; lines_to_scroll]);
    expected.append(&mut vec!["\u{1b}OA"; lines_to_scroll]);
    expected.append(&mut vec!["\u{1b}OB"; lines_to_scroll]);

    assert_eq!(pty_instruction_bus.clone_output(), expected);
}

#[test]
fn move_pane_focus_sends_tty_csi_event() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(2);
    tab.new_pane(
        new_pane_id_1,
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
        1,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.move_focus_left(client_id).unwrap();
    assert_snapshot!(format!("{:?}", *tty_stdin_bytes.lock().unwrap()));
}

#[test]
fn move_floating_pane_focus_sends_tty_csi_event() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
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
        1,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        3,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.move_focus_left(client_id).unwrap();
    assert_snapshot!(format!("{:?}", *tty_stdin_bytes.lock().unwrap()));
}

#[test]
fn toggle_floating_panes_on_sends_tty_csi_event() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.handle_pty_bytes(
        1,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        3,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    assert_snapshot!(format!("{:?}", *tty_stdin_bytes.lock().unwrap()));
}

#[test]
fn toggle_floating_panes_off_sends_tty_csi_event() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
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
        1,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        3,
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    assert_snapshot!(format!("{:?}", *tty_stdin_bytes.lock().unwrap()));
}
