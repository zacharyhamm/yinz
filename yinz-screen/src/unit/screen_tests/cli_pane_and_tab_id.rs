use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

// ==========================================
// Category 3: MockScreen end-to-end CLI tests
// ==========================================

#[test]
pub fn send_cli_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ScrollUp with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ScrollDown with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_scroll_to_top_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollToTop {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ScrollToTop with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_scroll_to_bottom_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollToBottom {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ScrollToBottom with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_page_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PageScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: PageScrollUp with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_page_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PageScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: PageScrollDown with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_half_page_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::HalfPageScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: HalfPageScrollUp with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_half_page_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::HalfPageScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: HalfPageScrollDown with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_resize_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: Resize with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_move_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: MovePane with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_move_pane_backwards_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MovePaneBackwards {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: MovePaneBackwards with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_clear_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::Clear {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: Clear with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_edit_scrollback_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_0".to_string()),
        ansi: false,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: EditScrollback with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_toggle_fullscreen_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleFullscreen {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ToggleFullscreen with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_toggle_pane_embed_or_floating_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::TogglePaneEmbedOrFloating {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: TogglePaneEmbedOrFloating with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_close_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ClosePane {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ClosePane with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_rename_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::RenamePane {
        name: "targeted-name".to_string(),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: RenamePane with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_undo_rename_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::UndoRenamePane {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: UndoRenamePane with pane_id CLI action completed without errors
}

#[test]
pub fn send_cli_toggle_pane_pinned_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::TogglePanePinned {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: TogglePanePinned with pane_id CLI action completed without errors
}

// TAB-TARGETING MockScreen tests

#[test]
pub fn send_cli_close_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::CloseTab {
        tab_id: Some(TabId::new(1)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: CloseTab with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_rename_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::RenameTab {
        name: "targeted-tab".to_string(),
        tab_id: Some(TabId::new(1)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: RenameTab with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_undo_rename_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::UndoRenameTab {
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: UndoRenameTab with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_toggle_active_sync_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleActiveSyncTab {
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ToggleActiveSyncTab with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_toggle_floating_panes_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleFloatingPanes {
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: ToggleFloatingPanes with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_previous_swap_layout_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PreviousSwapLayout {
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: PreviousSwapLayout with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_next_swap_layout_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::NextSwapLayout {
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: NextSwapLayout with tab_id CLI action completed without errors
}

#[test]
pub fn send_cli_move_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MoveTab {
        direction: Direction::Right,
        tab_id: Some(TabId::new(0)),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: MoveTab with tab_id CLI action completed without errors
}

// ==========================================
// Category 4: Direct Screen method tests
// ==========================================

#[test]
pub fn move_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    let original_pos_0 = screen.get_tab_by_id(TabId::new(0)).unwrap().position;
    let original_pos_1 = screen.get_tab_by_id(TabId::new(1)).unwrap().position;
    screen
        .move_tab_by_id(TabId::new(0), Direction::Right)
        .expect("TEST");
    assert_eq!(
        screen.get_tab_by_id(TabId::new(0)).unwrap().position,
        original_pos_1
    );
    assert_eq!(
        screen.get_tab_by_id(TabId::new(1)).unwrap().position,
        original_pos_0
    );
}

// ==========================================
// Category 5: ANSI flag tests
// ==========================================

#[test]
pub fn send_cli_dump_screen_action_with_ansi() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10);
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
        path: Some(PathBuf::from("/tmp/foo_ansi")),
        full: true,
        pane_id: None,
        ansi: true,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        dumped_content.contains("\x1b["),
        "Dumped file should contain ANSI escape codes when ansi flag is true. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_dump_screen_action_without_ansi_strips_codes() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10);
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
        path: Some(PathBuf::from("/tmp/foo_plain")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        !dumped_content.contains("\x1b["),
        "Dumped file should NOT contain ANSI escape codes when ansi flag is false. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_edit_scrollback_action_with_ansi() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(10);
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
        ansi: true,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        dumped_content.contains("\x1b["),
        "Edit scrollback dump should contain ANSI escape codes when ansi flag is true. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_edit_scrollback_with_pane_id_and_ansi() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_0".to_string()),
        ansi: true,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    // no panic above means: EditScrollback with pane_id and ansi CLI action completed without errors
}

#[test]
fn subscriber_ansi_flag_preserved_in_subscription() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        crate::ClientId::new(101),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        true,
    );

    assert!(
        !screen
            .pane_render_subscribers
            .get(&ClientId::new(100))
            .unwrap()
            .ansi,
        "Subscriber 100 should have ansi=false"
    );
    assert!(
        screen
            .pane_render_subscribers
            .get(&ClientId::new(101))
            .unwrap()
            .ansi,
        "Subscriber 101 should have ansi=true"
    );
}

#[test]
fn subscriber_ansi_and_plain_receive_different_content() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, TabId::new(0));

    // Subscribe plain and ansi subscribers
    screen.subscribe_to_pane_renders(
        crate::ClientId::new(100),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        crate::ClientId::new(101),
        vec![yinz_types::data::PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1),
        )],
        None,
        true,
    );

    // Clear initial messages
    messages
        .lock()
        .unwrap()
        .get_mut(&ClientId::new(100))
        .unwrap()
        .clear();
    messages
        .lock()
        .unwrap()
        .get_mut(&ClientId::new(101))
        .unwrap()
        .clear();

    // Reset previous viewports to force delivery
    screen
        .pane_render_subscribers
        .get_mut(&ClientId::new(100))
        .unwrap()
        .previous_viewports
        .insert(
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            vec!["old".to_string()],
        );
    screen
        .pane_render_subscribers
        .get_mut(&ClientId::new(101))
        .unwrap()
        .previous_viewports
        .insert(
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            vec!["old".to_string()],
        );

    // Build plain and ansi maps with different content
    let mut plain_map = HashMap::new();
    plain_map.insert(
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        PaneContents {
            viewport: vec!["plain text".to_string()],
            ..Default::default()
        },
    );
    let mut ansi_map = HashMap::new();
    ansi_map.insert(
        yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        PaneContents {
            viewport: vec!["\x1b[31mred text\x1b[0m".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&plain_map, Some(&ansi_map));

    let msgs = messages.lock().unwrap();
    let plain_msgs = msgs.get(&ClientId::new(100)).unwrap();
    let ansi_msgs = msgs.get(&ClientId::new(101)).unwrap();

    assert_eq!(
        plain_msgs.len(),
        1,
        "Plain subscriber should receive one update"
    );
    assert_eq!(
        ansi_msgs.len(),
        1,
        "Ansi subscriber should receive one update"
    );

    match &plain_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            assert_eq!(viewport, &vec!["plain text".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
    match &ansi_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            assert_eq!(viewport, &vec!["\x1b[31mred text\x1b[0m".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn integration_subscribe_with_ansi_flag() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: crate::ClientId::new(100),
            pane_ids: vec![yinz_types::data::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(0),
            )],
            scrollback: None,
            ansi: true,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        "\x1b[31mred text\x1b[0m\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&ClientId::new(100)).unwrap_or(&vec![]).clone();

    assert!(
        subscriber_msgs.len() >= 2,
        "Should have at least initial + update, got {}",
        subscriber_msgs.len()
    );

    match &subscriber_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { is_initial, .. } => {
            assert!(*is_initial, "First message should be initial");
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }

    let has_ansi_content = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial: false,
            viewport,
            ..
        } => viewport.iter().any(|line| line.contains("\x1b[")),
        _ => false,
    });
    assert!(
        has_ansi_content,
        "ANSI subscriber should receive viewport lines with ANSI escape codes. Messages: {:?}",
        subscriber_msgs
    );
}
