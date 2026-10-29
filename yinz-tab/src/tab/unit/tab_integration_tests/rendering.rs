use super::*;

#[test]
fn titles_frame_style_keeps_the_only_pane_title() {
    let size = Size { rows: 12, cols: 72 };
    let client = ClientId::new(1);
    let terminal = crate::TerminalId::new(1);
    let pane_id = PaneId::Terminal(terminal);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.handle_pty_bytes(
        terminal,
        b"\x1b]2;only pane title\x07CONTENT FIRST\r\nCONTENT SECOND".to_vec(),
    )
    .unwrap();

    let assert_title_and_content = |tab: &mut Tab| {
        tab.set_force_render();
        let mut output = Output::default();
        tab.render(&mut output, None).unwrap();
        let rendered = take_snapshot(
            output.serialize().unwrap().get(&client).unwrap(),
            size.rows,
            size.cols,
            Palette::default(),
        );
        let lines: Vec<_> = rendered.lines().collect();
        assert!(lines[0].contains("only pane title"), "{rendered}");
        assert!(lines[1].contains("CONTENT FIRST"), "{rendered}");
        assert!(lines[2].contains("CONTENT SECOND"), "{rendered}");
        let pane = tab.get_pane_with_id(pane_id).unwrap();
        assert_eq!(pane.get_content_y(), 1);
        assert_eq!(pane.get_content_rows(), size.rows - 1);
        assert_eq!(pane.get_content_columns(), size.cols);
        assert!(tab.agent_title_is_visible(pane_id, Some(client)));
    };
    assert_title_and_content(&mut tab);

    let second_pane = PaneId::Terminal(crate::TerminalId::new(2));
    tab.new_pane(
        second_pane,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    tab.close_pane(second_pane, false, None);
    assert_title_and_content(&mut tab);

    tab.set_pane_frames(PaneFrameStyle::None);
    assert_eq!(tab.get_pane_with_id(pane_id).unwrap().get_content_y(), 0);
    assert!(!tab.agent_title_is_visible(pane_id, Some(client)));
    tab.set_pane_frames(PaneFrameStyle::Titles);
    assert_title_and_content(&mut tab);

    tab.toggle_active_pane_no_ui_fullscreen(client);
    assert_eq!(tab.get_pane_with_id(pane_id).unwrap().get_content_y(), 0);
    assert!(!tab.agent_title_is_visible(pane_id, Some(client)));
    tab.toggle_active_pane_no_ui_fullscreen(client);
    assert_title_and_content(&mut tab);
}

#[test]
fn increase_tiled_pane_sizes_with_stacked_resizes() {
    // this is the default resizing algorithm
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..5 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }

    // first we increase until fullscreen and once more to make sure we don't increase beyond it
    for _ in 0..=6 {
        tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
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

    // then we decrease until the original position
    for _ in 0..=5 {
        tab.resize(client_id, ResizeStrategy::new(Resize::Decrease, None))
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
}

#[test]
fn increase_tiled_pane_sizes_with_stacked_resizes_into_uneven_panes() {
    // this is the default resizing algorithm
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    tab.move_focus_up(client_id).unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(client_id).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.move_focus_down(client_id).unwrap();

    // increase twice, once to add the short pane into the stack and shorten the larger one, and
    // once to add the remaining two panes (the one we shortened and the extra one near it)
    for _ in 0..2 {
        tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
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

    // then we decrease until the original position
    for _ in 0..2 {
        tab.resize(client_id, ResizeStrategy::new(Resize::Decrease, None))
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
}

#[test]
fn split_stack_vertically() {
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    // the below resizes will end up stacking the panes
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        None,
        client_id,
        None,
        None,
    )
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
fn split_stack_horizontally() {
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    // the below resizes will end up stacking the panes
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        None,
        client_id,
        None,
        None,
    )
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
fn render_stacks_without_pane_frames() {
    // this checks various cases and gotchas that have to do with rendering stacked panes when we
    // don't draw frames around panes
    let size = Size {
        cols: 100,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_without_pane_frames(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    // the below resizes will end up stacking the panes
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(4)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    for i in 5..7 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        false,
        false,
        client_id,
    );
    for i in 7..9 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        false,
        false,
        client_id,
    );
    for i in 9..11 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(i));
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
    }
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
    )
    .unwrap();
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(7)),
        false,
        false,
        client_id,
    );
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(5)),
        false,
        false,
        client_id,
    );

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
fn dump_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let map = Arc::new(Mutex::new(HashMap::new()));
    tab.os_api = Box::new(FakeInputOutput {
        file_dumps: map.clone(),
        ..Default::default()
    });
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
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
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("scratch".as_bytes()))
        .unwrap();
    let file = "/tmp/log.sh";
    tab.dump_active_terminal_screen(Some(file.to_string()), client_id, false)
        .unwrap();
    assert_eq!(
        map.lock().unwrap().get(file).unwrap(),
        "scratch",
        "screen was dumped properly"
    );
}

#[test]
fn clear_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let map = Arc::new(Mutex::new(HashMap::new()));
    tab.os_api = Box::new(FakeInputOutput {
        file_dumps: map.clone(),
        ..Default::default()
    });
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
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
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("scratch".as_bytes()))
        .unwrap();
    let file = "/tmp/log-clear-screen.sh";
    tab.clear_active_terminal_screen(client_id).unwrap();
    tab.dump_active_terminal_screen(Some(file.to_string()), client_id, false)
        .unwrap();
    assert_eq!(
        map.lock().unwrap().get(file).unwrap(),
        "",
        "screen was cleared properly"
    );
}

#[test]
fn pane_subtitle_reserves_content_and_clearing_restores_it() {
    let size = Size { rows: 12, cols: 72 };
    let client = ClientId::new(1);
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        for floating in [false, true] {
            let mut tab = create_new_tab(size, ModeInfo::default());
            tab.set_pane_frames(style);
            let terminal = crate::TerminalId::new(if floating { 2 } else { 1 });
            let id = PaneId::Terminal(terminal);
            if floating {
                tab.toggle_floating_panes(Some(client), None, None).unwrap();
                tab.new_pane(
                    id,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::default(),
                    Some(client),
                    None,
                )
                .unwrap();
            }
            let before = tab.get_pane_with_id(id).unwrap().get_content_rows();
            tab.set_pane_subtitle(id, Some("review · tests/unit · running".into()))
                .unwrap();
            let pane = tab.get_pane_with_id(id).unwrap();
            let shown = style != PaneFrameStyle::None;
            assert_eq!(pane.subtitle_rows(), usize::from(shown));
            let content_y = pane.get_content_y();
            tab.handle_pty_bytes(terminal, b"CONTENT FIRST\r\nCONTENT SECOND".to_vec())
                .unwrap();
            let mut output = Output::default();
            tab.render(&mut output, None).unwrap();
            let rendered = take_snapshot(
                output.serialize().unwrap().get(&client).unwrap(),
                size.rows,
                size.cols,
                Palette::default(),
            );
            assert!(
                rendered
                    .lines()
                    .nth(content_y)
                    .unwrap()
                    .contains("CONTENT FIRST"),
                "{rendered}"
            );
            assert_eq!(rendered.contains("tests/unit"), shown, "{rendered}");
            if shown {
                assert!(
                    rendered
                        .lines()
                        .nth(content_y - 1)
                        .unwrap()
                        .contains("review"),
                    "{rendered}"
                );
            }
            if style == PaneFrameStyle::None && !floating {
                assert_eq!(content_y, 0);
                assert!(
                    !rendered.contains('│') && !rendered.contains('─'),
                    "{rendered}"
                );
            }
            assert_snapshot!(format!("subtitle_{style:?}_{floating}"), rendered);
            if floating {
                tab.toggle_active_pane_fullscreen(client);
                let pane = tab.get_pane_with_id(id).unwrap();
                assert_eq!(pane.subtitle_rows(), usize::from(shown));
                assert_eq!(pane.get_content_y(), if shown { 2 } else { 0 });
                tab.toggle_active_pane_no_ui_fullscreen(client);
                assert_eq!(tab.get_pane_with_id(id).unwrap().subtitle_rows(), 0);
                tab.toggle_active_pane_no_ui_fullscreen(client);
                // Returning from no-UI fullscreen restores the ordinary floating geometry.
            }
            tab.set_pane_frames(PaneFrameStyle::None);
            assert_eq!(tab.get_pane_with_id(id).unwrap().subtitle_rows(), 0);
            tab.set_pane_frames(style);
            for clear in [Some(String::new()), None] {
                tab.set_pane_subtitle(id, clear).unwrap();
                let pane = tab.get_pane_with_id(id).unwrap();
                assert_eq!(pane.get_content_rows(), before);
                assert_eq!(pane.subtitle(), None);
            }
            tab.set_force_render();
            let mut output = Output::default();
            tab.render(&mut output, None).unwrap();
            let rendered = take_snapshot(
                output.serialize().unwrap().get(&client).unwrap(),
                size.rows,
                size.cols,
                Palette::default(),
            );
            assert!(!rendered.contains("review"), "{rendered}");
            assert!(rendered.contains("CONTENT FIRST"), "{rendered}");
        }
    }
}

#[test]
fn pane_subtitles_only_occupy_the_expanded_stack_content() {
    let size = Size {
        rows: 20,
        cols: 100,
    };
    let client = ClientId::new(1);
    for list in [false, true] {
        for style in [
            PaneFrameStyle::Full,
            PaneFrameStyle::Titles,
            PaneFrameStyle::None,
        ] {
            let mut tab = create_new_tab_with_stacked_pane_list(
                size,
                ModeInfo::default(),
                list,
                Some(two_adjacent_stacks_layout()),
            );
            tab.set_pane_frames(style);
            for id in tab.get_all_pane_ids() {
                tab.set_pane_subtitle(id, Some("workflow · running".into()))
                    .unwrap();
            }
            let id = tab.get_active_pane_id(client).unwrap();
            let PaneId::Terminal(terminal) = id else {
                panic!("expected a terminal")
            };
            tab.handle_pty_bytes(terminal, b"STACK CONTENT".to_vec())
                .unwrap();
            let mut output = Output::default();
            tab.render(&mut output, None).unwrap();
            let content_y = tab.get_pane_with_id(id).unwrap().get_content_y();
            let rendered = take_snapshot(
                output.serialize().unwrap().get(&client).unwrap(),
                size.rows,
                size.cols,
                Palette::default(),
            );
            assert!(
                rendered
                    .lines()
                    .nth(content_y)
                    .unwrap()
                    .contains("STACK CONTENT"),
                "{rendered}"
            );
            assert_eq!(
                rendered.matches("workflow").count(),
                if style == PaneFrameStyle::None { 0 } else { 2 },
                "list={list}, style={style:?}\n{rendered}"
            );
        }
    }
}
