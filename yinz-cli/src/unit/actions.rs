use super::*;
use std::path::PathBuf;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::data::KeyModifier;
use yinz_types::data::UnblockCondition;
use yinz_types::data::{BareKey, InputMode};
use yinz_types::geometry::{Direction, Resize};
use yinz_types::input::actions::hook_report_actions;

#[test]
fn actions_that_leave_a_clipboard_message() {
    assert!(Action::Copy.leaves_clipboard_message());
    assert!(Action::CopyLastCommandOutput.leaves_clipboard_message());
    // wrapping one does not change what it does to the clipboard
    assert!(Action::SkipConfirm {
        action: Box::new(Action::Copy),
    }
    .leaves_clipboard_message());
    assert!(!Action::SwitchToMode {
        input_mode: InputMode::Normal,
    }
    .leaves_clipboard_message());
    assert!(!Action::SkipConfirm {
        action: Box::new(Action::Quit),
    }
    .leaves_clipboard_message());
}

#[test]
fn test_send_keys_single_key() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["Enter".to_string()],
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::Write {
            key_with_modifier,
            bytes,
            is_kitty_keyboard_protocol,
        } => {
            assert!(key_with_modifier.is_some());
            let key = key_with_modifier.as_ref().unwrap();
            assert_eq!(key.bare_key, BareKey::Enter);
            assert!(key.key_modifiers.is_empty());
            assert!(!bytes.is_empty());
            assert!(*is_kitty_keyboard_protocol);
        },
        _ => panic!("Expected Write action"),
    }
}

#[test]
fn test_send_keys_with_modifier() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl a".to_string()],
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::Write {
            key_with_modifier,
            is_kitty_keyboard_protocol,
            ..
        } => {
            assert!(key_with_modifier.is_some());
            let key = key_with_modifier.as_ref().unwrap();
            assert_eq!(key.bare_key, BareKey::Char('a'));
            assert!(key.key_modifiers.contains(&KeyModifier::Ctrl));
            assert!(*is_kitty_keyboard_protocol);
        },
        _ => panic!("Expected Write action"),
    }
}

#[test]
fn test_send_keys_multiple_keys() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl a".to_string(), "F1".to_string(), "Enter".to_string()],
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 3);
    for action in &actions {
        match action {
            Action::Write {
                is_kitty_keyboard_protocol,
                ..
            } => {
                assert!(*is_kitty_keyboard_protocol);
            },
            _ => panic!("Expected Write action"),
        }
    }
}

#[test]
fn test_send_keys_error_hyphen_syntax() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl-a".to_string()],
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Use spaces instead of hyphens"));
}

#[test]
fn test_send_keys_error_typo() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrll a".to_string()],
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Ctrl") || err.contains("modifier"));
}

#[test]
fn test_send_keys_with_pane_id() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["a".to_string()],
        pane_id: Some("terminal_1".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::WriteToPaneId { pane_id, bytes } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1))
            );
            assert!(!bytes.is_empty());
        },
        _ => panic!("Expected WriteToPaneId action"),
    }
}

#[test]
fn test_send_keys_error_invalid_pane_id() {
    let cli_action = CliAction::SendKeys {
        keys: vec!["a".to_string()],
        pane_id: Some("invalid_id".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Malformed pane id"));
}

// =============================================
// Category 1: Pane-targeting tests
// =============================================

// 1. ScrollUp
#[test]
fn test_resize_with_pane_id() {
    let cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: Some("terminal_3".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::ResizeByPaneId {
            pane_id,
            resize,
            direction,
        } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3))
            );
            assert!(matches!(resize, Resize::Increase));
            assert!(matches!(direction, Some(Direction::Left)));
        },
        _ => panic!("Expected ResizeByPaneId action"),
    }
}

#[test]
fn test_resize_without_pane_id() {
    let cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::Resize { resize, direction } => {
            assert!(matches!(resize, Resize::Increase));
            assert!(matches!(direction, Some(Direction::Left)));
        },
        _ => panic!("Expected Resize action"),
    }
}

// 10. MovePane
#[test]
fn test_move_pane_with_pane_id() {
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: Some("terminal_9".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::MovePaneByPaneId { pane_id, direction } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(9))
            );
            assert!(matches!(direction, Some(Direction::Right)));
        },
        _ => panic!("Expected MovePaneByPaneId action"),
    }
}

#[test]
fn test_move_pane_without_pane_id() {
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::MovePane { direction } => {
            assert!(matches!(direction, Some(Direction::Right)));
        },
        _ => panic!("Expected MovePane action"),
    }
}

// 11. MovePaneBackwards
#[test]
fn test_edit_scrollback_with_pane_id() {
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_15".to_string()),
        ansi: false,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::EditScrollbackByPaneId { pane_id, ansi } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
            );
            assert!(!ansi);
        },
        _ => panic!("Expected EditScrollbackByPaneId action"),
    }
}

#[test]
fn test_rename_pane_with_pane_id() {
    let cli_action = CliAction::RenamePane {
        name: "my-pane".to_string(),
        pane_id: Some("terminal_19".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::RenamePaneByPaneId { pane_id, name } => {
            assert_eq!(
                *pane_id,
                Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                    19
                )))
            );
            assert_eq!(name, &"my-pane".as_bytes().to_vec());
        },
        _ => panic!("Expected RenamePaneByPaneId action"),
    }
}

#[test]
fn test_rename_pane_without_pane_id() {
    let cli_action = CliAction::RenamePane {
        name: "my-pane".to_string(),
        pane_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::RenamePaneByPaneId { pane_id, name } => {
            assert!(pane_id.is_none());
            assert_eq!(name, &"my-pane".as_bytes().to_vec());
        },
        _ => panic!("Expected RenamePaneByPaneId action"),
    }
}

// 18. UndoRenamePane
#[test]
fn test_scroll_up_with_invalid_pane_id() {
    let cli_action = CliAction::ScrollUp {
        pane_id: Some("invalid_id".to_string()),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Malformed pane id"));
}

// =============================================
// Category 1: Tab-targeting tests
// =============================================

// 20. CloseTab
#[test]
fn test_close_tab_with_tab_id() {
    let cli_action = CliAction::CloseTab {
        tab_id: Some(TabId::new(5)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::CloseTabById { id } => {
            assert_eq!(*id, TabId::new(5));
        },
        _ => panic!("Expected CloseTabById action"),
    }
}

#[test]
fn test_close_tab_without_tab_id() {
    let cli_action = CliAction::CloseTab { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::CloseTab));
}

#[test]
fn test_set_dark_theme_cli_to_action() {
    let result = crate::actions::actions_from_cli(
        CliAction::SetDarkTheme,
        Box::new(|| PathBuf::from("/tmp")),
        None,
    );
    let actions = result.expect("SetDarkTheme conversion should succeed");
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::SetDarkTheme));
}

#[test]
fn test_set_light_theme_cli_to_action() {
    let result = crate::actions::actions_from_cli(
        CliAction::SetLightTheme,
        Box::new(|| PathBuf::from("/tmp")),
        None,
    );
    let actions = result.expect("SetLightTheme conversion should succeed");
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::SetLightTheme));
}

#[test]
fn test_toggle_theme_cli_to_action() {
    let result = crate::actions::actions_from_cli(
        CliAction::ToggleTheme,
        Box::new(|| PathBuf::from("/tmp")),
        None,
    );
    let actions = result.expect("ToggleTheme conversion should succeed");
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::ToggleTheme));
}

// 21. RenameTab
#[test]
fn test_rename_tab_with_tab_id() {
    let cli_action = CliAction::RenameTab {
        name: "my-tab".to_string(),
        tab_id: Some(TabId::new(3)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::RenameTabById { id, name } => {
            assert_eq!(*id, TabId::new(3));
            assert_eq!(name, "my-tab");
        },
        _ => panic!("Expected RenameTabById action"),
    }
}

#[test]
fn test_rename_tab_without_tab_id() {
    let cli_action = CliAction::RenameTab {
        name: "my-tab".to_string(),
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 2);
    assert!(matches!(actions[0], Action::TabNameInput { .. }));
    assert!(matches!(actions[1], Action::TabNameInput { .. }));
}

// 22. UndoRenameTab
#[test]
fn test_undo_rename_tab_with_tab_id() {
    let cli_action = CliAction::UndoRenameTab {
        tab_id: Some(TabId::new(7)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::UndoRenameTabByTabId { id } => {
            assert_eq!(*id, TabId::new(7));
        },
        _ => panic!("Expected UndoRenameTabByTabId action"),
    }
}

#[test]
fn test_undo_rename_tab_without_tab_id() {
    let cli_action = CliAction::UndoRenameTab { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::UndoRenameTab));
}

// 23. ToggleActiveSyncTab
#[test]
fn test_toggle_active_sync_tab_with_tab_id() {
    let cli_action = CliAction::ToggleActiveSyncTab {
        tab_id: Some(TabId::new(2)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::ToggleActiveSyncTabByTabId { id } => {
            assert_eq!(*id, TabId::new(2));
        },
        _ => panic!("Expected ToggleActiveSyncTabByTabId action"),
    }
}

#[test]
fn test_toggle_active_sync_tab_without_tab_id() {
    let cli_action = CliAction::ToggleActiveSyncTab { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::ToggleActiveSyncTab));
}

// 24. ToggleFloatingPanes
#[test]
fn test_toggle_floating_panes_with_tab_id() {
    let cli_action = CliAction::ToggleFloatingPanes {
        tab_id: Some(TabId::new(4)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::ToggleFloatingPanesByTabId { id } => {
            assert_eq!(*id, TabId::new(4));
        },
        _ => panic!("Expected ToggleFloatingPanesByTabId action"),
    }
}

#[test]
fn test_toggle_floating_panes_without_tab_id() {
    let cli_action = CliAction::ToggleFloatingPanes { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::ToggleFloatingPanes));
}

// 25. PreviousSwapLayout
#[test]
fn test_previous_swap_layout_with_tab_id() {
    let cli_action = CliAction::PreviousSwapLayout {
        tab_id: Some(TabId::new(6)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::PreviousSwapLayoutByTabId { id } => {
            assert_eq!(*id, TabId::new(6));
        },
        _ => panic!("Expected PreviousSwapLayoutByTabId action"),
    }
}

#[test]
fn test_previous_swap_layout_without_tab_id() {
    let cli_action = CliAction::PreviousSwapLayout { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::PreviousSwapLayout));
}

// 26. NextSwapLayout
#[test]
fn test_next_swap_layout_with_tab_id() {
    let cli_action = CliAction::NextSwapLayout {
        tab_id: Some(TabId::new(8)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NextSwapLayoutByTabId { id } => {
            assert_eq!(*id, TabId::new(8));
        },
        _ => panic!("Expected NextSwapLayoutByTabId action"),
    }
}

#[test]
fn test_next_swap_layout_without_tab_id() {
    let cli_action = CliAction::NextSwapLayout { tab_id: None };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::NextSwapLayout));
}

// 27. MoveTab
#[test]
fn test_move_tab_with_tab_id() {
    let cli_action = CliAction::MoveTab {
        direction: Direction::Right,
        tab_id: Some(TabId::new(10)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::MoveTabByTabId { id, direction } => {
            assert_eq!(*id, TabId::new(10));
            assert!(matches!(direction, Direction::Right));
        },
        _ => panic!("Expected MoveTabByTabId action"),
    }
}

#[test]
fn test_move_tab_without_tab_id() {
    let cli_action = CliAction::MoveTab {
        direction: Direction::Right,
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::MoveTab { direction } => {
            assert!(matches!(direction, Direction::Right));
        },
        _ => panic!("Expected MoveTab action"),
    }
}

// 28. ANSI flag tests

#[test]
fn test_edit_scrollback_with_ansi_flag() {
    let cli_action = CliAction::EditScrollback {
        pane_id: None,
        ansi: true,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], Action::EditScrollback { ansi: true }));
}

#[test]
fn test_edit_scrollback_with_pane_id_and_ansi() {
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_15".to_string()),
        ansi: true,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::EditScrollbackByPaneId { pane_id, ansi } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
            );
            assert!(*ansi);
        },
        _ => panic!("Expected EditScrollbackByPaneId action"),
    }
}

#[test]
fn test_dump_screen_with_ansi_flag() {
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/test")),
        full: true,
        pane_id: None,
        ansi: true,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::DumpScreen {
            ansi,
            include_scrollback,
            ..
        } => {
            assert!(*ansi);
            assert!(*include_scrollback);
        },
        _ => panic!("Expected DumpScreen action"),
    }
}

#[test]
fn test_dump_screen_with_pane_id_and_ansi() {
    let cli_action = CliAction::DumpScreen {
        path: None,
        full: false,
        pane_id: Some("terminal_5".to_string()),
        ansi: true,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::DumpScreen { pane_id, ansi, .. } => {
            assert_eq!(
                *pane_id,
                Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                    5
                )))
            );
            assert!(*ansi);
        },
        _ => panic!("Expected DumpScreen action"),
    }
}

#[test]
fn test_focus_pane_id() {
    let cli_action = CliAction::FocusPaneId {
        pane_id: "terminal_7".to_string(),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::FocusPaneByPaneId { pane_id } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
            );
        },
        _ => panic!("Expected FocusPaneByPaneId action"),
    }
}

#[test]
fn test_focus_pane_id_bare_int() {
    let cli_action = CliAction::FocusPaneId {
        pane_id: "3".to_string(),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::FocusPaneByPaneId { pane_id } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3))
            );
        },
        _ => panic!("Expected FocusPaneByPaneId action"),
    }
}

#[test]
fn test_focus_pane_id_plugin() {
    let cli_action = CliAction::FocusPaneId {
        pane_id: "plugin_2".to_string(),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::FocusPaneByPaneId { pane_id } => {
            assert_eq!(
                *pane_id,
                PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2))
            );
        },
        _ => panic!("Expected FocusPaneByPaneId action"),
    }
}

#[test]
fn test_focus_pane_id_malformed() {
    let cli_action = CliAction::FocusPaneId {
        pane_id: "invalid_id".to_string(),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
}

#[test]
fn test_new_tab_with_layout_string() {
    let cli_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n}\n".into()),
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
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewTab {
            layout_source: None,
            tiled_layout,
            floating_layouts,
            ..
        } => {
            assert!(tiled_layout.is_some());
            let layout = tiled_layout.as_ref().unwrap();
            // layout { pane; pane } produces a layout with 2 children
            assert_eq!(layout.children.len(), 2);
            assert!(floating_layouts.is_empty());
        },
        _ => panic!("Expected NewTab action"),
    }
}

#[test]
fn test_new_tab_with_invalid_layout_string() {
    let cli_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: Some("invalid { kdl".into()),
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
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
}

#[test]
fn test_override_layout_with_layout_string() {
    let cli_action = CliAction::OverrideLayout {
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n}\n".into()),
        layout_dir: None,
        retain_existing_terminal_panes: false,
        retain_existing_plugin_panes: false,
        apply_only_to_active_tab: false,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::OverrideLayout { tabs, .. } => {
            assert!(!tabs.is_empty());
        },
        _ => panic!("Expected OverrideLayout action"),
    }
}

#[test]
fn test_switch_session_with_layout_string() {
    let cli_action = CliAction::SwitchSession {
        name: "test-session".into(),
        tab_position: None,
        pane_id: None,
        layout: None,
        layout_string: Some("layout {\n    pane\n}\n".into()),
        layout_dir: None,
        cwd: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::SwitchSession { layout, .. } => {
            assert!(matches!(
                layout,
                Some(yinz_types::data::LayoutInfo::Stringified(_))
            ));
        },
        _ => panic!("Expected SwitchSession action"),
    }
}

#[test]
fn test_switch_session_with_invalid_layout_string() {
    let cli_action = CliAction::SwitchSession {
        name: "test-session".into(),
        tab_position: None,
        pane_id: None,
        layout: None,
        layout_string: Some("invalid { kdl".into()),
        layout_dir: None,
        cwd: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
}

/// A remote pane asked for with no command is the far machine's login
/// shell. It has to become a `RunCommandAction` all the same — a `None`
/// command would drop the host on the floor and open a pane here.
fn remote_new_pane(command: Vec<String>, cwd: Option<PathBuf>) -> CliAction {
    CliAction::NewPane {
        remote_host: Some("build-box".to_owned()),

        direction: None,
        command,
        plugin: None,
        cwd,
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
        borderless: None,
        tab_id: None,
    }
}

fn tiled_run_command(cli_action: CliAction) -> RunCommandAction {
    let actions =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None)
            .expect("the cli action should be accepted");
    match &actions[0] {
        Action::NewTiledPane {
            command: Some(command),
            ..
        } => command.clone(),
        other => panic!("expected a tiled pane with a command, got {other:?}"),
    }
}

#[test]
fn a_remote_pane_with_no_command_asks_for_that_machines_login_shell() {
    let command = tiled_run_command(remote_new_pane(vec![], None));
    assert!(command.remote_default_shell);
    assert_eq!(command.target.ssh_host(), Some("build-box"));
    assert_eq!(
        command.command,
        PathBuf::new(),
        "naming a command here would name a path on the wrong machine"
    );
    assert_eq!(
        command.cwd, None,
        "the local working directory is not a directory on the far machine; \
         leaving it unset is what lands the pane in the remote home"
    );
    assert!(
        !command.hold_on_close,
        "logging out of an ssh ends the session; holding it would leave a banner"
    );
}

/// An explicit `--cwd` names a path on the *far* machine, so it must survive
/// unjoined — joining it with this machine's cwd would mangle a relative one
/// and is meaningless for an absolute one.
#[test]
fn an_explicit_cwd_reaches_a_remote_pane_untouched() {
    let command = tiled_run_command(remote_new_pane(vec![], Some(PathBuf::from("srv/www"))));
    assert_eq!(command.cwd, Some(PathBuf::from("srv/www")));

    let with_command = tiled_run_command(remote_new_pane(
        vec!["htop".to_owned()],
        Some(PathBuf::from("/srv/www")),
    ));
    assert_eq!(with_command.cwd, Some(PathBuf::from("/srv/www")));
    assert!(!with_command.remote_default_shell);
    assert_eq!(with_command.command, PathBuf::from("htop"));
}

/// Without a host, no command still means no command — the local pane keeps
/// getting its shell from the server, not from an empty `RunCommandAction`.
#[test]
fn a_local_pane_with_no_command_is_still_commandless() {
    let mut cli_action = remote_new_pane(vec![], None);
    if let CliAction::NewPane { remote_host, .. } = &mut cli_action {
        *remote_host = None;
    }
    let actions =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None)
            .expect("the cli action should be accepted");
    match &actions[0] {
        Action::NewTiledPane { command, .. } => assert!(command.is_none()),
        other => panic!("expected a tiled pane, got {other:?}"),
    }
}

// Tab-targeting for pane creation commands

#[test]
fn test_new_pane_tiled_with_tab_id() {
    let cli_action = CliAction::NewPane {
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
        borderless: None,
        tab_id: Some(TabId::new(3)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewTiledPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(3)));
        },
        _ => panic!("Expected NewTiledPane action"),
    }
}

#[test]
fn test_new_pane_tiled_without_tab_id() {
    let cli_action = CliAction::NewPane {
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
        borderless: None,
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewTiledPane { tab_id, .. } => {
            assert_eq!(*tab_id, None);
        },
        _ => panic!("Expected NewTiledPane action"),
    }
}

#[test]
fn test_new_in_place_pane_with_pane_id_to_replace() {
    let cli_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        pane_id: Some("terminal_4".to_string()),
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
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewInPlacePane {
            pane_id_to_replace, ..
        } => {
            assert_eq!(
                *pane_id_to_replace,
                Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                    4
                )))
            );
        },
        _ => panic!("Expected NewInPlacePane action"),
    }
}

#[test]
fn test_new_in_place_pane_with_malformed_pane_id() {
    let cli_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: false,
        pane_id: Some("not_a_pane".to_string()),
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
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Malformed pane id"));
}

#[test]
fn test_new_pane_floating_with_tab_id() {
    let cli_action = CliAction::NewPane {
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
        tab_id: Some(TabId::new(5)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewFloatingPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(5)));
        },
        _ => panic!("Expected NewFloatingPane action"),
    }
}

#[test]
fn test_new_pane_stacked_with_tab_id() {
    let cli_action = CliAction::NewPane {
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
        tab_id: Some(TabId::new(1)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewStackedPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(1)));
        },
        _ => panic!("Expected NewStackedPane action"),
    }
}

/// A fragment is checked against the caller's config before it leaves the machine, so a
/// typo is an error naming the problem rather than a server log line.
#[test]
fn reconfigure_takes_a_fragment_and_refuses_one_that_does_not_parse() {
    let reconfigure = |kdl: &str, write: bool| {
        crate::actions::actions_from_cli(
            CliAction::Reconfigure {
                kdl: Some(kdl.to_owned()),
                file: None,
                write,
            },
            Box::new(|| PathBuf::from("/tmp")),
            None,
        )
    };
    let fragment = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
    assert_eq!(
        reconfigure(fragment, true).unwrap(),
        vec![Action::Reconfigure {
            config: fragment.to_owned(),
            write_to_disk: true,
        }]
    );
    let refused = reconfigure("options { nonsense", false).unwrap_err();
    assert!(refused.contains("does not parse"), "{refused}");
}

/// `set-hook` is the one-block fragment spelled for you; an empty url unbinds, and a point
/// nobody has heard of is refused with the ones that exist.
#[test]
fn set_hook_is_a_hooks_fragment() {
    let set_hook = |point: &str, url: &str| {
        crate::actions::actions_from_cli(
            CliAction::SetHook {
                point: point.to_owned(),
                url: url.to_owned(),
                write: false,
            },
            Box::new(|| PathBuf::from("/tmp")),
            None,
        )
    };
    assert_eq!(
        set_hook("agent_exit", "file:my-hook.scm").unwrap(),
        vec![Action::Reconfigure {
            config: "hooks {\n    agent_exit \"file:my-hook.scm\"\n}\n".to_owned(),
            write_to_disk: false,
        }]
    );
    assert_eq!(
        set_hook("context-menu", "").unwrap(),
        vec![Action::Reconfigure {
            config: "hooks {\n    context_menu \"\"\n}\n".to_owned(),
            write_to_disk: false,
        }]
    );
    assert!(set_hook("agent_exit", "file:quote\"d.scm").unwrap()[0]
        .to_string()
        .contains("Reconfigure"));
    let refused = set_hook("on_coffee", "file:x.scm").unwrap_err();
    assert!(refused.contains("agent_exit"), "{refused}");
    // Every point the config knows, the CLI accepts - and the near miss is refused.
    for point in yinz_types::input::hooks::HookPoint::ALL {
        let actions = set_hook(point.as_str(), "file:x.scm").unwrap();
        assert_eq!(
            actions,
            vec![Action::Reconfigure {
                config: format!("hooks {{\n    {point} \"file:x.scm\"\n}}\n"),
                write_to_disk: false,
            }]
        );
    }
    let refused = set_hook("agent_started", "file:x.scm").unwrap_err();
    assert!(refused.contains("agent_start,"), "{refused}");
}

/// A hook event does two things from the one process a hook may spend: it sets the pane's
/// record, and it goes out whole as an `AgentEvent` for whoever wants more than a record
/// holds. An event that says nothing about the state still goes out — the summary hangs
/// off text a state says nothing about.
#[test]
fn a_hook_event_sets_the_record_and_broadcasts_itself() {
    let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7));
    let hook = |json: &str| {
        let event: yinz_types::agent::HookEvent = serde_json::from_str(json).expect("parsed");
        hook_report_actions("claude", pane_id, &event, json.to_owned())
    };
    let broadcast = |actions: &[Action], json: &str| match actions.last() {
        Some(Action::AgentEvent(event)) => {
            assert_eq!(event.raw, json, "the event goes out whole");
            assert_eq!(
                event.pane_id,
                yinz_types::internal_ids::TerminalId::new(7),
                "the bare number, which is what a reader keys by"
            );
            // The payload is the agent's own JSON and never says which agent wrote it, so
            // a reader that writes a record of its own is told here.
            assert_eq!(event.kind, "claude");
        },
        other => panic!("{other:?}"),
    };

    let json = r#"{"hook_event_name":"PermissionRequest","session_id":"s1",
                   "tool_name":"Bash","tool_input":{"command":"rm -rf target"}}"#;
    let actions = hook(json);
    assert_eq!(actions.len(), 2);
    match &actions[0] {
        Action::SetPaneAgent {
            pane_id,
            agent: Some(agent),
        } => {
            assert_eq!(
                *pane_id,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
            );
            assert_eq!(agent.state, AgentState::Busy);
            assert_eq!(agent.source, AgentSource::Reported);
            assert_eq!(agent.detail.as_deref(), Some("Bash rm"));
            assert_eq!(agent.waiting_on, None);
            assert_eq!(agent.session_id.as_deref(), Some("s1"));
        },
        other => panic!("{other:?}"),
    }
    broadcast(&actions, json);
    // The event is read once, the way the record is: the same state, the same tool.
    match &actions[1] {
        Action::AgentEvent(event) => {
            assert_eq!(event.hook, "PermissionRequest");
            assert_eq!(event.state, Some(AgentState::Busy));
            assert_eq!(event.session_id.as_deref(), Some("s1"));
            assert_eq!(event.tool_name.as_deref(), Some("Bash"));
        },
        other => panic!("{other:?}"),
    }

    // `SessionEnd` ends the record rather than moving it.
    let json = r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#;
    let actions = hook(json);
    assert_eq!(actions.len(), 2);
    assert!(matches!(
        &actions[0],
        Action::SetPaneAgent { agent: None, .. }
    ));
    broadcast(&actions, json);

    // A notification that means neither waiting nor idle leaves the record alone — and is
    // still broadcast, because `last_assistant_message` and friends live on events a state
    // reading has no opinion about.
    let json = r#"{"hook_event_name":"Notification","notification_type":"auth_success"}"#;
    let actions = hook(json);
    assert_eq!(actions.len(), 1, "nothing to report is nothing to set");
    broadcast(&actions, json);

    // Another agent's question, read the same way and named on the broadcast, so the
    // plugin that hears it does not have to guess that a codex is a claude.
    let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1",
                   "tool_name":"request_user_input"}"#;
    let event: yinz_types::agent::HookEvent = serde_json::from_str(json).expect("parsed");
    let actions = hook_report_actions("codex", pane_id, &event, json.to_owned());
    match &actions[0] {
        Action::SetPaneAgent {
            agent: Some(agent), ..
        } => {
            assert_eq!(agent.kind, "codex");
            assert_eq!(agent.state, AgentState::Waiting);
        },
        other => panic!("{other:?}"),
    }
    match actions.last() {
        Some(Action::AgentEvent(event)) => assert_eq!(event.kind, "codex"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn automatic_permission_records_and_broadcasts_are_busy() {
    for kind in ["claude", "codex"] {
        for mode in ["auto", "bypassPermissions", "dontAsk", "acceptEdits"] {
            let tool = if mode == "acceptEdits" {
                "Edit"
            } else {
                "Bash"
            };
            let json = serde_json::json!({
                "hook_event_name": "PermissionRequest", "permission_mode": mode,
                "tool_name": tool, "tool_input": {"command": "git status", "file_path": "/work/main.rs"}
            }).to_string();
            let event = serde_json::from_str(&json).unwrap();
            let actions = hook_report_actions(
                kind,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
                &event,
                json,
            );
            match &actions[..] {
                [Action::SetPaneAgent {
                    agent: Some(agent), ..
                }, Action::AgentEvent(event)] => {
                    assert_eq!(agent.kind, kind);
                    assert_eq!(agent.state, AgentState::Busy);
                    assert_eq!(agent.waiting_on, None);
                    assert!(!agent.wait_needs_a_person);
                    assert_eq!(
                        agent.detail.as_deref(),
                        Some(if tool == "Bash" {
                            "Bash git"
                        } else {
                            "Edit main.rs"
                        })
                    );
                    assert_eq!(event.kind, kind);
                    assert_eq!(event.state, Some(AgentState::Busy));
                },
                other => panic!("{other:?}"),
            }
            for reminder in ["permission_prompt", "worker_permission_prompt"] {
                let json = serde_json::json!({"hook_event_name": "Notification", "notification_type": reminder, "permission_mode": mode}).to_string();
                let event = serde_json::from_str(&json).unwrap();
                let actions = hook_report_actions(
                    kind,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
                    &event,
                    json,
                );
                if kind == "claude" && reminder == "permission_prompt" {
                    assert!(matches!(&actions[..], [Action::SetPaneAgent {
                        agent: Some(agent), ..
                    }, Action::AgentEvent(event)] if agent.state == AgentState::Waiting
                        && agent.wait_needs_a_person
                        && event.state == Some(AgentState::Waiting)));
                } else {
                    assert!(
                        matches!(&actions[..], [Action::AgentEvent(event)] if event.state.is_none())
                    );
                }
            }
        }
    }
}

/// …but not at any size. This payload is copied into every running plugin's memory,
/// twice per tool call, inside a hook with five seconds to live, and `tool_input` and
/// `tool_response` have no bound at all in Claude Code's schema. Over the cap the event
/// goes out pruned — still an event, and still carrying everything a reader takes.
#[test]
fn a_hook_event_too_big_to_hand_round_is_cut_rather_than_dropped() {
    let json = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "s1",
        "tool_name": "Write",
        "tool_input": {
            "file_path": "/work/src/main.rs",
            "content": "x".repeat(2 * 1024 * 1024),
        },
        // Not on `HookEvent` at all, so no reader here has ever seen it.
        "tool_response": "y".repeat(2 * 1024 * 1024),
    })
    .to_string();
    assert!(json.len() > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);

    let event: yinz_types::agent::HookEvent = serde_json::from_str(&json).expect("parsed");
    let actions = hook_report_actions(
        "claude",
        PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
        &event,
        json,
    );
    let Some(Action::AgentEvent(yinz_types::agent::AgentEvent { raw: payload, .. })) =
        actions.last()
    else {
        panic!("{actions:?}");
    };
    assert!(
        payload.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES,
        "the broadcast is bounded, got {}",
        payload.len()
    );
    let arrived: yinz_types::agent::HookEvent =
        serde_json::from_str(payload).expect("a pruned event is still an event");
    assert_eq!(
        yinz_types::agent::hook_detail(&arrived).as_deref(),
        Some("Write main.rs"),
        "and still says what the row draws"
    );
}

/// `agent-report` is what an agent's hooks call: a state names the record, `--clear` ends
/// it, and the pane is the one the caller is in unless it says otherwise.
#[test]
fn agent_report_sets_or_clears_the_panes_agent_record() {
    let report = |state: Option<&str>, clear: bool| {
        crate::actions::actions_from_cli(
            CliAction::AgentReport {
                pane_id: Some("terminal_4".to_owned()),
                kind: Some(" Codex ".to_owned()),
                state: state.map(str::to_owned),
                detail: Some("rebasing".to_owned()),
                cwd: Some(PathBuf::from("/work/.worktrees/topic")),
                hook_json: false,
                clear,
                run_id: None,
            },
            Box::new(|| PathBuf::from("/tmp")),
            None,
        )
    };
    assert_eq!(
        report(Some("waiting"), false).unwrap(),
        vec![Action::SetPaneAgent {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4)),
            agent: Some(AgentInfo {
                kind: "codex".to_owned(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: Some("rebasing".to_owned()),
                waiting_on: None,
                wait_needs_a_person: true,
                cwd: Some("/work/.worktrees/topic".to_owned()),
                session_id: None,
                run_id: None,
            }),
        }]
    );
    assert_eq!(
        report(None, true).unwrap(),
        vec![Action::SetPaneAgent {
            pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4)),
            agent: None,
        }]
    );
    assert!(
        report(None, false).is_err(),
        "a report has to say something"
    );
    assert!(report(Some("asleep"), false).is_err());
}

/// A script keeping its own history names the run it is keeping. Agents that report a
/// session id of their own do not need this — their run id is derived from it — but a
/// pane whose run id is only ever a local invention has nothing else to be found by
/// after a warp.
#[test]
fn agent_report_carries_the_run_id_it_was_given() {
    let report = |run_id: Option<&str>| {
        crate::actions::actions_from_cli(
            CliAction::AgentReport {
                pane_id: Some("terminal_4".to_owned()),
                kind: None,
                state: Some("busy".to_owned()),
                detail: None,
                cwd: None,
                hook_json: false,
                clear: false,
                run_id: run_id.map(str::to_owned),
            },
            Box::new(|| PathBuf::from("/tmp")),
            None,
        )
        .unwrap()
    };
    let run_id_of = |actions: Vec<Action>| match actions.into_iter().next() {
        Some(Action::SetPaneAgent { agent, .. }) => agent.and_then(|agent| agent.run_id),
        other => panic!("expected a SetPaneAgent, got {other:?}"),
    };
    assert_eq!(run_id_of(report(Some("mine:7"))), Some("mine:7".to_owned()));
    // Unset, not cleared: the pane keeps whatever it already carries, which is what lets a
    // script name its run once rather than on every report.
    assert_eq!(run_id_of(report(None)), None);
    assert_eq!(run_id_of(report(Some("  "))), None, "blank is not an id");
}

/// Each `--block-until-exit*` flag names the condition that ends the wait, and a condition
/// handed in directly (`unblock_condition`, the escape hatch `yinz run` itself uses) wins
/// over the flags.
#[test]
fn blocking_flags_map_to_their_unblock_condition() {
    let build = |success: bool, failure: bool, any: bool, direct: Option<UnblockCondition>| {
        let cli_action = CliAction::NewPane {
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
            stacked: false,
            blocking: false,
            block_until_exit_success: success,
            block_until_exit_failure: failure,
            block_until_exit: any,
            unblock_condition: direct,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        };
        let actions =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None)
                .unwrap();
        match actions.as_slice() {
            [Action::NewBlockingPane {
                unblock_condition, ..
            }] => *unblock_condition,
            other => panic!("expected one NewBlockingPane, got {other:?}"),
        }
    };
    assert_eq!(
        build(true, false, false, None),
        Some(UnblockCondition::OnExitSuccess)
    );
    assert_eq!(
        build(false, true, false, None),
        Some(UnblockCondition::OnExitFailure)
    );
    assert_eq!(
        build(false, false, true, None),
        Some(UnblockCondition::OnAnyExit)
    );
    assert_eq!(
        build(true, false, false, Some(UnblockCondition::OnAnyExit)),
        Some(UnblockCondition::OnAnyExit),
        "a condition given directly wins over the flags"
    );
}

#[test]
fn test_new_pane_blocking_with_tab_id() {
    let cli_action = CliAction::NewPane {
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
        stacked: false,
        blocking: true,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(TabId::new(2)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewBlockingPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(2)));
        },
        _ => panic!("Expected NewBlockingPane action"),
    }
}

#[test]
fn test_edit_with_tab_id() {
    let cli_action = CliAction::Edit {
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
        tab_id: Some(TabId::new(4)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::EditFile { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(4)));
        },
        _ => panic!("Expected EditFile action"),
    }
}

#[test]
fn test_edit_without_tab_id() {
    let cli_action = CliAction::Edit {
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
        tab_id: None,
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::EditFile { tab_id, .. } => {
            assert_eq!(*tab_id, None);
        },
        _ => panic!("Expected EditFile action"),
    }
}

#[test]
fn test_new_pane_plugin_tiled_with_tab_id() {
    let cli_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec![],
        plugin: Some("yinz:strider".into()),
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
        borderless: None,
        tab_id: Some(TabId::new(2)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewTiledPluginPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(2)));
        },
        _ => panic!("Expected NewTiledPluginPane action"),
    }
}

#[test]
fn test_new_pane_plugin_floating_with_tab_id() {
    let cli_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec![],
        plugin: Some("yinz:strider".into()),
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
        tab_id: Some(TabId::new(1)),
    };
    let result =
        crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
    assert!(result.is_ok());
    let actions = result.unwrap();
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Action::NewFloatingPluginPane { tab_id, .. } => {
            assert_eq!(*tab_id, Some(TabId::new(1)));
        },
        _ => panic!("Expected NewFloatingPluginPane action"),
    }
}

// 35 with_pane_id/without_pane_id example tests were removed here. Each
// asserted only which Action variant came out and which pane id it
// carried - both now checked for all 26 swept variants on every run by
// props::each_cli_action_maps_to_its_expected_action_variant.

mod props {
    use super::*;
    use hegel::generators;
    use yinz_types::geometry::{Direction, Resize};

    fn cwd() -> Box<dyn Fn() -> PathBuf> {
        Box::new(|| PathBuf::from("/tmp"))
    }

    /// Every `CliAction` carrying an optional `--pane-id`. All the other
    /// fields are pinned, so the pane id is the only thing that varies
    /// between two calls with different `which` held constant.
    ///
    /// Six of the `CliAction`s that mention a pane id are deliberately
    /// absent, and keep their own example tests:
    ///
    /// * `FocusPaneId`, `ChangeFloatingPaneCoordinates`,
    ///   `TogglePaneBorderless` and `SetPaneBorderless` take a required
    ///   `String`, so they have no "without pane id" form to compare to.
    /// * `NewPane`'s `pane_id` is the pane to *replace* and is only legal
    ///   alongside `in_place`, so it is not the same parameter at all.
    /// * `SwitchSession`'s names a pane in the session being switched to.
    const PANE_ID_ACTIONS: usize = 26;

    /// The prefix of `cli_action_with_pane_id` whose no-`--pane-id` form is
    /// actually pane-id-free. `SetPaneColor` - the last index, and the only
    /// one outside this prefix - requires the caller pane to be resolved
    /// before parsing instead of acting on the focused pane. Everything that
    /// builds `pane_id: None` stops here; the with-a-pane-id sweeps still
    /// run to `PANE_ID_ACTIONS`.
    const PANE_ID_OPTIONAL_ACTIONS: usize = PANE_ID_ACTIONS - 1;

    fn cli_action_with_pane_id(which: usize, pane_id: Option<String>) -> CliAction {
        match which {
            0 => CliAction::Write {
                bytes: vec![10],
                pane_id,
            },
            1 => CliAction::WriteChars {
                chars: "foo".to_owned(),
                pane_id,
            },
            2 => CliAction::Paste {
                chars: "foo".to_owned(),
                pane_id,
            },
            3 => CliAction::SendKeys {
                keys: vec!["Enter".to_owned()],
                pane_id,
            },
            4 => CliAction::Resize {
                resize: Resize::Increase,
                direction: Some(Direction::Left),
                pane_id,
            },
            5 => CliAction::MovePane {
                direction: Some(Direction::Right),
                pane_id,
            },
            6 => CliAction::MovePaneBackwards { pane_id },
            7 => CliAction::Clear { pane_id },
            8 => CliAction::DumpScreen {
                path: Some(PathBuf::from("/tmp/dump")),
                full: false,
                pane_id,
                ansi: false,
            },
            9 => CliAction::EditScrollback {
                pane_id,
                ansi: false,
            },
            10 => CliAction::ScrollUp { pane_id },
            11 => CliAction::ScrollDown { pane_id },
            12 => CliAction::ScrollToBottom { pane_id },
            13 => CliAction::ScrollToTop { pane_id },
            14 => CliAction::PageScrollUp { pane_id },
            15 => CliAction::PageScrollDown { pane_id },
            16 => CliAction::HalfPageScrollUp { pane_id },
            17 => CliAction::HalfPageScrollDown { pane_id },
            18 => CliAction::ToggleFullscreen { pane_id },
            19 => CliAction::ToggleNoUiFullscreen { pane_id },
            20 => CliAction::TogglePaneEmbedOrFloating { pane_id },
            21 => CliAction::ClosePane { pane_id },
            22 => CliAction::RenamePane {
                name: "new-name".to_owned(),
                pane_id,
            },
            23 => CliAction::UndoRenamePane { pane_id },
            24 => CliAction::TogglePanePinned { pane_id },
            25 => CliAction::SetPaneColor {
                pane_id,
                fg: Some("red".to_owned()),
                bg: Some("blue".to_owned()),
                reset: false,
            },
            other => unreachable!("no CliAction at index {}", other),
        }
    }

    /// The `Action` each indexed `CliAction` must produce, as
    /// `(with a pane id, without one)`. The second element is unused past
    /// `PANE_ID_OPTIONAL_ACTIONS` - `SetPaneColor` has no pane-id-free
    /// form - and is left empty there.
    ///
    /// The relational properties below pin that the pane id reaches the
    /// action; this pins *which* action, which is the other half of what
    /// the per-variant example tests asserted. Without it a variant could
    /// route to the wrong ById action and every other property here would
    /// still pass.
    const EXPECTED_ACTIONS: [(&str, &str); PANE_ID_ACTIONS] = [
        ("WriteToPaneId", "Write"),
        ("WriteCharsToPaneId", "WriteChars"),
        ("Paste", "Paste"),
        ("WriteToPaneId", "Write"),
        ("ResizeByPaneId", "Resize"),
        ("MovePaneByPaneId", "MovePane"),
        ("MovePaneBackwardsByPaneId", "MovePaneBackwards"),
        ("ClearScreenByPaneId", "ClearScreen"),
        ("DumpScreen", "DumpScreen"),
        ("EditScrollbackByPaneId", "EditScrollback"),
        ("ScrollUpByPaneId", "ScrollUp"),
        ("ScrollDownByPaneId", "ScrollDown"),
        ("ScrollToBottomByPaneId", "ScrollToBottom"),
        ("ScrollToTopByPaneId", "ScrollToTop"),
        ("PageScrollUpByPaneId", "PageScrollUp"),
        ("PageScrollDownByPaneId", "PageScrollDown"),
        ("HalfPageScrollUpByPaneId", "HalfPageScrollUp"),
        ("HalfPageScrollDownByPaneId", "HalfPageScrollDown"),
        ("ToggleFocusFullscreenByPaneId", "ToggleFocusFullscreen"),
        (
            "ToggleFocusNoUiFullscreenByPaneId",
            "ToggleFocusNoUiFullscreen",
        ),
        (
            "TogglePaneEmbedOrFloatingByPaneId",
            "TogglePaneEmbedOrFloating",
        ),
        ("CloseFocusByPaneId", "CloseFocus"),
        ("RenamePaneByPaneId", "RenamePaneByPaneId"),
        ("UndoRenamePaneByPaneId", "UndoRenamePane"),
        ("TogglePanePinnedByPaneId", "TogglePanePinned"),
        ("SetPaneColor", ""),
    ];

    /// The variant name at the head of an `Action`'s `Debug` rendering.
    fn variant_name(action: &Action) -> String {
        format!("{:?}", action)
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }

    #[hegel::test]
    fn each_cli_action_maps_to_its_expected_action_variant(tc: hegel::TestCase) {
        let (spelling, parsed) = tc.draw(pane_id_spelling());
        for (which, (expected_with, expected_without)) in EXPECTED_ACTIONS.iter().enumerate() {
            let with = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(spelling.clone())),
                cwd(),
                None,
            )
            .unwrap();
            assert_eq!(
                variant_name(&with[0]),
                *expected_with,
                "action {} with pane id {:?} produced {:?}",
                which,
                spelling,
                with[0]
            );
            if which < PANE_ID_OPTIONAL_ACTIONS {
                let without = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, None),
                    cwd(),
                    None,
                )
                .unwrap();
                assert_eq!(
                    variant_name(&without[0]),
                    *expected_without,
                    "action {} without a pane id produced {:?}",
                    which,
                    without[0]
                );
            }
            // ...carrying that exact pane id, not merely some pane id that
            // varies with the input. `the_pane_id_reaches_the_action_it_
            // produces` only shows distinct ids stay distinct, so an
            // off-by-one in the parse would slip past it.
            let rendered = format!("{:?}", with[0]);
            let expected_id = format!("{:?}", parsed);
            assert!(
                rendered.contains(&expected_id),
                "action {} built from {:?} does not carry {}: {}",
                which,
                spelling,
                expected_id,
                rendered
            );
        }
    }

    /// The three spellings `PaneId::from_str` accepts, as
    /// `(rendered id, parsed id)`.
    #[hegel::composite]
    fn pane_id_spelling(tc: &hegel::TestCase) -> (String, PaneId) {
        let n = tc.draw(generators::integers::<u32>());
        if tc.draw(generators::booleans()) {
            (
                format!("plugin_{}", n),
                PaneId::Plugin(yinz_types::internal_ids::PluginId::new(n)),
            )
        } else if tc.draw(generators::booleans()) {
            (
                format!("terminal_{}", n),
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n)),
            )
        } else {
            // a bare integer is a terminal pane id
            (
                format!("{}", n),
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n)),
            )
        }
    }

    // Each property sweeps all of `0..PANE_ID_ACTIONS` per test case rather
    // than drawing one index. Drawing it looks equivalent but is not: with
    // 24 variants the higher indices come up only a handful of times in a
    // default run, so a single variant dropping its pane id passed
    // unnoticed. These properties stand in for one example test per
    // variant, so every variant has to be exercised on every case.

    #[hegel::test]
    fn a_pane_id_bearing_cli_action_always_yields_exactly_one_action(tc: hegel::TestCase) {
        let (spelling, _) = tc.draw(pane_id_spelling());
        for which in 0..PANE_ID_ACTIONS {
            let with = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(spelling.clone())),
                cwd(),
                None,
            );
            let mut cases = vec![("with", with)];
            if which < PANE_ID_OPTIONAL_ACTIONS {
                cases.push((
                    "without",
                    crate::actions::actions_from_cli(
                        cli_action_with_pane_id(which, None),
                        cwd(),
                        None,
                    ),
                ));
            }
            for (label, result) in &cases {
                let actions = result.as_ref().unwrap_or_else(|e| {
                    panic!("action {} {} pane id errored: {}", which, label, e)
                });
                assert_eq!(
                    actions.len(),
                    1,
                    "action {} {} pane id {:?} produced {:?}",
                    which,
                    label,
                    spelling,
                    actions
                );
            }
        }
    }

    /// `"5"`, `"terminal_5"` both parse to `PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(5))`, so an
    /// action built from either spelling has to come out identical. This is
    /// what pins the id to `PaneId::from_str` rather than to some
    /// variant-local parse.
    #[hegel::test]
    fn equivalent_pane_id_spellings_produce_the_same_action(tc: hegel::TestCase) {
        let n = tc.draw(generators::integers::<u32>());
        for which in 0..PANE_ID_ACTIONS {
            let bare = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(format!("{}", n))),
                cwd(),
                None,
            );
            let prefixed = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(format!("terminal_{}", n))),
                cwd(),
                None,
            );
            assert_eq!(
                bare, prefixed,
                "action {} read {:?} and terminal_{} differently",
                which, n, n
            );
        }
    }

    /// The pane id has to actually reach the produced action. If a variant
    /// dropped it on the floor, two different ids would collapse to the
    /// same action - and so would the no-pane-id form.
    #[hegel::test]
    fn the_pane_id_reaches_the_action_it_produces(tc: hegel::TestCase) {
        let (first, first_id) = tc.draw(pane_id_spelling());
        let (second, second_id) = tc.draw(pane_id_spelling());
        tc.assume(first_id != second_id);

        for which in 0..PANE_ID_ACTIONS {
            let a = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(first.clone())),
                cwd(),
                None,
            );
            let b = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(second.clone())),
                cwd(),
                None,
            );

            assert_ne!(
                a, b,
                "action {} produced the same thing for pane ids {:?} and {:?}",
                which, first, second
            );
            if which < PANE_ID_OPTIONAL_ACTIONS {
                let none = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, None),
                    cwd(),
                    None,
                );
                assert_ne!(
                    a, none,
                    "action {} with pane id {:?} is indistinguishable from without one",
                    which, first
                );
            }
        }
    }

    /// Anything `PaneId::from_str` rejects must come back as an `Err` from
    /// every one of these variants - never a panic, and never a silently
    /// accepted action. The "Malformed pane id" branch is written out ~15
    /// times in `actions_from_cli`; this is what checks they all fire.
    #[hegel::test]
    fn a_malformed_pane_id_is_always_an_error(tc: hegel::TestCase) {
        let spelling = tc.draw(generators::text());
        tc.assume(PaneId::from_str(&spelling).is_err());

        for which in 0..PANE_ID_ACTIONS {
            let result = crate::actions::actions_from_cli(
                cli_action_with_pane_id(which, Some(spelling.clone())),
                cwd(),
                None,
            );
            assert!(
                result.is_err(),
                "action {} accepted malformed pane id {:?} as {:?}",
                which,
                spelling,
                result
            );
        }
    }
}
