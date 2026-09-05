use super::*;

#[test]
fn increase_tiled_pane_sizes_with_stacked_resizes() {
    // this is the default resizing algorithm
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..5 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.move_focus_right(client_id).unwrap();
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId(5)),
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(4)),
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(4)),
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab_without_pane_frames(size, ModeInfo::default());
    let mut output = Output::default();
    for i in 2..4 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(4)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    for i in 5..7 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(1)),
        false,
        false,
        client_id,
    );
    for i in 7..9 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(1)),
        false,
        false,
        client_id,
    );
    for i in 9..11 {
        let new_pane_id_1 = PaneId::Terminal(crate::TerminalId(i));
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
        PaneId::Terminal(crate::TerminalId(7)),
        false,
        false,
        client_id,
    );
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId(5)),
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let map = Arc::new(Mutex::new(HashMap::new()));
    tab.os_api = Box::new(FakeInputOutput {
        file_dumps: map.clone(),
        ..Default::default()
    });
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
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
    tab.handle_pty_bytes(crate::TerminalId(2), Vec::from("scratch".as_bytes()))
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
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let map = Arc::new(Mutex::new(HashMap::new()));
    tab.os_api = Box::new(FakeInputOutput {
        file_dumps: map.clone(),
        ..Default::default()
    });
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
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
    tab.handle_pty_bytes(crate::TerminalId(2), Vec::from("scratch".as_bytes()))
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
