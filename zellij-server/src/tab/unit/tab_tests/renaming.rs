use super::*;

#[test]
pub fn rename_pane_sets_current_title() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "flame");
}

#[test]
pub fn rename_pane_replaces_existing_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    let _ = tab.rename_pane_by_pane_id(pane_id, "spark".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "spark");
}

#[test]
pub fn rename_pane_to_empty_clears_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    let _ = tab.rename_pane_by_pane_id(pane_id, "".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    // Empty name should fall through to the fallback title
    assert_ne!(pane.current_title(), "flame");
}

#[test]
pub fn rename_pane_single_char() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "x".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "x");
}

#[test]
pub fn rename_pane_with_spaces() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "my pane".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "my pane");
}

#[test]
pub fn rename_pane_with_special_chars() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "pane#1 (dev)".as_bytes().to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "pane#1 (dev)");
}

#[test]
pub fn named_pane_not_overridden_by_osc_title() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // Simulate shell sending OSC 0 title
    let osc_title = b"\x1b]0;user@host: ~/code\x07";
    let _ = tab.handle_pty_bytes(crate::TerminalId::new(1), osc_title.to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "flame");
}

#[test]
pub fn unnamed_pane_shows_osc_title() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    // Send OSC 0 title without renaming the pane
    let osc_title = b"\x1b]0;user@host: ~/code\x07";
    let _ = tab.handle_pty_bytes(crate::TerminalId::new(1), osc_title.to_vec());
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "user@host: ~/code");
}

#[test]
pub fn undo_rename_clears_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let title_before = tab.get_pane_with_id(pane_id).unwrap().current_title();
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    tab.undo_rename_pane_by_pane_id(pane_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), title_before);
}

#[test]
pub fn undo_rename_on_unnamed_pane_is_noop() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let title_before = tab.get_pane_with_id(pane_id).unwrap().current_title();
    tab.undo_rename_pane_by_pane_id(pane_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), title_before);
}

#[test]
pub fn interactive_rename_appends_to_empty_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let _ = tab.update_active_pane_name(vec![b's'], client_id);
    let _ = tab.update_active_pane_name(vec![b'p'], client_id);
    let _ = tab.update_active_pane_name(vec![b'a'], client_id);
    let _ = tab.update_active_pane_name(vec![b'r'], client_id);
    let _ = tab.update_active_pane_name(vec![b'k'], client_id);
    let pane = tab
        .get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    assert_eq!(pane.current_title(), "spark");
}

#[test]
pub fn interactive_rename_appends_to_existing_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // Simulate entering rename mode
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    let _ = tab.update_active_pane_name(vec![b's'], client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "flames");
}

#[test]
pub fn interactive_rename_nul_clears_existing_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    let _ = tab.update_active_pane_name(vec![0], client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.custom_title(), None);

    let _ = tab.update_active_pane_name(b"spark".to_vec(), client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.custom_title(), Some("spark".to_owned()));
}

#[test]
pub fn interactive_rename_sanitizes_other_control_characters() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    let _ = tab.update_active_pane_name(vec![0x01, 0x07, b'\n', b'\r', 0x1b], client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.custom_title(), Some("flame".to_owned()));
}

#[test]
pub fn interactive_rename_backspace_removes_chars() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    // Backspace 3 times (DEL = 0x7F)
    let _ = tab.update_active_pane_name(vec![0x7f], client_id);
    let _ = tab.update_active_pane_name(vec![0x7f], client_id);
    let _ = tab.update_active_pane_name(vec![0x7f], client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "fl");
}

#[test]
pub fn interactive_rename_backspace_all_then_retype() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    // Backspace 5 times to clear "flame"
    for _ in 0..5 {
        let _ = tab.update_active_pane_name(vec![0x7f], client_id);
    }
    // Type "new"
    let _ = tab.update_active_pane_name(vec![b'n'], client_id);
    let _ = tab.update_active_pane_name(vec![b'e'], client_id);
    let _ = tab.update_active_pane_name(vec![b'w'], client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "new");
}

#[test]
pub fn interactive_rename_esc_reverts() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // Enter rename mode
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    // Type some chars
    let _ = tab.update_active_pane_name(vec![b'x'], client_id);
    let _ = tab.update_active_pane_name(vec![b'y'], client_id);
    // Esc — undo
    let _ = tab.undo_active_rename_pane(client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "flame");
}

#[test]
pub fn interactive_rename_esc_on_unnamed_stays_unnamed() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let title_before = tab.get_pane_with_id(pane_id).unwrap().current_title();
    // Enter rename mode on unnamed pane
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    let _ = tab.update_active_pane_name(vec![b'a'], client_id);
    let _ = tab.update_active_pane_name(vec![b'b'], client_id);
    // Esc
    let _ = tab.undo_active_rename_pane(client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), title_before);
}

#[test]
pub fn cli_rename_then_interactive_esc_restores_cli_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    // CLI rename
    let _ = tab.rename_pane_by_pane_id(pane_id, "spark".as_bytes().to_vec());
    // Enter interactive rename mode
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    let _ = tab.update_active_pane_name(vec![b'!'], client_id);
    // Esc — should restore "spark"
    let _ = tab.undo_active_rename_pane(client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "spark");
}

#[test]
pub fn cli_rename_then_undo_clears_to_fallback() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let fallback_title = tab.get_pane_with_id(pane_id).unwrap().current_title();
    let _ = tab.rename_pane_by_pane_id(pane_id, "spark".as_bytes().to_vec());
    tab.undo_rename_pane_by_pane_id(pane_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), fallback_title);
}

#[test]
pub fn cli_rename_active_pane_replaces_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    // Set initial name
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // CLI rename (focused pane) — full replacement
    let _ = tab.rename_active_pane("spark".as_bytes().to_vec(), client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "spark");
}

#[test]
pub fn cli_rename_active_pane_single_char() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // CLI rename with single character — should replace, not append
    let _ = tab.rename_active_pane("x".as_bytes().to_vec(), client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "x");
}

#[test]
pub fn cli_rename_active_pane_on_unnamed_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    // Pane has no name — CLI rename should set it
    let _ = tab.rename_active_pane("spark".as_bytes().to_vec(), client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "spark");
}

#[test]
pub fn cli_rename_active_pane_to_empty_clears_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let fallback = tab.get_pane_with_id(pane_id).unwrap().current_title();
    let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    // CLI rename to empty — should clear name
    let _ = tab.rename_active_pane("".as_bytes().to_vec(), client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), fallback);
}

#[test]
pub fn cli_rename_active_pane_then_interactive_esc_restores() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let client_id = ClientId::new(1);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    // CLI rename
    let _ = tab.rename_active_pane("spark".as_bytes().to_vec(), client_id);
    // Enter interactive rename, type something
    if let Some(pane) = tab.get_active_pane_or_floating_pane_mut(client_id) {
        pane.store_pane_name();
    }
    let _ = tab.update_active_pane_name(vec![b'!'], client_id);
    // Esc — should restore "spark"
    let _ = tab.undo_active_rename_pane(client_id);
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(pane.current_title(), "spark");
}

/// The caller skips the repaint and the session-state report when this says
/// nothing moved. That report rebuilds every tab's pane manifest and pushes a
/// `PaneUpdate`/`TabUpdate` at every subscribed plugin, so a plugin restating
/// its own title every render used to bury the user's keystrokes under its own
/// event traffic.
#[test]
pub fn renaming_a_pane_reports_whether_the_name_moved() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));

    assert!(
        tab.rename_pane("flame".as_bytes().to_vec(), pane_id)
            .unwrap(),
        "a name it did not have"
    );
    assert!(
        !tab.rename_pane("flame".as_bytes().to_vec(), pane_id)
            .unwrap(),
        "the one it already has"
    );
    assert!(
        tab.rename_pane("spark".as_bytes().to_vec(), pane_id)
            .unwrap(),
        "a different one"
    );

    // Clearing a name is a change; clearing an already-clear one is not.
    assert!(
        tab.rename_pane(Vec::new(), pane_id).unwrap(),
        "dropping the custom name"
    );
    assert!(
        !tab.rename_pane(Vec::new(), pane_id).unwrap(),
        "and it is already dropped"
    );
}
