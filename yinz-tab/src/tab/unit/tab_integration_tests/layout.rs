use super::*;

#[test]
fn layout_reapplication_preserves_user_pane_names() {
    let mut tab = create_new_tab(
        Size {
            cols: 120,
            rows: 40,
        },
        ModeInfo::default(),
    );
    tab.tiled_panes.drain();
    let tiled_layout = TiledPaneLayout {
        name: Some("layout tiled".into()),
        ..Default::default()
    };
    let floating_layout = vec![FloatingPaneLayout {
        name: Some("layout floating".into()),
        ..Default::default()
    }];
    tab.apply_layout(
        tiled_layout.clone(),
        floating_layout.clone(),
        vec![(TerminalId::new(1), None)],
        vec![(TerminalId::new(2), None)],
        HashMap::new(),
        ClientId::new(1),
        None,
    )
    .unwrap();
    let names = [(1, "tiled"), (2, "floating")];
    for (id, name) in names {
        let pane_id = PaneId::Terminal(TerminalId::new(id));
        assert_eq!(
            tab.get_pane_with_id(pane_id).unwrap().current_title(),
            format!("layout {name}")
        );
        tab.rename_pane(format!("user {name}").into_bytes(), pane_id)
            .unwrap();
    }

    tab.layout_applier(None)
        .apply_tiled_panes_layout_to_existing_panes(&tiled_layout)
        .unwrap();
    tab.layout_applier(None)
        .apply_floating_panes_layout_to_existing_panes(&floating_layout)
        .unwrap();
    for (id, name) in names {
        assert_eq!(
            tab.get_pane_with_id(PaneId::Terminal(TerminalId::new(id)))
                .unwrap()
                .current_title(),
            format!("user {name}")
        );
    }

    tab.resize_whole_tab(Size {
        cols: 140,
        rows: 50,
    })
    .unwrap();
    for (id, name) in names {
        assert_eq!(
            tab.get_pane_with_id(PaneId::Terminal(TerminalId::new(id)))
                .unwrap()
                .current_title(),
            format!("user {name}")
        );
    }
}

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
    let client_id = ClientId::new(1);
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
    let client_id = ClientId::new(1);
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
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.handle_pty_bytes(
        crate::TerminalId::new(0),
        Vec::from("I am the fixed size pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("I am the flexible pane".as_bytes()),
    )
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
    let client_id = ClientId::new(1);
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
    let client_id = ClientId::new(1);
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
    let client_id = ClientId::new(1);

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    let bracketed_paste_start = vec![27, 91, 50, 48, 48, 126]; // \u{1b}[200~
    let bracketed_paste_end = vec![27, 91, 50, 48, 49, 126]; // \u{1b}[201
    tab.write_to_active_terminal(&None, bracketed_paste_start, false, client_id, None)
        .unwrap();
    tab.write_to_active_terminal(&None, "test".as_bytes().to_vec(), false, client_id, None)
        .unwrap();
    tab.write_to_active_terminal(&None, bracketed_paste_end, false, client_id, None)
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
    let client_id = ClientId::new(1);
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

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        enable_alternate_screen.as_bytes().to_vec(),
    )
    .unwrap();
    // CSI A * lines_to_scroll, CSI B * lines_to_scroll
    tab.handle_scrollwheel_up(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();
    tab.handle_scrollwheel_down(&Position::new(1, 1), lines_to_scroll, client_id)
        .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        set_application_mode.as_bytes().to_vec(),
    )
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
    let client_id = ClientId::new(1);
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
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
        crate::TerminalId::new(1),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
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
    let client_id = ClientId::new(1);
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));

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
        crate::TerminalId::new(1),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(3),
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
    let client_id = ClientId::new(1);
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));

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
        crate::TerminalId::new(1),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(3),
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
    let client_id = ClientId::new(1);
    let tty_stdin_bytes = Arc::new(Mutex::new(BTreeMap::new()));
    let os_api = Box::new(FakeInputOutput {
        tty_stdin_bytes: tty_stdin_bytes.clone(),
        ..Default::default()
    });
    let mut tab = create_new_tab_with_os_api(size, ModeInfo::default(), &os_api);
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));

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
        crate::TerminalId::new(1),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(3),
        // subscribe to focus events
        Vec::from("\u{1b}[?1004h".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    assert_snapshot!(format!("{:?}", *tty_stdin_bytes.lock().unwrap()));
}

#[test]
fn a_resize_tells_only_the_panes_whose_size_changed() {
    // pane 1 | pane 2 | pane 3, pane 2 focused and grown to the right: pane 2 and pane 3
    // change size, pane 1 does not - and used to get a pty resize (a SIGWINCH) anyway,
    // on every step of a held resize key.
    let (tx, rx) = channels::unbounded();
    let mut tab = create_new_tab_with_mock_pty_writer(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
        SenderWithContext::new(tx),
    );
    let client_id = crate::ClientId::new(1);
    let pane = |n: u32| PaneId::Terminal(crate::TerminalId::new(n));
    tab.vertical_split(pane(2), None, None, client_id, None, None)
        .unwrap();
    tab.vertical_split(pane(3), None, None, client_id, None, None)
        .unwrap();
    tab.move_focus_left(client_id).unwrap();
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(pane(2)),
        "precondition"
    );
    let content_sizes = |tab: &Tab| -> HashMap<PaneId, (usize, usize)> {
        tab.tiled_panes
            .panes
            .iter()
            .map(|(id, pane)| (*id, (pane.get_content_columns(), pane.get_content_rows())))
            .collect()
    };
    let before = content_sizes(&tab);
    let _ = rx.try_iter().count();

    tab.resize(
        client_id,
        yinz_types::geometry::ResizeStrategy::new(
            yinz_types::geometry::Resize::Increase,
            Some(yinz_types::geometry::Direction::Right),
        ),
    )
    .unwrap();

    let after = content_sizes(&tab);
    let changed: HashSet<PaneId> = after
        .iter()
        .filter(|(id, size)| before.get(*id) != Some(*size))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        changed,
        HashSet::from([pane(2), pane(3)]),
        "precondition: the resize moves one boundary"
    );
    let resized: HashSet<PaneId> = rx
        .try_iter()
        .filter_map(|(msg, _)| match msg {
            PtyWriteInstruction::ResizePty(id, ..) => Some(PaneId::Terminal(id)),
            _ => None,
        })
        .collect();
    assert_eq!(resized, changed);
}

fn render_split_row_of_ui_panes_at_top(pane_frame_style: PaneFrameStyle) -> String {
    let layout = r#"
        layout {
            pane size=1 split_direction="vertical" {
                pane size=55
                pane
            }
            pane
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    tab.set_pane_frames(pane_frame_style);
    let mut panes_by_position: Vec<(usize, usize, crate::TerminalId)> = tab
        .tiled_panes
        .get_panes()
        .filter_map(|(pane_id, pane)| match pane_id {
            PaneId::Terminal(terminal_id) => Some((pane.y(), pane.x(), *terminal_id)),
            PaneId::Plugin(_) => None,
        })
        .collect();
    panes_by_position.sort_unstable();
    let [(_, _, left_bar_id), (_, _, right_bar_id), (_, _, main_pane_id)] = panes_by_position[..]
    else {
        panic!("unexpected panes: {:?}", panes_by_position);
    };
    tab.set_pane_selectable(PaneId::Terminal(left_bar_id), false);
    tab.set_pane_selectable(PaneId::Terminal(right_bar_id), false);
    tab.handle_pty_bytes(left_bar_id, Vec::from("I am the left bar".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(right_bar_id, Vec::from("I am the right bar".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(main_pane_id, Vec::from("I am the main pane".as_bytes()))
        .unwrap();
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    )
}

#[test]
fn titles_frame_style_with_split_row_of_ui_panes_at_top() {
    let snapshot = render_split_row_of_ui_panes_at_top(PaneFrameStyle::Titles);
    assert_snapshot!(snapshot);
}

#[test]
fn no_frame_style_with_split_row_of_ui_panes_at_top() {
    let snapshot = render_split_row_of_ui_panes_at_top(PaneFrameStyle::None);
    assert_snapshot!(snapshot);
}

#[test]
fn bulk_focus_events_are_unique_and_only_for_connected_clients() {
    use yinz_panes::panes::ActivePanes;
    let os_api = Box::new(FakeInputOutput::default());
    let mut tab = create_new_tab_with_os_api(
        Size {
            cols: 120,
            rows: 20,
        },
        ModeInfo::default(),
        &os_api,
    );
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    let absent = ClientId::new(3);
    let left = PaneId::Terminal(TerminalId::new(1));
    let right = PaneId::Terminal(TerminalId::new(2));
    tab.vertical_split(right, None, None, first, None, None)
        .unwrap();
    for id in [TerminalId::new(1), TerminalId::new(2)] {
        tab.handle_pty_bytes(id, b"\x1b[?1004h".to_vec()).unwrap();
    }
    let panes = tab.tiled_panes.drain();
    let tty_stdin = os_api.tty_stdin_bytes.clone();
    let os_api: Box<dyn ServerOsApi> = os_api;
    let mut active = ActivePanes::new(&os_api);
    active.insert(first, left, &panes);
    active.insert(second, left, &panes);
    active.insert(absent, right, &panes);
    tty_stdin.lock().unwrap().clear();
    let connected = HashSet::from([first, second]);
    active.unfocus_all_panes(&panes, &connected);
    active.focus_all_panes(&panes, &connected);
    assert_eq!(
        *tty_stdin.lock().unwrap(),
        BTreeMap::from([(TerminalId::new(1), b"\x1b[O\x1b[I".to_vec())])
    );
    tty_stdin.lock().unwrap().clear();
    active.insert(first, PaneId::Terminal(TerminalId::new(99)), &panes);
    assert_eq!(active.get(&first), Some(&left));
    assert!(tty_stdin.lock().unwrap().is_empty());
}
