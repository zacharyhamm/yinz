use super::*;

#[test]
pub fn screen_can_suppress_pane() {
    let size = Size { cols: 80, rows: 20 };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SuppressPane(PaneId::Terminal(1), 1));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_cannot_break_last_selectable_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let initial_layout = TiledPaneLayout::default();
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_floating_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("tiled_pane".to_owned()),
        ..Default::default()
    };
    let floating_pane = FloatingPaneLayout {
        name: Some("floating_pane_to_eject".to_owned()),
        ..Default::default()
    };
    let mut floating_panes_layout = vec![floating_pane];
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(200));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_multiple_stacked_panes_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let stacked_parent = TiledPaneLayout {
        children_are_stacked: true,
        children: vec![
            TiledPaneLayout {
                name: Some("pane_to_stay".to_owned()),
                ..Default::default()
            },
            TiledPaneLayout {
                name: Some("pane_to_break_1".to_owned()),
                ..Default::default()
            },
            TiledPaneLayout {
                name: Some("pane_to_break_2".to_owned()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children: vec![stacked_parent],
        ..Default::default()
    };

    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render

    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![PaneId::Terminal(1), PaneId::Terminal(2)],
            default_shell: None,
            should_change_focus_to_new_tab: true,
            new_tab_name: None,
            client_id: 1,
            completion_tx: None,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![],
        Default::default(),
        vec![],
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken panes are in the next tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in &snapshots {
        eprintln!("{}", snapshot);
    }
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("plugin_pane_to_break_free".to_owned()),
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_floating_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("tiled_pane".to_owned()),
        ..Default::default()
    };
    let floating_pane = FloatingPaneLayout {
        name: Some("floating_plugin_pane_to_eject".to_owned()),
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let mut floating_panes_layout = vec![floating_pane];
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_right() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));

    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneRight(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_left() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneLeft(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_stack_panes_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let stack_panes_action = CliAction::StackPanes {
        pane_ids: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
    };
    send_cli_action_to_server(&session_metadata, stack_panes_action, client_id);
    let render_count = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instruction| matches!(instruction, ServerInstruction::Render(_)))
        .count();
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    let started_at = std::time::Instant::now();
    while received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instruction| matches!(instruction, ServerInstruction::Render(_)))
        .count()
        == render_count
    {
        assert!(
            started_at.elapsed() < std::time::Duration::from_secs(5),
            "timed out waiting for stacked panes to render"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let (_, snapshot) = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    )
    .into_iter()
    .last()
    .expect("stack panes action should render");
    assert_snapshot!(format!("{}", snapshot));
}

#[test]
pub fn send_cli_change_floating_pane_coordinates_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let initial_tiled_layout = TiledPaneLayout::default();
    let initial_floating_panes = vec![FloatingPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_tiled_layout), initial_floating_panes);

    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let change_floating_pane_coordinates_action = CliAction::ChangeFloatingPaneCoordinates {
        pane_id: "0".to_owned(),
        x: Some("0".to_owned()),
        y: Some("0".to_owned()),
        width: Some("10".to_owned()),
        height: Some("10".to_owned()),
        pinned: None,
        borderless: Some(false),
    };
    send_cli_action_to_server(
        &session_metadata,
        change_floating_pane_coordinates_action,
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn go_to_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1
    new_tab(&mut screen, 3, 2); // ID 2

    // Active tab should be the last one created (ID 2)
    assert_eq!(screen.get_active_tab(client_id).unwrap().id, 2);

    // Switch to tab with ID 0
    if let Some(tab_position) = screen.get_tab_position_by_id(0) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 0
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        0,
        "Active tab should be tab with ID 0"
    );

    // Switch to tab with ID 1
    if let Some(tab_position) = screen.get_tab_position_by_id(1) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 1
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        1,
        "Active tab should be tab with ID 1"
    );
}

#[test]
pub fn send_cli_go_to_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    // Create tabs
    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::GoToTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action caused screen updates (Render instructions sent)
    let render_count = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instr| matches!(instr, ServerInstruction::Render(_)))
        .count();

    assert!(
        render_count > 0,
        "GoToTabById CLI action should trigger screen renders"
    );
}

#[test]
pub fn rename_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1

    // Verify initial tab names
    assert_eq!(screen.get_tab_by_id(0).unwrap().name, "Tab #1");
    assert_eq!(screen.get_tab_by_id(1).unwrap().name, "Tab #2");

    // Rename tab with ID 1
    if let Some(tab) = screen.get_tab_by_id_mut(1) {
        tab.name = "CustomTabName".to_string();
    }

    // Verify the tab name changed
    assert_eq!(
        screen.get_tab_by_id(1).unwrap().name,
        "CustomTabName",
        "Tab with ID 1 should be renamed"
    );

    // Verify other tab name unchanged
    assert_eq!(
        screen.get_tab_by_id(0).unwrap().name,
        "Tab #1",
        "Tab with ID 0 should keep original name"
    );
}

#[test]
pub fn send_cli_rename_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::RenameTabById {
        id: 1,
        name: "TestName".to_string(),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    // no panic above means: RenameTabById CLI action completed without errors
}

#[test]
pub fn close_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1
    new_tab(&mut screen, 3, 2); // ID 2

    assert_eq!(screen.tabs.len(), 3, "Should have 3 tabs initially");

    // Verify all tabs exist
    assert!(
        screen.get_tab_by_id(0).is_some(),
        "Tab with ID 0 should exist"
    );
    assert!(
        screen.get_tab_by_id(1).is_some(),
        "Tab with ID 1 should exist"
    );
    assert!(
        screen.get_tab_by_id(2).is_some(),
        "Tab with ID 2 should exist"
    );

    // Close tab with ID 1
    screen.close_tab_by_id(1).expect("TEST");

    assert_eq!(screen.tabs.len(), 2, "Should have 2 tabs after closing one");

    // Verify tab with ID 1 no longer exists
    assert!(
        screen.get_tab_by_id(1).is_none(),
        "Tab with ID 1 should not exist"
    );

    // Verify other tabs still exist
    assert!(
        screen.get_tab_by_id(0).is_some(),
        "Tab with ID 0 should still exist"
    );
    assert!(
        screen.get_tab_by_id(2).is_some(),
        "Tab with ID 2 should still exist"
    );
}

#[test]
pub fn send_cli_close_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::CloseTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    // no panic above means: CloseTabById CLI action completed without errors
}

#[test]
pub fn send_cli_new_pane_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from CLI through to the
    // PtyInstruction::SpawnInPlaceTerminal instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::NewPane {
        remote_host: None,
        direction: None,
        command: vec!["bash".into()],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::SpawnInPlaceTerminal(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_edit_in_place_with_close_replaced_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::Edit {
        file: PathBuf::from("/some/file.txt"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::SpawnInPlaceTerminal(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from the `launch-or-focus-plugin --in-place`
    // CLI action through to the PtyInstruction::FillPluginCwd instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        move_to_focused_tab: false,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        skip_plugin_cache: false,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let fill_plugin_cwd_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", fill_plugin_cwd_instruction));
}
