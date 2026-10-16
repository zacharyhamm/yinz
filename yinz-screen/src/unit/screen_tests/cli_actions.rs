#[cfg(test)]
const TAB_0: TabId = TabId::new(0);

use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;

// Following are tests for sending CLI actions
// these tests are only partially relevant to Screen
// and are included here for two reasons:
// 1. The best way to "integration test" these is combining the "screen_thread_main" and
//    "route_action" functions and mocking everything around them
// 2. These inadvertently also test many parts of Screen that are not tested elsewhere

#[test]
pub fn send_cli_write_chars_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::WriteChars {
        chars: "input from the cli".into(),
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    let write = received_pty_instructions.wait_until(
        "the CLI characters to reach the pty writer",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    PtyWriteInstruction::Write(bytes, pane_id, _) => {
                        Some((bytes.clone(), *pane_id))
                    },
                    _ => None,
                })
        },
    );
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    assert_eq!(write, (b"input from the cli".to_vec(), TerminalId::new(0)));
}

#[test]
pub fn send_cli_write_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::Write {
        bytes: vec![102, 111, 111],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    let write = received_pty_instructions.wait_until(
        "the CLI bytes to reach the pty writer",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    PtyWriteInstruction::Write(bytes, pane_id, _) => {
                        Some((bytes.clone(), *pane_id))
                    },
                    _ => None,
                })
        },
    );
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    assert_eq!(write, (b"foo".to_vec(), TerminalId::new(0)));
}

#[test]
pub fn write_only_requests_a_clear_scroll_render_when_scrolled() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(1);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);

    let render_count_barrier = |marker_client_id| {
        let (screen_barrier_tx, screen_barrier_rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetPaneInfo {
                pane_id: PaneId::Terminal(crate::TerminalId::new(0)),
                response_channel: screen_barrier_tx,
            })
            .expect("failed to send screen barrier");
        screen_barrier_rx
            .recv_timeout(yinz_test_support::default_timeout())
            .expect("screen barrier timed out");

        let marker = BackgroundJob::ClearHelpText {
            client_id: marker_client_id,
        };
        mock_screen
            .to_background_jobs
            .send(marker.clone())
            .expect("failed to send background-job barrier");
        received_background_jobs.wait_until(
            "the background-job barrier",
            yinz_test_support::default_timeout(),
            |jobs| jobs.iter().position(|job| job == &marker),
        )
    };

    let unscrolled_start = render_count_barrier(crate::ClientId::new(100));
    send_cli_action_to_server(
        &session_metadata,
        CliAction::WriteChars {
            chars: "unscrolled".into(),
            pane_id: None,
        },
        client_id,
    );
    let unscrolled_end = render_count_barrier(crate::ClientId::new(101));
    let unscrolled_render_requests = received_background_jobs.lock()
        [unscrolled_start + 1..unscrolled_end]
        .iter()
        .filter(|job| **job == BackgroundJob::RenderToClients)
        .count();

    let mut pane_contents = String::new();
    for line in 0..20 {
        pane_contents.push_str(&format!("fill pane {line}\n\r"));
    }
    mock_screen
        .to_screen
        .send(ScreenInstruction::PtyBytes(
            crate::TerminalId::new(0),
            pane_contents.as_bytes().to_vec(),
        ))
        .expect("failed to fill pane");
    for _ in 0..2 {
        send_cli_action_to_server(
            &session_metadata,
            CliAction::ScrollUp { pane_id: None },
            client_id,
        );
    }

    let scrolled_start = render_count_barrier(crate::ClientId::new(102));
    send_cli_action_to_server(
        &session_metadata,
        CliAction::WriteChars {
            chars: "scrolled".into(),
            pane_id: None,
        },
        client_id,
    );
    let scrolled_end = render_count_barrier(crate::ClientId::new(103));
    let scrolled_render_requests = received_background_jobs.lock()
        [scrolled_start + 1..scrolled_end]
        .iter()
        .filter(|job| **job == BackgroundJob::RenderToClients)
        .count();

    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);

    // These count synchronous render requests from fixed instruction paths, not asynchronous
    // frames, so scheduler-side render coalescing cannot change the totals.
    assert_eq!(unscrolled_render_requests, 1);
    assert_eq!(scrolled_render_requests, 2);
}

#[test]
pub fn send_cli_send_keys_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl a".to_string(), "x".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    // here we assert only the write instructions to make sure they arrived properly and in
    // sequence to the pane
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_ctrl_c_reaches_pty_writer() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl c".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_ctrl_w_reaches_pty_writer() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl w".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_resize_action_to_screen() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let pty_writer_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let resize_cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: None,
    };
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, resize_cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_next_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let focus_next_pane_action = CliAction::FocusNextPane;
    send_cli_action_to_server(&session_metadata, focus_next_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_previous_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let focus_next_pane_action = CliAction::FocusPreviousPane;
    send_cli_action_to_server(&session_metadata, focus_next_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_last_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocus {
        direction: Direction::Right,
    };
    let focus_last_pane_action = CliAction::FocusLastPane;
    // move focus 1 -> 2 -> 3
    send_cli_action_to_server(&session_metadata, move_focus_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, move_focus_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
                                                               // move focus 3 -> 2
    send_cli_action_to_server(&session_metadata, focus_last_pane_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
                                                               // move focus 2 -> 3
    send_cli_action_to_server(&session_metadata, focus_last_pane_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_focus_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocus {
        direction: Direction::Right,
    };
    send_cli_action_to_server(&session_metadata, move_focus_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_focus_or_tab_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocusOrTab {
        direction: Direction::Right,
    };
    send_cli_action_to_server(&session_metadata, move_focus_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_dump_screen_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/foo")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "fill pane up with something".as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert_snapshot!(format!(
        "{:?}",
        *mock_screen.os_input.fake_filesystem.lock().unwrap()
    ));
}

#[test]
pub fn send_cli_edit_scrollback_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::EditScrollback {
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "fill pane up with something".as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let dumped_file_name = mock_screen
        .os_input
        .fake_filesystem
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let mut found_instruction = false;
    for instruction in received_pty_instructions.lock().iter() {
        if let PtyInstruction::OpenInPlaceEditor(
            scrollback_contents_file,
            terminal_id,
            client_id,
            _,
        ) = instruction
        {
            assert_eq!(scrollback_contents_file, &PathBuf::from(&dumped_file_name));
            assert_eq!(terminal_id, &Some(1));
            assert_eq!(
                client_id,
                &ClientTabIndexOrPaneId::ClientId(crate::ClientId::new(1))
            );
            found_instruction = true;
        }
    }
    assert!(found_instruction);
}

#[test]
pub fn send_cli_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::ScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    // we send two actions here because only the last line in the pane is empty, so one action
    // won't show in a render
    send_cli_action_to_server(&session_metadata, cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, cli_action.clone(), client_id);

    // The pane's own scroll indicator reports the settled position, so waiting for it is exact.
    // The previous version snapshotted every render *and* the render count, both of which are
    // properties of how the screen thread happened to batch its work rather than of scrolling.
    let settled = wait_for_settled_render(
        &received_server_instructions,
        size,
        "a render showing the pane scrolled up by two lines",
        |snapshot| snapshot.contains("SCROLL:  2/13"),
    );

    mock_screen.teardown(vec![server_instruction, screen_thread]);

    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_up_cli_action = CliAction::ScrollUp { pane_id: None };
    let scroll_down_cli_action = CliAction::ScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    // scroll up some
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);

    wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to scroll up before scrolling down",
        |snapshot| snapshot.contains("SCROLL:  4/13"),
    );
    // Exclude earlier frames, which may already show the target position.
    let checkpoint = received_server_instructions.lock().len();

    // scroll down some
    send_cli_action_to_server(&session_metadata, scroll_down_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_down_cli_action.clone(), client_id);
    let settled = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "the pane to reach the requested scroll position",
        |_, snapshot| snapshot.contains("SCROLL:  2/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_scroll_to_bottom_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_up_cli_action = CliAction::ScrollUp { pane_id: None };
    let scroll_to_bottom_action = CliAction::ScrollToBottom { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    // scroll up some
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);

    wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to scroll up before scrolling down",
        |snapshot| snapshot.contains("SCROLL:  4/13"),
    );
    // Exclude earlier frames, which may already show the target position.
    let checkpoint = received_server_instructions.lock().len();

    // scroll to bottom
    send_cli_action_to_server(
        &session_metadata,
        scroll_to_bottom_action.clone(),
        client_id,
    );
    let settled = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "the pane to reach the requested scroll position",
        |_, snapshot| snapshot.contains("SCROLL:  0/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_scroll_to_top_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_to_top_action = CliAction::ScrollToTop { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    // scroll to top
    send_cli_action_to_server(&session_metadata, scroll_to_top_action.clone(), client_id);
    let settled = wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to reach the requested scroll position",
        |snapshot| snapshot.contains("SCROLL:  13/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_page_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let page_scroll_up_action = CliAction::PageScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, page_scroll_up_action, client_id);
    let settled = wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to reach the requested scroll position",
        |snapshot| snapshot.contains("SCROLL:  9/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_page_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let page_scroll_up_action = CliAction::PageScrollUp { pane_id: None };
    let page_scroll_down_action = CliAction::PageScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));

    // scroll up some
    send_cli_action_to_server(&session_metadata, page_scroll_up_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, page_scroll_up_action.clone(), client_id);

    wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to scroll up before scrolling down",
        |snapshot| snapshot.contains("SCROLL:  13/13"),
    );
    // Exclude earlier frames, which may already show the target position.
    let checkpoint = received_server_instructions.lock().len();

    // scroll down
    send_cli_action_to_server(&session_metadata, page_scroll_down_action, client_id);
    let settled = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "the pane to reach the requested scroll position",
        |_, snapshot| snapshot.contains("SCROLL:  5/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_half_page_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let half_page_scroll_up_action = CliAction::HalfPageScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, half_page_scroll_up_action, client_id);
    let settled = wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to reach the requested scroll position",
        |snapshot| snapshot.contains("SCROLL:  4/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_half_page_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let half_page_scroll_up_action = CliAction::HalfPageScrollUp { pane_id: None };
    let half_page_scroll_down_action = CliAction::HalfPageScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));

    // scroll up some
    send_cli_action_to_server(
        &session_metadata,
        half_page_scroll_up_action.clone(),
        client_id,
    );
    send_cli_action_to_server(
        &session_metadata,
        half_page_scroll_up_action.clone(),
        client_id,
    );

    wait_for_settled_render(
        &received_server_instructions,
        size,
        "the pane to scroll up before scrolling down",
        |snapshot| snapshot.contains("SCROLL:  8/13"),
    );
    // Exclude earlier frames, which may already show the target position.
    let checkpoint = received_server_instructions.lock().len();

    // scroll down
    send_cli_action_to_server(&session_metadata, half_page_scroll_down_action, client_id);
    let settled = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "the pane to reach the requested scroll position",
        |_, snapshot| snapshot.contains("SCROLL:  4/13"),
    );
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    assert_snapshot!(settled);
}

#[test]
pub fn send_cli_toggle_full_screen_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_full_screen_action = CliAction::ToggleFullscreen { pane_id: None };
    send_cli_action_to_server(&session_metadata, toggle_full_screen_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_toggle_pane_frames_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_frames_action = CliAction::TogglePaneFrames;
    send_cli_action_to_server(&session_metadata, toggle_pane_frames_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_toggle_active_tab_sync_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_toggle_active_tab_sync_action = CliAction::ToggleActiveSyncTab { tab_id: None };
    let cli_write_action = CliAction::Write {
        bytes: vec![102, 111, 111],
        pane_id: None,
    };
    send_cli_action_to_server(
        &session_metadata,
        cli_toggle_active_tab_sync_action,
        client_id,
    );
    send_cli_action_to_server(&session_metadata, cli_write_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    // here we should have 2 Write instructions, one for each pane
    assert_snapshot!(format!("{:?}", received_write_instructions));
}

#[test]
pub fn send_cli_new_pane_action_with_default_parameters() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn web_new_pane_in_tab_action_targets_requested_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );

    // This is exactly the Action the web control bridge produces for the
    // browser `NewPaneInTab { tab_id }` payload.
    let action = Action::NewTiledPane {
        direction: None,
        command: None,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(TabId::new(0)),
    };
    route_arbitrary_action_to_server(&session_metadata, action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawned_with_tab_index = received_pty_instructions.lock().iter().any(|i| match i {
        PtyInstruction::SpawnTerminal(_, _, _, _, client_tab_index_or_pane_id, ..) => {
            matches!(
                client_tab_index_or_pane_id,
                ClientTabIndexOrPaneId::TabIndex(TAB_0)
            )
        },
        _ => false,
    });
    assert!(
        spawned_with_tab_index,
        "NewPaneInTab must spawn a terminal targeting the requested tab index; got {:?}",
        *received_pty_instructions.lock()
    );
}

#[test]
pub fn send_cli_new_pane_action_with_split_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        remote_host: None,

        direction: Some(Direction::Right),
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn send_cli_new_pane_action_with_command_and_cwd() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        remote_host: None,

        direction: Some(Direction::Right),
        command: vec!["htop".into()],
        plugin: None,
        cwd: Some("/some/folder".into()),
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let new_pane_instruction = received_pty_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::SpawnTerminal(..)))
        .cloned();

    assert_snapshot!(format!("{:?}", new_pane_instruction));
}

#[test]
pub fn send_cli_new_pane_action_with_floating_pane_and_coordinates() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        remote_host: None,

        direction: Some(Direction::Right),
        command: vec!["htop".into()],
        plugin: None,
        cwd: Some("/some/folder".into()),
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: Some("10".to_owned()),
        y: None,
        width: Some("20%".to_owned()),
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn send_cli_edit_action_with_default_parameters() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn send_cli_edit_action_with_line_number() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: None,
        line_number: Some(100),
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn send_cli_edit_action_with_split_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: Some(Direction::Down),
        line_number: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock()));
}

#[test]
pub fn send_cli_switch_mode_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let cli_switch_mode = CliAction::SwitchMode {
        input_mode: InputMode::Locked,
    };
    send_cli_action_to_server(&session_metadata, cli_switch_mode, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);

    let switch_mode_action = received_server_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, ServerInstruction::ChangeMode(..)))
        .cloned();

    assert_snapshot!(format!("{:?}", switch_mode_action));
}

#[test]
pub fn send_cli_toggle_pane_embed_or_float() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_embed_or_floating = CliAction::TogglePaneEmbedOrFloating { pane_id: None };
    // first time to float
    send_cli_action_to_server(
        &session_metadata,
        toggle_pane_embed_or_floating.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    // second time to embed
    send_cli_action_to_server(
        &session_metadata,
        toggle_pane_embed_or_floating.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let _snapshot_count = snapshots.len();
    let last_three_snapshots = snapshots.clone().into_iter().rev().take(3).rev(); // we do this to
                                                                                  // prevent extra
                                                                                  // renders from
                                                                                  // throwing us
                                                                                  // off
    for (_cursor_coordinates, snapshot) in last_three_snapshots.clone() {
        eprintln!("{}", snapshot);
    }
    for (_cursor_coordinates, snapshot) in last_three_snapshots {
        assert_snapshot!(snapshot.to_string());
    }
}

#[test]
pub fn send_cli_toggle_floating_panes() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_embed_or_floating = CliAction::TogglePaneEmbedOrFloating { pane_id: None };
    let toggle_floating_panes = CliAction::ToggleFloatingPanes { tab_id: None };
    // float the focused pane
    send_cli_action_to_server(&session_metadata, toggle_pane_embed_or_floating, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // toggle floating panes (will hide the floated pane from the previous action)
    send_cli_action_to_server(&session_metadata, toggle_floating_panes.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // toggle floating panes (will show the floated pane)
    send_cli_action_to_server(&session_metadata, toggle_floating_panes.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_close_pane_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let close_pane_action = CliAction::ClosePane { pane_id: None };
    send_cli_action_to_server(&session_metadata, close_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_new_tab_action_default_params() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let new_tab_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: None,
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let received_plugin_instructions = received_plugin_instructions.lock();
    let new_tab_action = received_plugin_instructions
        .iter()
        .find(|instruction| matches!(instruction, PluginInstruction::NewTab(..)));
    assert_snapshot!(format!("{:#?}", new_tab_action));
}

#[test]
pub fn send_cli_new_tab_action_with_name_and_layout() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let new_tab_action = CliAction::NewTab {
        name: Some("my-awesome-tab-name".into()),
        layout: Some(PathBuf::from(format!(
            "{}/src/unit/fixtures/layout-with-three-panes.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout_string: None,
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let new_tab_instruction = received_plugin_instructions
        .lock()
        .iter()
        .rev()
        .find(|i| matches!(i, PluginInstruction::NewTab(..)))
        .unwrap()
        .clone();
    let output = format!("{:#?}", new_tab_instruction);
    assert_snapshot!(output);
}

#[test]
pub fn send_cli_next_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_next_tab = CliAction::GoToNextTab;
    send_cli_action_to_server(&session_metadata, goto_next_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![server_thread, screen_thread]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_previous_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_previous_tab = CliAction::GoToPreviousTab;
    send_cli_action_to_server(&session_metadata, goto_previous_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_goto_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_tab = CliAction::GoToTab {
        index: TabNumber::new(1),
    };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_close_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let close_tab = CliAction::CloseTab { tab_id: None };
    send_cli_action_to_server(&session_metadata, close_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_rename_tab() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let rename_tab = CliAction::RenameTab {
        name: "new-tab-name".into(),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let plugin_rename_tab_instruction = received_plugin_instructions
        .lock()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Update(updates) => updates
                .iter()
                .find(|u| matches!(u, (_, _, Event::TabUpdate(..))))
                .is_some(),
            _ => false,
        })
        .cloned();
    assert_snapshot!(format!("{:#?}", plugin_rename_tab_instruction))
}

#[test]
pub fn send_cli_undo_rename_tab() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let second_tab_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let rename_tab = CliAction::RenameTab {
        name: "new-tab-name".into(),
        tab_id: None,
    };
    let undo_rename_tab = CliAction::UndoRenameTab { tab_id: None };
    // first rename the tab
    send_cli_action_to_server(&session_metadata, rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // then undo the tab rename to go back to the default name
    send_cli_action_to_server(&session_metadata, undo_rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let plugin_undo_rename_tab_instruction = received_plugin_instructions
        .lock()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Update(updates) => updates
                .iter()
                .find(|u| matches!(u, (_, _, Event::TabUpdate(..))))
                .is_some(),
            _ => false,
        })
        .cloned();
    assert_snapshot!(format!("{:#?}", plugin_undo_rename_tab_instruction))
}

#[test]
pub fn send_cli_query_tab_names_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(TiledPaneLayout::default());
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let query_tab_names = CliAction::QueryTabNames;
    send_cli_action_to_server(&session_metadata, query_tab_names, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let log_tab_names_instruction = received_server_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, ServerInstruction::Log(..)))
        .cloned();
    assert_snapshot!(format!("{:#?}", log_tab_names_instruction));
}

#[test]
fn queried_tab_names_can_be_used_for_navigation() {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 10 });
    mock_screen.config.options.pane_frame_style = Some(PaneFrameStyle::Titles);
    mock_screen
        .to_screen
        .send(ScreenInstruction::SetCliEventSubscribersPresent(true))
        .unwrap();
    let client_id = mock_screen.main_client_id;
    mock_screen.new_tab(TiledPaneLayout::default());
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    send_cli_action_to_server(
        &session_metadata,
        CliAction::RenameTab {
            name: "work".into(),
            tab_id: Some(TAB_0),
        },
        client_id,
    );
    send_cli_action_to_server(&session_metadata, CliAction::QueryTabNames, client_id);
    let names = received_server_instructions.wait_until(
        "the stored tab names from the CLI query",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ServerInstruction::Log(names, _, _) => Some(names.clone()),
                    _ => None,
                })
        },
    );
    assert_eq!(names, vec!["work", "Tab #2"]);
    let wait_for_active_tab = |tab_id| {
        received_plugin_instructions.wait_until(
            "navigation to the requested tab without creating another tab",
            yinz_test_support::default_timeout(),
            |instructions| {
                instructions.iter().rev().find_map(|instruction| {
                    let PluginInstruction::Update(updates) = instruction else {
                        return None;
                    };
                    updates.iter().find_map(|(_, _, event)| match event {
                        Event::TabUpdate(tabs)
                            if tabs.iter().any(|tab| tab.tab_id == tab_id && tab.active) =>
                        {
                            Some(tabs.len())
                        },
                        _ => None,
                    })
                })
            },
        )
    };
    send_cli_action_to_server(
        &session_metadata,
        CliAction::GoToTab {
            index: TabNumber::new(1),
        },
        client_id,
    );
    assert_eq!(wait_for_active_tab(TAB_0), 2);
    for create in [false, true] {
        received_plugin_instructions.lock().clear();
        send_cli_action_to_server(
            &session_metadata,
            CliAction::GoToTab {
                index: TabNumber::new(2),
            },
            client_id,
        );
        assert_eq!(wait_for_active_tab(TabId::new(1)), 2);
        received_plugin_instructions.lock().clear();
        send_cli_action_to_server(
            &session_metadata,
            CliAction::GoToTabName {
                name: names[0].clone(),
                create,
            },
            client_id,
        );
        assert_eq!(wait_for_active_tab(TAB_0), 2);
    }
    mock_screen.teardown(vec![plugin_thread, server_thread, screen_thread]);
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let pty_fill_plugin_cwd_instruction = received_pty_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", pty_fill_plugin_cwd_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action_when_plugin_is_already_loaded() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    let existing_plugin_pane = TiledPaneLayout {
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), existing_plugin_pane];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![plugin_thread, server_thread, screen_thread]);

    let plugin_load_instruction_sent = received_plugin_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PluginInstruction::Load(..)))
        .is_some();
    assert!(
        !plugin_load_instruction_sent,
        "Plugin Load instruction should not be sent for an already loaded plugin"
    );
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    assert_eq!(
        snapshot_count, 2,
        "Another render was sent for focusing the already loaded plugin"
    );
    for (cursor_coordinates, _snapshot) in snapshots.iter().skip(1) {
        assert!(
            cursor_coordinates.is_none(),
            "Cursor moved to existing plugin in final snapshot indicating focus changed"
        );
    }
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action_when_plugin_is_already_loaded_for_plugin_alias() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    let existing_plugin_pane = TiledPaneLayout {
        run: Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias {
            name: "fixture_plugin_for_tests".to_owned(),
            configuration: Some(Default::default()),
            run_plugin: Some(RunPlugin {
                _allow_exec_host_cmd: false,
                location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
                configuration: Default::default(),
                ..Default::default()
            }),
            ..Default::default()
        }))),
        ..Default::default()
    };
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), existing_plugin_pane];
    let screen_thread = mock_screen.run_with_alias(Some(initial_layout), vec![]);
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "fixture_plugin_for_tests".to_owned(),
        configuration: Default::default(),
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    wait_for_render_after(
        &received_server_instructions,
        size,
        0,
        "the existing plugin has focus",
        |cursor, _| cursor.is_none(),
    );
    mock_screen.teardown(vec![plugin_thread, server_thread, screen_thread]);

    let plugin_load_instruction_sent = received_plugin_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PluginInstruction::Load(..)))
        .is_some();
    assert!(
        !plugin_load_instruction_sent,
        "Plugin Load instruction should not be sent for an already loaded plugin"
    );
}
