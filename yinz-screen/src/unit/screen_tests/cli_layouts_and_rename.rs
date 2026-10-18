use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
pub fn send_cli_new_tab_action_with_layout_string() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10);
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
    // Same layout as layout-with-three-panes.kdl but passed as a string
    let new_tab_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n    pane\n}\n".into()),
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
pub fn send_cli_new_tab_action_with_layout_string_and_name() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10);
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
        name: Some("my-string-layout-tab".into()),
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n    pane\n}\n".into()),
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
pub fn send_cli_new_pane_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
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
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock();
    // Verify that the PTY instruction uses TabIndex(TabId(0)) instead of ClientId
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(TabId(0))"),
        "Expected TabIndex(TabId(0)) in PTY instructions, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_floating_pane_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
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
        borderless: None,
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(TabId(0))"),
        "Expected TabIndex(TabId(0)) in PTY instructions for floating pane, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_edit_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
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
        file: PathBuf::from("/tmp/test.rs"),
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
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(TabId(0))"),
        "Expected TabIndex(TabId(0)) in PTY instructions for edit, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_pane_action_with_tab_id_and_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
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
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(TabId(0))"),
        "Expected TabIndex(TabId(0)) in PTY instructions with direction, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_pane_action_with_tab_id_and_stacked() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
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
        command: vec!["ls".into()],
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
        stacked: true,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(TabId(0))"),
        "Expected TabIndex(TabId(0)) in PTY instructions with stacked, got: {}",
        pty_debug
    );
}

#[test]
fn cli_rename_active_pane_via_screen_replaces_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));

    // First give the pane a name
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // Now rename via the active pane path (what CLI rename without --pane-id does)
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("spark".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "spark",
        "CLI rename should fully replace the name"
    );
}

#[test]
fn cli_rename_active_pane_single_char_via_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));

    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // Single char rename via active pane path
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("x".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "x",
        "Single-char CLI rename should replace, not append"
    );
}

#[test]
fn cli_rename_focused_pane_single_char_via_rename_active_pane() {
    // Tests that single-char CLI rename via RenameActivePane (the path used
    // by `yinz action rename-pane "x"` without --pane-id) correctly
    // replaces the existing name.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));

    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // CLI rename single char via RenameActivePane (full replacement)
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("x".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "x",
        "Single-char CLI rename should replace the name, not append"
    );
}

#[test]
pub fn pty_bytes_and_hold_pane_buffered_before_new_pane() {
    // Regression test: when a command exits very quickly (e.g. `yinz run -- echo hello`),
    // PtyBytes and HoldPane can arrive at the screen thread before NewPane because the async
    // reader and quit_cb run on separate threads. This test verifies that such early-arriving
    // events are buffered and replayed once NewPane is processed.
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // The initial layout creates pane id 0. We will use pane id 2 for the new pane
    // (id 1 is used by the plugin in the initial layout).
    let new_pane_id = 2;

    // Simulate the race: send PtyBytes for the new pane BEFORE NewPane
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(new_pane_id),
        "hello\r\n".as_bytes().to_vec(),
    ));

    // Send HoldPane before NewPane as well
    let run_command = RunCommand {
        command: PathBuf::from("echo"),
        args: vec!["hello".to_string()],
        hold_on_close: true,
        ..Default::default()
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::HoldPane(
        PaneId::Terminal(crate::TerminalId::new(new_pane_id)),
        Some(0),
        run_command,
    ));

    // Small sleep to ensure the above messages are processed (and buffered) first
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Now send NewPane — this should replay the buffered PtyBytes and HoldPane
    let _ = mock_screen.to_screen.send(ScreenInstruction::NewPane(
        PaneId::Terminal(crate::TerminalId::new(new_pane_id)),
        Some("echo hello".to_string()),
        None, // hold_for_command
        None, // invoked_with
        NewPanePlacement::default(),
        false, // start_suppressed
        ClientTabIndexOrPaneId::ClientId(client_id),
        None,  // completion_tx
        false, // set_blocking
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Use DumpScreen to verify the pane received the bytes
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/dump_early_bytes")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let filesystem = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped = filesystem
        .values()
        .next()
        .expect("DumpScreen should have written a file");
    assert!(
        dumped.contains("hello"),
        "Pane should contain the buffered output 'hello', but got: {:?}",
        dumped
    );
}

#[test]
fn pty_bytes_buffered_before_replace_pane_are_replayed() {
    for close_replaced_pane in [false, true] {
        let size = Size { cols: 80, rows: 20 };
        let mut mock = MockScreen::new(size);
        let screen_thread = mock.run(None, vec![]);
        let server_events = Watched::new(vec![]);
        let server_receiver = mock.server_receiver.take().unwrap();
        let server_thread = log_actions_in_thread!(
            server_events,
            ServerInstruction::KillSession,
            server_receiver
        );
        let replacement = TerminalId::new(2);
        // Hold the screen on a rendezvous until all three instructions are queued. A replay
        // at the queue's tail must then put the early bytes after the later bytes and fail.
        let (response_channel, barrier) = crossbeam::channel::bounded(0);
        mock.to_screen
            .send(ScreenInstruction::GetPaneInfo {
                pane_id: PaneId::Terminal(TerminalId::new(0)),
                response_channel,
            })
            .unwrap();
        mock.to_screen
            .send(ScreenInstruction::PtyBytes(
                replacement,
                b"EARLY REPLACEMENT ".to_vec(),
            ))
            .unwrap();
        mock.to_screen
            .send(ScreenInstruction::ReplacePane(
                PaneId::Terminal(replacement),
                None,
                None,
                None,
                yinz_bus::pty::ReplacedPane::closed_if(close_replaced_pane),
                ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(TerminalId::new(0))),
                None,
            ))
            .unwrap();
        mock.to_screen
            .send(ScreenInstruction::PtyBytes(
                replacement,
                b"THEN LATER OUTPUT\r\n".to_vec(),
            ))
            .unwrap();
        barrier
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        wait_for_settled_render(
            &server_events,
            size,
            "replacement displays output received before it was placed",
            |frame| frame.contains("EARLY REPLACEMENT THEN LATER OUTPUT"),
        );
        mock.teardown(vec![server_thread, screen_thread]);
    }
}

/// The first `LogError` line reported to the server that mentions `needle`, once one arrives.
fn wait_for_reported_error_mentioning(
    received_server_instructions: &Watched<Vec<ServerInstruction>>,
    needle: &str,
) -> String {
    received_server_instructions.wait_until_described(
        &format!("an error mentioning {needle:?} to be reported to the server"),
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ServerInstruction::LogError(lines, _, _) => {
                        lines.iter().find(|line| line.contains(needle)).cloned()
                    },
                    _ => None,
                })
        },
        |instructions| {
            let reported: Vec<&String> = instructions
                .iter()
                .filter_map(|instruction| match instruction {
                    ServerInstruction::LogError(lines, _, _) => lines.first(),
                    _ => None,
                })
                .collect();
            format!(
                "{} instruction(s); reported errors: {reported:?}",
                instructions.len()
            )
        },
    )
}

#[test]
pub fn rename_active_pane_without_a_connected_client_reports_an_error() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    // Instructions are drained in order, so the client is gone by the time the rename runs.
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RemoveClient(client_id));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RenameActivePane(
            "my-new-name".as_bytes().to_vec(),
            client_id,
            None,
        ));
    let reported = wait_for_reported_error_mentioning(&received_server_instructions, "--pane-id");
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        reported.contains("no client is attached"),
        "renaming the focused pane with no client attached says why it failed, got: {reported:?}"
    );
}

#[test]
pub fn rename_active_tab_without_a_connected_client_reports_an_error() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    // Instructions are drained in order, so the client is gone by the time the rename runs.
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RemoveClient(client_id));
    let _ = mock_screen.to_screen.send(ScreenInstruction::UpdateTabName(
        "my-new-tab-name".as_bytes().to_vec(),
        client_id,
        None,
    ));
    let reported = wait_for_reported_error_mentioning(&received_server_instructions, "--tab-id");
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        reported.contains("no client is attached"),
        "renaming the focused tab with no client attached says why it failed, got: {reported:?}"
    );
}
