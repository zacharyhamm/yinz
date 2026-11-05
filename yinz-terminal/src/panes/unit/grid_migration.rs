use super::*;

fn grid(content: &[u8]) -> Grid {
    let mut grid = Grid::new(
        5,
        20,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, content);
    grid
}

#[test]
fn semantic_snapshot_retains_alternate_screen_cursor_modes_and_scrollback() {
    let mut original = grid(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\x1b[?1049h\x1b[31mALTERNATE\x1b[?1h\x1b[?2004h\x1b[?1003h\x1b[3;7H\x1b7");
    let mut replacement = grid(b"unrelated destination");
    replacement
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    assert_eq!(replacement.dump_screen(true), original.dump_screen(true));
    assert!(replacement.is_alternate_mode_active());
    assert!(replacement.cursor_key_mode && replacement.bracketed_paste_mode);
    assert_eq!(replacement.mouse_tracking, original.mouse_tracking);
    assert_eq!(
        replacement.cursor_coordinates(),
        original.cursor_coordinates()
    );
    let suffix = b"\x1b8tail\x1b[?1049l\r\nafter";
    vte::Parser::new().advance(&mut original, suffix);
    vte::Parser::new().advance(&mut replacement, suffix);
    assert_eq!(replacement.dump_screen(true), original.dump_screen(true));
    assert_eq!(
        replacement.cursor_coordinates(),
        original.cursor_coordinates()
    );
}

#[test]
fn snapshot_preserves_scrolled_viewport_and_rows_below_without_ansi_reflow() {
    let mut original = grid(b"012345678901234567890123456789\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7");
    original.move_viewport_up(2);
    let before = original.migration_state().unwrap();
    let mut replacement = grid(b"");
    replacement.restore_migration_state(before.clone()).unwrap();
    assert_eq!(replacement.migration_state().unwrap(), before);
}

#[test]
fn snapshot_preserves_distinct_agent_palettes_in_history_and_the_active_launch() {
    use yinz_types::agent::theme::*;
    let mut original = grid(b"shell");
    for (launch, role) in [("old", Role::Success), ("current", Role::Error)] {
        original.apply_agent_theme_event(AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: launch.into(),
            snapshot: Some(AgentThemeSnapshot {
                version: SNAPSHOT_VERSION,
                provider: "codex".into(),
                provider_version: "fixture".into(),
                theme: launch.into(),
                provenance: vec![],
                availability: Availability::Available,
                collisions: vec![],
                definitions: vec![Definition {
                    token: "status".into(),
                    channel: Channel::Foreground,
                    color: SourceColor::Index(2),
                    pair: None,
                    target: Some(role.into()),
                }],
            }),
        });
        vte::Parser::new().advance(
            &mut original,
            format!("\r\n\x1b[32m{launch}\x1b[0m").as_bytes(),
        );
    }
    let before = original.migration_state().unwrap();
    let mut replacement = grid(b"");
    replacement.restore_migration_state(before.clone()).unwrap();
    assert_eq!(replacement.migration_state().unwrap(), before);
    vte::Parser::new().advance(&mut original, b"\r\n\x1b[32mcontinuation");
    vte::Parser::new().advance(&mut replacement, b"\r\n\x1b[32mcontinuation");
    assert_eq!(
        replacement.migration_state().unwrap(),
        original.migration_state().unwrap()
    );
    let completed = original.render(0, 0, &Style::default()).unwrap().unwrap().0;
    vte::Parser::new().advance(&mut original, b"\x1b[?2026h\x1b[2J");
    replacement
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    let restored = replacement
        .render(0, 0, &Style::default())
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(completed.len(), restored.len());
    assert!(completed.iter().any(|chunk| chunk
        .agent_theme
        .as_ref()
        .is_some_and(|palette| palette.has_snapshot())));
    for (before, after) in completed.iter().zip(&restored) {
        let snapshot = |chunk: &CharacterChunk| {
            chunk
                .agent_theme
                .as_ref()
                .and_then(|palette| palette.snapshot().map(|snapshot| snapshot.clone()))
        };
        assert_eq!(snapshot(before), snapshot(after));
        assert_eq!(before.terminal_characters, after.terminal_characters);
        for character in &before.terminal_characters {
            assert_eq!(
                before.application_styles(*character.styles),
                after.application_styles(*character.styles)
            );
        }
    }
}

#[test]
fn malformed_snapshot_does_not_mutate_destination() {
    let original = grid(b"source");
    let mut snapshot = original.migration_state().unwrap();
    snapshot["format"] = serde_json::json!(999);
    let mut replacement = grid(b"destination");
    let before = replacement.migration_state().unwrap();
    assert!(replacement.restore_migration_state(snapshot).is_err());
    assert_eq!(replacement.migration_state().unwrap(), before);
}

#[test]
fn migration_preserves_incomplete_frame_but_waits_for_host_reply() {
    let mut original = grid(b"content");
    original.lock_renders = true;
    let mut destination = grid(b"old");
    destination
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    assert!(destination.lock_renders);
    assert_eq!(destination.dump_screen(true), original.dump_screen(true));
    original.lock_renders = false;
    original.pending_messages_to_pty.push(b"reply".to_vec());
    assert!(original.validate_migration().is_err());
    assert!(original.migration_state().is_err());
}

#[test]
fn migration_repaints_the_completed_frame_until_synchronized_output_ends() {
    let mut original = grid(b"\x1b[31mcompleted frame");
    original
        .selection
        .set_start_and_end_positions(Position::new(0, 0), Position::new(0, 5));
    let completed = original.render(3, 2, &Style::default()).unwrap().unwrap().0;
    assert!(!completed[0].selection_and_colors().is_empty());
    vte::Parser::new().advance(&mut original, b"\x1b[?2026h\x1b[H\x1b[32mpartial update ");
    assert!(original.lock_renders);
    let snapshot = original.migration_state().unwrap();
    original.render_full_viewport();
    let locked = original.render(3, 2, &Style::default()).unwrap().unwrap().0;
    assert_eq!(format!("{locked:?}"), format!("{completed:?}"));
    for content in [b"".as_slice(), b"previous destination frame"] {
        let mut replacement = grid(content);
        replacement.render(0, 0, &Style::default()).unwrap();
        replacement
            .restore_migration_state(snapshot.clone())
            .unwrap();
        let restored = replacement
            .render(3, 2, &Style::default())
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(format!("{restored:?}"), format!("{locked:?}"));
        assert!(replacement
            .render(3, 2, &Style::default())
            .unwrap()
            .is_none());
        vte::Parser::new().advance(&mut replacement, b"\x1b[?2026l");
        let updated = replacement
            .render(3, 2, &Style::default())
            .unwrap()
            .unwrap()
            .0;
        let text: String = updated
            .iter()
            .flat_map(|chunk| &chunk.terminal_characters)
            .map(|cell| cell.character)
            .collect();
        assert!(text.contains("partial update"));
    }
    // The same restoration path is used to resume the original grid on rollback.
    original.restore_migration_state(snapshot).unwrap();
    let rolled_back = original.render(3, 2, &Style::default()).unwrap().unwrap().0;
    assert_eq!(format!("{rolled_back:?}"), format!("{locked:?}"));
}

#[test]
fn migration_remaps_hyperlinks_in_the_completed_frame() {
    let mut original = grid(b"\x1b]8;;https://source.example\x1b\\source\x1b]8;;\x1b\\");
    original.render(0, 0, &Style::default()).unwrap();
    vte::Parser::new().advance(&mut original, b"\x1b[?2026h\x1b[2J");
    let mut replacement = grid(b"\x1b]8;;https://existing.example\x1b\\existing\x1b]8;;\x1b\\");
    replacement
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    let frame = replacement
        .render(0, 0, &Style::default())
        .unwrap()
        .unwrap()
        .0;
    let anchor = frame[0].terminal_characters[0].styles.link_anchor;
    assert_ne!(anchor, Some(LinkAnchor::Start(0)));
    assert!(replacement
        .link_handler
        .borrow()
        .output_osc8(anchor)
        .unwrap()
        .contains("source.example"));
}

#[test]
fn migration_accepts_snapshots_without_a_completed_frame() {
    let original = grid(b"legacy snapshot");
    let mut snapshot = original.migration_state().unwrap();
    snapshot.as_object_mut().unwrap().remove("completed_text");
    let mut replacement = grid(b"");
    replacement.restore_migration_state(snapshot).unwrap();
    assert_eq!(replacement.dump_screen(true), original.dump_screen(true));
}

#[test]
fn invalid_completed_frame_does_not_mutate_destination() {
    let mut original = grid(b"source");
    original.render(0, 0, &Style::default()).unwrap();
    original.lock_renders();
    let snapshot = original.migration_state().unwrap();
    for (field, invalid) in [
        ("agent_theme", serde_json::json!(999)),
        ("changed_colors", serde_json::json!([])),
    ] {
        let mut malformed = snapshot.clone();
        malformed["completed_text"][0][0][field] = invalid;
        let mut replacement = grid(b"destination");
        let before = replacement.migration_state().unwrap();
        assert!(replacement.restore_migration_state(malformed).is_err());
        assert_eq!(replacement.migration_state().unwrap(), before);
    }
}

#[test]
fn migration_does_not_turn_temporary_command_feedback_into_a_permanent_highlight() {
    let mut original = grid(b"content");
    original.set_command_output_flash(Position::new(0, 0), Position::new(0, 3));
    assert!(original.command_output_flash_is_shown());
    let mut replacement = grid(b"");
    replacement
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    assert!(!replacement.command_output_flash_is_shown());
    assert_eq!(replacement.dump_screen(true), original.dump_screen(true));
}

#[test]
fn hyperlinks_merge_without_rebinding_existing_destination_links() {
    let source = grid(b"\x1b]8;;https://source.example\x1b\\source\x1b]8;;\x1b\\");
    let mut replacement = grid(b"\x1b]8;;https://existing.example\x1b\\existing\x1b]8;;\x1b\\");
    let links = replacement.link_handler.clone();
    replacement
        .restore_migration_state(source.migration_state().unwrap())
        .unwrap();
    assert!(links
        .borrow()
        .output_osc8(Some(LinkAnchor::Start(0)))
        .unwrap()
        .contains("existing.example"));
    let anchor = replacement.viewport[0].columns[0].styles.link_anchor;
    assert!(links
        .borrow()
        .output_osc8(anchor)
        .unwrap()
        .contains("source.example"));
}

#[test]
fn colored_scrollback_snapshot_uses_compact_runs() {
    let mut original = grid(b"\x1b[31mcolored");
    let character = original.viewport[0].columns[0].clone();
    original.lines_above.clear();
    for _ in 0..10_000 {
        let mut row = Row::new();
        row.columns = std::iter::repeat_n(character.clone(), 80).collect();
        original.lines_above.push_back(row);
    }
    original.validate_migration().unwrap();
    let snapshot = original.migration_state().unwrap();
    assert!(serde_json::to_vec(&snapshot).unwrap().len() < 8_000_000);
    for row in snapshot["lines_above"].as_array().unwrap() {
        assert_eq!(row["columns"].as_array().unwrap().len(), 1);
    }
    let mut replacement = grid(b"");
    replacement.restore_migration_state(snapshot).unwrap();
    assert_eq!(replacement.lines_above.len(), 10_000);
    assert_eq!(
        replacement.migration_state().unwrap(),
        original.migration_state().unwrap()
    );
}

#[test]
fn character_runs_preserve_widths_and_explicit_blanks() {
    let mut original = grid("\x1b[31m界e\u{301}  \x1b[32mx".as_bytes());
    original.viewport[0]
        .columns
        .push_back(EMPTY_TERMINAL_CHARACTER.clone());
    original.viewport[0]
        .columns
        .push_back(TerminalCharacter::new_singlewidth('界'));
    let mut replacement = grid(b"");
    replacement
        .restore_migration_state(original.migration_state().unwrap())
        .unwrap();
    assert!(replacement.viewport[0].columns == original.viewport[0].columns);
}

#[test]
fn transferred_alternate_screen_overflow_never_enters_normal_history() {
    let mut original =
        grid(b"normal-1\r\nnormal-2\r\nnormal-3\r\nnormal-4\r\nnormal-5\r\nnormal-6\x1b[3;4H");
    let normal = original.dump_screen(true);
    let cursor = original.cursor_coordinates();
    vte::Parser::new().advance(
        &mut original,
        b"\x1b[?1049h\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[>1u\x1b[2Jprompt>",
    );
    let state = original.migration_state().unwrap();
    let mut restored = grid(b"");
    restored.restore_migration_state(state).unwrap();
    // A normal prompt submission causes a fullscreen application to repaint
    // more rows than its viewport. These must remain in the alternate buffer.
    for round in 0..3 {
        let output = format!(
            "\x1b[H\x1b[2J{}\x1b[2;1Hprompt-{round}>",
            (0..20)
                .map(|i| format!("transcript-{i}\r\n"))
                .collect::<String>()
        );
        vte::Parser::new().advance(&mut restored, output.as_bytes());
        assert!(restored.lines_above.is_empty());
    }
    assert!(restored.bracketed_paste_mode);
    assert!(restored.supports_kitty_keyboard_protocol);
    assert_eq!(restored.mouse_tracking, original.mouse_tracking);
    vte::Parser::new().advance(&mut restored, b"\x1b[?1049l");
    assert_eq!(restored.dump_screen(true), normal);
    assert_eq!(restored.cursor_coordinates(), cursor);
}
