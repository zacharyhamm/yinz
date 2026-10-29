use super::*;
use crate::output::Output;
use std::time::Duration;
use yinz_types::internal_ids::{PluginId, SessionName};
use yinz_types::keys::{BareKey, KeyWithModifier};

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size { cols: 80, rows: 24 };

#[test]
fn palette_wheel_moves_selection_without_scrolling_terminal() {
    let mut screen = screen();
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .get_active_pane_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(b"scrollback\r\n".repeat(100));
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b" ", false, CLIENT);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    screen.handle_mouse_event(
        MouseEvent::new_scroll_down_event(Position::new(10, 30)),
        CLIENT,
    );
    let after = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_ne!(before, after);
    screen.handle_mouse_event(
        MouseEvent::new_scroll_up_event(Position::new(10, 30)),
        CLIENT,
    );
    assert_eq!(
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        before
    );
    assert!(!screen.active_pane_is_scrolled(CLIENT));
}

fn screen() -> Screen {
    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
}

#[test]
fn palette_text_spaces_cover_dividers_and_dismissal_restores_them() {
    let mut screen = screen();
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(
            PaneId::Terminal(TerminalId::new(2)),
            None,
            None,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    for opacity in [85, 0, 100, 85] {
        let mut config = test_screen_config();
        config.background_opacity = opacity;
        screen
            .reconfigure(config, Keybinds::default(), CLIENT)
            .unwrap();
        let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let cells = grid.as_character_lines();
        assert_eq!(cells[11][17].character, '─', "{before}");
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(&None, b" ", false, CLIENT);
        for step in 0..=6 {
            let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
            assert!(
                frame.contains("Archived agents…"),
                "opacity={opacity}, step={step}:\n{frame}"
            );
            if step < 5 {
                palette_key(&mut screen, BareKey::Down);
            } else {
                palette_key(&mut screen, BareKey::Up);
            }
        }
        screen
            .resize_to_screen(Size { cols: 70, rows: 24 })
            .unwrap();
        grid.change_size(24, 70);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("Archived agents…"), "{frame}");
        screen.resize_to_screen(SIZE).unwrap();
        grid.change_size(SIZE.rows, SIZE.cols);
        screen.close_command_palette(CLIENT);
        assert_eq!(
            render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            before
        );
        assert_eq!(grid.as_character_lines(), cells);
    }
}

fn floating_only_screen() -> Screen {
    let mut screen = screen();
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    screen
}

#[test]
fn palette_dismissal_restores_cells_outside_floating_panes() {
    let mut screen = floating_only_screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (cursor_before, before) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    let cells_before = grid.as_character_lines();
    screen.toggle_command_palette(CLIENT);
    let opened = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(opened.contains("Do · Project:"), "{opened}");
    let compact_cells = grid.as_character_lines();
    screen.command_palette_handles_input(&None, b"Rename tab", false, CLIENT);
    let expanded = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(expanded.contains("> Rename tab"), "{expanded}");
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()),
        &[],
        false,
        CLIENT,
    );
    assert_eq!(
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        opened
    );
    assert_eq!(grid.as_character_lines(), compact_cells);
    screen.close_command_palette(CLIENT);
    let (cursor_after, after) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(after, before);
    assert_eq!(grid.as_character_lines(), cells_before);
    assert_eq!(cursor_after, cursor_before);
}

#[test]
fn watchers_see_only_the_followed_palette_and_restore_the_floating_background() {
    let mut screen = floating_only_screen();
    let other = ClientId::new(2);
    let watcher = ClientId::new(3);
    screen.add_client(other).unwrap();
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, SIZE);
    screen.set_followed_client(CLIENT).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (cursor_before, before) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    let cells_before = grid.as_character_lines();

    screen.toggle_command_palette(other);
    screen.command_palette_handles_input(&None, b"private", false, other);
    let snapshot = render_frame_into(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(snapshot, before, "another client's palette stays private");

    screen.toggle_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(snapshot.contains("Do · Project:"), "{snapshot}");
    assert!(!snapshot.contains("private"), "{snapshot}");
    assert_eq!(cursor, Some((6, 3)));
    screen.command_palette_handles_input(&None, b"Rename tab", false, CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        snapshot.contains("> Rename tab") && snapshot.contains("Rename tab"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(cursor, Some((16, 3)));

    screen.close_command_palette(CLIENT);
    let (cursor_after, after) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(after, before);
    assert_eq!(grid.as_character_lines(), cells_before);
    assert_eq!(cursor_after, cursor_before);
}

#[test]
fn a_smaller_watcher_crops_the_palette_cursor_and_restores_it_when_back_in_bounds() {
    let mut screen = screen();
    let watcher = ClientId::new(2);
    let size = Size { cols: 20, rows: 8 };
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, size);
    screen.set_followed_client(CLIENT).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    screen.toggle_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(snapshot.contains("> "), "{snapshot}");
    assert_eq!(cursor, Some((6, 3)));
    screen.command_palette_handles_input(&None, b"a very long query", false, CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(snapshot.contains("> a very long qu"), "{snapshot}");
    assert_eq!(
        cursor, None,
        "the input cursor is outside this watcher's width"
    );
    assert!(screen.watcher_clients[&watcher].cursor_was_cropped());
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Home)),
        &[],
        false,
        CLIENT,
    );
    let (cursor, _) = render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert_eq!(cursor, Some((6, 3)));
    assert!(!screen.watcher_clients[&watcher].cursor_was_cropped());
    screen.close_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(!snapshot.contains("a very long qu"), "{snapshot}");
    assert_eq!(cursor, Some(pane_cursor(&mut screen, CLIENT)));
}

#[test]
fn palette_keeps_focus_owns_input_and_resets_on_reopen() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let pane = screen.get_active_pane_id(&CLIENT);
    let underlying_cursor = pane_cursor(&mut screen, CLIENT);
    screen.toggle_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(cursor, Some((6, 3)), "{snapshot}");
    assert!(snapshot.contains("Do · Project:") && !snapshot.contains("New pane"));
    assert_eq!(screen.get_active_pane_id(&CLIENT), pane);
    // Even an intercepting plugin must not get the palette's keys.
    assert!(!screen
        .write_character(
            Some(KeyWithModifier::new(BareKey::Char('☃'))),
            "☃".as_bytes().to_vec(),
            false,
            CLIENT,
            &HashMap::from([(CLIENT, crate::PluginId::new(5))]),
            None
        )
        .unwrap());
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        CLIENT
    ));
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("No matches"), "{snapshot}");
    assert!(!snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(screen.get_active_pane_id(&CLIENT), pane);
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        &[],
        false,
        CLIENT
    ));
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(cursor, Some(underlying_cursor), "{snapshot}");
    assert!(
        !snapshot.contains("Selected:") && !snapshot.contains("Do · Project:"),
        "{snapshot}"
    );
    screen.toggle_command_palette(CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Do · Project:") && !snapshot.contains("Selected:"),
        "{snapshot}"
    );
}

#[test]
fn palette_is_private_to_each_client_and_dismisses_on_tab_change_or_disconnect() {
    let mut screen = screen();
    let second = ClientId::new(2);
    screen.add_client(second).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, second, &mut grid);
    assert!(!snapshot.contains("Do · Project:"), "{snapshot}");
    assert_eq!(cursor, Some(pane_cursor(&mut screen, second)));
    screen.toggle_command_palette(second);
    screen.command_palette_handles_input(&None, b"Rename tab", false, CLIENT);
    screen.command_palette_handles_input(&None, b"Close pane", false, second);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Rename tab") && !snapshot.contains("Close pane"),
        "{snapshot}"
    );
    let snapshot = render_frame_into(&mut screen, &receiver, second, &mut grid);
    assert!(
        snapshot.contains("Close pane") && !snapshot.contains("Rename tab"),
        "{snapshot}"
    );
    new_tab(&mut screen, 2, TabId::new(1));
    screen.render().unwrap();
    assert!(!screen.command_palettes.contains_key(&CLIENT));
    assert!(screen.command_palettes.contains_key(&second));
    screen.remove_client(second).unwrap();
    assert!(screen.command_palettes.is_empty());
}

#[test]
fn agent_info_requests_keep_each_clients_timezone_and_clear_it_on_disconnect() {
    use yinz_runtime::{
        channels::{unbounded, SenderWithContext},
        client_time::ClientTimeZone,
    };
    let mut screen = screen();
    let second = ClientId::new(2);
    screen.add_client(second).unwrap();
    let (tx, rx) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    for (client, timezone, expected) in [
        (CLIENT, "America/Chicago", "Dec. 31st, 1969, 6pm"),
        (second, "Asia/Tokyo", "Jan. 1st, 1970, 9am"),
    ] {
        screen
            .client_timezones
            .insert(client, ClientTimeZone::from_client(Some(timezone)));
        screen.toggle_command_palette(client);
        screen.command_palette_handles_input(&None, b"codex project", false, client);
        screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Enter)),
            b"",
            false,
            client,
        );
        let request = rx
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PaletteAgent(request) => Some(request),
                _ => None,
            })
            .unwrap();
        assert_eq!(request.client, client);
        assert_eq!(request.timezone.timestamp(Some(0)), expected);
    }
    screen.remove_client(second).unwrap();
    assert!(!screen.client_timezones.contains_key(&second));
    assert_eq!(
        screen.client_timezones[&CLIENT].timestamp(Some(0)),
        "Dec. 31st, 1969, 6pm"
    );
}

#[test]
fn palette_paste_resize_and_outside_click_repaint_without_changing_focus() {
    let mut screen = screen();
    let other_pane = PaneId::Terminal(TerminalId::new(2));
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.horizontal_split(other_pane, None, None, CLIENT, None, None)
        .unwrap();
    tab.move_focus_up(CLIENT).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let pane = screen.get_active_pane_id(&CLIENT);
    screen.toggle_command_palette(CLIENT);
    for bytes in [b"\x1b[200~".as_slice(), b"New\npane", b"\x1b[201~"] {
        assert!(!screen
            .write_character(None, bytes.to_vec(), false, CLIENT, &HashMap::new(), None)
            .unwrap());
    }
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("> New pane") && !snapshot.contains("New tab"),
        "{snapshot}"
    );
    let small = Size { cols: 30, rows: 8 };
    screen.resize_to_screen(small).unwrap();
    grid.change_size(small.rows, small.cols);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("New pane"), "{snapshot}");
    assert!(cursor.is_some_and(|(x, y)| x < small.cols && y < small.rows));
    screen.resize_to_screen(SIZE).unwrap();
    grid.change_size(SIZE.rows, SIZE.cols);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(20, 60)),
        CLIENT,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(20, 60)),
        CLIENT,
    );
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(screen.get_active_pane_id(&CLIENT), pane);
    // The same click after dismissal reaches the other pane normally.
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(20, 60)),
        CLIENT,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(20, 60)),
        CLIENT,
    );
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(other_pane));
}

#[test]
fn palette_focus_click_captures_release_before_terminal_mouse_reporting() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

    let mut screen = create_non_mirrored_screen(SIZE);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    let terminal = TerminalId::new(1);
    let pane = PaneId::Terminal(terminal);
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.handle_pty_bytes(terminal, b"\x1b[?1002h\x1b[?1006h".to_vec())
        .unwrap();
    tab.set_pane_agent(
        pane,
        Some(AgentInfo {
            kind: "claude".into(),
            state: AgentState::Waiting,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: Some("permission".into()),
            wait_needs_a_person: true,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
    );
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "state:waiting");
    palette_key(&mut screen, BareKey::Enter);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    let (row, col) = grid
        .as_character_lines()
        .iter()
        .enumerate()
        .find_map(|(row, cells)| {
            let line = cells.iter().map(|cell| cell.character).collect::<String>();
            line.find("Focus")
                .map(|byte| (row, line[..byte].chars().count()))
        })
        .expect("agent info has a Focus button");
    pty_writer_rx.try_iter().for_each(drop);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row as i32, col as u16)),
        CLIENT,
    );
    assert!(!screen.command_palettes.contains_key(&CLIENT));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
    assert!(screen.command_palette_mouse_captures.contains(&CLIENT));
    let position = Position::new(5, 5);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(position), CLIENT);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(position), CLIENT);
    assert!(pty_writer_rx.try_recv().is_err());
    assert!(!screen.command_palette_mouse_captures.contains(&CLIENT));

    // Capture ends at release, so the next click reaches the mouse-reporting app.
    screen.handle_mouse_event(MouseEvent::new_left_press_event(position), CLIENT);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(position), CLIENT);
    let writes = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, id, _) => Some((bytes, id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 2);
    assert!(writes
        .iter()
        .all(|(bytes, id)| { *id == terminal && bytes.starts_with(b"\x1b[<") }));
    assert!(writes[0].0.ends_with(b"M"));
    assert!(writes[1].0.ends_with(b"m"));
}

#[test]
fn opening_palette_closes_context_menu_and_toggle_key_closes_palette() {
    let mut screen = screen();
    screen.open_context_menu(
        yinz_types::data::ContextMenuContext {
            pane_id: screen.get_active_pane_id(&CLIENT).unwrap(),
            ..Default::default()
        },
        CLIENT,
    );
    screen.toggle_command_palette(CLIENT);
    assert!(!screen.context_menu_is_open(CLIENT));
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier()),
        &[],
        true,
        CLIENT
    ));
    assert!(screen.command_palettes.is_empty());
}

#[test]
fn palette_cursor_is_opaque_on_query_idle_and_watcher_frames() {
    use yinz_terminal::panes::terminal_character::AnsiCode;

    for watching in [false, true] {
        let mut screen = screen();
        let mut config = test_screen_config();
        config.background_opacity = 50;
        config.theme.text_unselected.background =
            yinz_types::colors::PaletteColor::Rgb((90, 80, 70));
        screen
            .reconfigure(config, Keybinds::default(), CLIENT)
            .unwrap();
        let watcher = ClientId::new(2);
        screen.add_watcher_client(watcher).unwrap();
        screen.set_watcher_size(watcher, SIZE);
        screen.set_followed_client(CLIENT).unwrap();
        let client = if watching { watcher } else { CLIENT };
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let paint = (1..=SIZE.rows)
            .map(|row| format!("\x1b[{row};1H{}", "w".repeat(SIZE.cols)))
            .collect::<String>();
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(1),
                format!("\x1b[?7l\x1b[48;2;10;20;30m{paint}").into_bytes(),
            )
            .unwrap();
        screen.toggle_command_palette(CLIENT);
        let background = Some(AnsiCode::from(
            screen.style.colors.text_unselected.background,
        ));
        for (query, expected_x) in [("", 6), ("pane", 10)] {
            screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT);
            for _ in 0..2 {
                let (cursor, frame) =
                    render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
                assert_eq!(cursor, Some((expected_x, 3)), "{frame}");
                let cells = grid.as_character_lines();
                assert_eq!(cells[3][expected_x].character, ' ');
                assert_eq!(cells[3][expected_x].styles.background, background);
                assert_eq!(cells[3][expected_x + 1].character, 'w');
                assert_ne!(cells[3][expected_x + 1].styles.background, background);
            }
        }
        palette_key(&mut screen, BareKey::Left);
        let (cursor, _) = render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
        assert_eq!(cursor, Some((9, 3)));
        let cells = grid.as_character_lines();
        assert_eq!(cells[3][9].character, 'e');
        assert_eq!(cells[3][9].styles.background, background);
        assert_eq!(cells[3][10].character, 'w');
        assert_ne!(cells[3][10].styles.background, background);
    }
}

#[test]
fn palette_hides_covered_emoji_and_restores_them_on_dismissal() {
    for opacity in [0, 50, 85, 100] {
        let mut screen = screen();
        let mut config = test_screen_config();
        config.background_opacity = opacity;
        screen
            .reconfigure(config, Keybinds::default(), CLIENT)
            .unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(1),
                "\x1b[4;40Hemoji:👇end\x1b[22;40Houtside:👇end"
                    .as_bytes()
                    .to_vec(),
            )
            .unwrap();
        let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(before.contains("emoji:👇end"), "{before}");
        screen.toggle_command_palette(CLIENT);
        let expected = if opacity == 0 {
            "emoji:👇end"
        } else {
            "emoji:  end"
        };
        let open = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(open.contains(expected), "opacity={opacity}:\n{open}");
        assert!(open.contains("outside:👇end"), "{open}");
        // A query-only frame reuses the backing without new pane output.
        palette_key(&mut screen, BareKey::Left);
        let idle = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(idle.contains(expected), "{idle}");
        screen.close_command_palette(CLIENT);
        let closed = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(closed.contains("emoji:👇end"), "{closed}");
        assert!(closed.contains("outside:👇end"), "{closed}");
    }
}

#[test]
fn palette_repaints_its_background_on_frames_without_new_pane_output() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    for text in ["background one", "background two"] {
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), format!("\x1b[4;40H{text}").into_bytes())
            .unwrap();
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(snapshot.contains(text), "{snapshot}");
        let cells = grid.as_character_lines();
        // A query-only frame must tint fresh pane cells too, rather than clearing the backing text.
        screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Left)),
            &[],
            false,
            CLIENT,
        );
        let unchanged = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(snapshot, unchanged);
        assert_eq!(
            cells,
            grid.as_character_lines(),
            "backing colors must not accumulate tint"
        );
    }
    screen.close_command_palette(CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("background two") && !snapshot.contains("Do · Project:"),
        "{snapshot}"
    );
}

#[test]
fn palette_navigation_does_not_repaint_pane_contents_or_borders() {
    for query in [" ", "pane"] {
        let mut screen = screen();
        let mut config = test_screen_config();
        config.pane_frame_style = PaneFrameStyle::Full;
        screen
            .reconfigure(config, Keybinds::default(), CLIENT)
            .unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let original = grid.as_character_lines();
        for key in [BareKey::Down, BareKey::Up, BareKey::Down, BareKey::Up] {
            let before = grid.as_character_lines();
            palette_key(&mut screen, key);
            let mut output = Output::default();
            output.pane_change_client = Some(CLIENT);
            screen.prepare_command_palette_render();
            output.set_input_overlay_clients(std::iter::once(CLIENT));
            screen
                .get_active_tab_mut(CLIENT)
                .unwrap()
                .render_panes(&mut output, None)
                .unwrap();
            assert!(
                output.rendered_pane_contents.is_empty(),
                "navigation repainted pane contents"
            );
            assert!(
                output.rendered_pane_frames.is_empty(),
                "navigation repainted pane frames"
            );
            assert!(
                !output.rendered_pane_boundaries,
                "navigation repainted pane boundaries"
            );
            screen
                .render_command_palettes(&mut output, std::iter::once(CLIENT), false)
                .unwrap();
            let rendered = output.serialize().unwrap();
            take_snapshot_and_cursor_coordinates(&rendered[&CLIENT], &mut grid);
            assert_ne!(
                before,
                grid.as_character_lines(),
                "navigation changes selection"
            );
        }
        assert_eq!(original, grid.as_character_lines());
    }
}

#[test]
fn palette_backing_refreshes_terminal_colors_and_theme() {
    use yinz_types::colors::PaletteColor;
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            b"\x1b[4;40Hdefault \x1b[31mindexed".to_vec(),
        )
        .unwrap();
    screen.toggle_command_palette(CLIENT);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    for change in 0..8 {
        let before = grid.as_character_lines();
        match change {
            0 => screen.terminal_emulator_colors.borrow_mut().fg = PaletteColor::Rgb((12, 34, 56)),
            1 => screen.terminal_emulator_colors.borrow_mut().bg = PaletteColor::Rgb((65, 43, 21)),
            2 => {
                screen
                    .terminal_emulator_color_codes
                    .borrow_mut()
                    .insert(1, "rgb:00/ff/00".into());
            },
            3..=6 => {
                let osc = match change {
                    3 | 5 => "\x1b]4;1;rgb:ff/00/ff\x07",
                    4 => "\x1b]104;1\x07",
                    _ => "\x1b]104\x07",
                };
                screen
                    .get_active_tab_mut(CLIENT)
                    .unwrap()
                    .handle_pty_bytes(TerminalId::new(1), osc.as_bytes().to_vec())
                    .unwrap();
            },
            _ => {
                let mut config = test_screen_config();
                config.theme.text_unselected.background = PaletteColor::Rgb((90, 80, 70));
                screen
                    .reconfigure(config, Keybinds::default(), CLIENT)
                    .unwrap();
            },
        }
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let refreshed = grid.as_character_lines();
        assert_ne!(before[3][39..54], refreshed[3][39..54]);
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .set_force_render();
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(
            refreshed,
            grid.as_character_lines(),
            "cached colors match a full repaint"
        );
    }
}

#[test]
fn palette_backing_clears_exposed_notifications_outside_floating_panes() {
    use yinz_types::animation::Tween;
    for moved in [false, true] {
        let mut screen = floating_only_screen();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let now = std::time::Instant::now();
        screen.pinned_now = Some(now);
        screen.toggle_command_palette(CLIENT);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let before = grid.as_character_lines()[3][60].clone();
        assert_eq!(before.character, ' ');

        let mut notification = crate::screen::Notification::new(
            NotificationId::new(0),
            "a toast over the empty tab background".into(),
            None,
            None,
            now,
        );
        notification.reveal = Tween::settled(1.0);
        screen.notifications.push(notification);
        let surface = yinz_types::pane_size::Viewport {
            x: 0,
            y: 0,
            cols: SIZE.cols,
            rows: SIZE.rows,
        };
        let y = screen.notification_layout(surface, now, ClientId::new(1))[0].y;
        screen.notifications[0].y_offset = Tween::settled(3.0 - y as f32);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(grid.as_character_lines()[3][60].character, '─');

        if moved {
            screen.notifications[0].y_offset = Tween::settled(9.0 - y as f32);
        } else {
            screen.notifications.clear();
        }
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(grid.as_character_lines()[3][60], before, "moved: {moved}");
    }
}

#[test]
fn palette_backing_keeps_watcher_notifications_separate() {
    use yinz_types::animation::Tween;
    let mut screen = screen();
    let watcher = ClientId::new(2);
    screen.add_watcher_client(watcher).unwrap();
    screen.set_watcher_size(watcher, SIZE);
    screen.set_followed_client(CLIENT).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let now = std::time::Instant::now();
    screen.pinned_now = Some(now);
    let mut notification = crate::screen::Notification::new(
        NotificationId::new(0),
        "a toast visible to regular clients".into(),
        None,
        None,
        now,
    );
    notification.reveal = Tween::settled(1.0);
    screen.notifications.push(notification);
    let surface = yinz_types::pane_size::Viewport {
        x: 0,
        y: 0,
        cols: SIZE.cols,
        rows: SIZE.rows,
    };
    let y = screen.notification_layout(surface, now, ClientId::new(1))[0].y;
    screen.notifications[0].y_offset = Tween::settled(3.0 - y as f32);
    screen.toggle_command_palette(CLIENT);
    let mut grids = [
        grid_for_render_snapshot(SIZE),
        grid_for_render_snapshot(SIZE),
    ];
    let mut initial = None;
    for _ in 0..3 {
        palette_key(&mut screen, BareKey::Left);
        screen.render_to_clients().unwrap();
        for (instruction, _) in receiver.try_iter() {
            if let ServerInstruction::Render(Some(rendered)) = instruction {
                for (client, grid) in [CLIENT, watcher].into_iter().zip(&mut grids) {
                    if let Some(bytes) = rendered.get(&client) {
                        take_snapshot_and_cursor_coordinates(bytes, grid);
                    }
                }
            }
        }
        let cells = grids
            .each_ref()
            .map(|grid| grid.as_character_lines()[3][60].clone());
        assert_ne!(
            cells[0], cells[1],
            "only the regular client's backing contains the toast"
        );
        assert_eq!(*initial.get_or_insert(cells.clone()), cells);
    }
}

#[test]
#[ignore = "manual navigation timing and output-volume measurement"]
fn palette_navigation_measurement() {
    for size in [
        SIZE,
        Size {
            cols: 240,
            rows: 80,
        },
    ] {
        for query in ["", "pane"] {
            let mut screen = create_non_mirrored_screen(size);
            new_tab(&mut screen, 1, TabId::new(0));
            let receiver = screen_wired_to_server(&mut screen);
            let paint = (1..size.rows)
                .map(|row| format!("\x1b[{row};1H{}", "background ".repeat(size.cols / 11)))
                .collect::<String>();
            screen
                .get_active_tab_mut(CLIENT)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(1), paint.into_bytes())
                .unwrap();
            screen.toggle_command_palette(CLIENT);
            screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT);
            screen.render_to_clients().unwrap();
            receiver.try_iter().for_each(drop);
            let started = std::time::Instant::now();
            let mut bytes = 0;
            // Direct screen calls, with no background runner: exactly one frame per key.
            for index in 0..200 {
                palette_key(
                    &mut screen,
                    if index % 2 == 0 {
                        BareKey::Down
                    } else {
                        BareKey::Up
                    },
                );
                screen.render_to_clients().unwrap();
                for (instruction, _) in receiver.try_iter() {
                    if let ServerInstruction::Render(Some(rendered)) = instruction {
                        bytes += rendered.get(&CLIENT).map_or(0, String::len);
                    }
                }
            }
            eprintln!(
                "palette {}x{} query={query:?}: {:?}/move, {} bytes/move",
                size.cols,
                size.rows,
                started.elapsed() / 200,
                bytes / 200
            );
        }
    }
}

fn palette_key(screen: &mut Screen, key: BareKey) {
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(key)),
        &[],
        false,
        CLIENT
    ));
}

fn palette_search(screen: &mut Screen, query: &str) {
    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT));
}

fn assert_palette_query(snapshot: &str, expected: &str) {
    let query = snapshot.lines().find_map(|line| {
        let (_, query) = line.split_once("> ")?;
        let (query, _) = query.split_once('│')?;
        Some(query.trim_end())
    });
    assert_eq!(query, Some(expected), "{snapshot}");
}

fn palette_actions_receiver(screen: &mut Screen) -> Receiver<(ScreenInstruction, ErrorContext)> {
    let (sender, receiver) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(sender));
    receiver
}

fn enqueued_palette_action(receiver: &Receiver<(ScreenInstruction, ErrorContext)>) -> Action {
    let (instruction, _) = receiver.try_recv().unwrap();
    match instruction {
        ScreenInstruction::ActivateCommandPaletteAction {
            pane_id,
            tab_id,
            action,
            client_id,
            ..
        } => {
            assert_eq!(pane_id, PaneId::Terminal(TerminalId::new(1)));
            assert_eq!(tab_id, TabId::new(0));
            assert_eq!(client_id, CLIENT);
            action
        },
        other => panic!("unexpected instruction: {other:?}"),
    }
}

#[test]
fn palette_builtin_actions_capture_origin_and_close_before_dispatch() {
    for (query, expected) in [
        (
            "Fullscreen",
            Action::ToggleFocusFullscreenByPaneId {
                pane_id: PaneId::Terminal(TerminalId::new(1)),
            },
        ),
        (
            "Float/embed",
            Action::TogglePaneEmbedOrFloatingByPaneId {
                pane_id: PaneId::Terminal(TerminalId::new(1)),
            },
        ),
        (
            "Show/hide",
            Action::ToggleFloatingPanesByTabId { id: TabId::new(0) },
        ),
        ("Move pane to new tab", Action::BreakPane),
        ("Move pane to previous tab", Action::BreakPaneLeft),
        ("Move pane to next tab", Action::BreakPaneRight),
        (
            "Move tab left",
            Action::MoveTabByTabId {
                id: TabId::new(0),
                direction: Direction::Left,
            },
        ),
        (
            "Move tab right",
            Action::MoveTabByTabId {
                id: TabId::new(0),
                direction: Direction::Right,
            },
        ),
        ("Detach", Action::Detach),
    ] {
        let mut screen = screen();
        let receiver = palette_actions_receiver(&mut screen);
        palette_search(&mut screen, query);
        palette_key(&mut screen, BareKey::Enter);
        assert!(screen.command_palettes.is_empty());
        assert_eq!(enqueued_palette_action(&receiver), expected, "{query}");
    }
}

#[test]
fn palette_rejects_directional_splits_without_losing_input() {
    for target in ["floating", "overlay", "tiled-with-floating-visible"] {
        for (query, placement_steps) in [("Split right", 1), ("Split down", 2)] {
            for form in [false, true] {
                let mut screen = screen();
                let other = PaneId::Terminal(TerminalId::new(2));
                if target == "overlay" {
                    screen
                        .get_active_tab_mut(CLIENT)
                        .unwrap()
                        .new_overlay_pane(
                            other,
                            None,
                            None,
                            false,
                            true,
                            OverlayPlacement::dismissible(None),
                            None,
                            None,
                        )
                        .unwrap();
                    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(other));
                } else if target == "floating" {
                    screen = floating_only_screen();
                }
                let receiver = palette_actions_receiver(&mut screen);
                let renders = screen_wired_to_server(&mut screen);
                palette_search(&mut screen, if form { "New pane…" } else { query });
                if form {
                    palette_key(&mut screen, BareKey::Enter);
                    screen.command_palette_handles_input(&None, b"keep-name", false, CLIENT);
                    palette_key(&mut screen, BareKey::Enter);
                    screen.command_palette_handles_input(&None, b"/tmp", false, CLIENT);
                    palette_key(&mut screen, BareKey::Enter);
                    for _ in 0..placement_steps {
                        palette_key(&mut screen, BareKey::Right);
                    }
                    palette_key(&mut screen, BareKey::Enter);
                }
                if target == "tiled-with-floating-visible" {
                    screen
                        .get_active_tab_mut(CLIENT)
                        .unwrap()
                        .new_floating_pane(other, None, None, false, true, None, None)
                        .unwrap();
                }
                palette_key(&mut screen, BareKey::Enter);
                assert!(screen.command_palettes.contains_key(&CLIENT));
                assert!(matches!(
                    receiver.try_recv().unwrap().0,
                    ScreenInstruction::Notify(..)
                ));
                let mut grid = grid_for_render_snapshot(SIZE);
                let rendered = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
                // The footer clips the full error to the palette width.
                assert!(
                    rendered.contains("splits require a tiled pane"),
                    "{rendered}"
                );
                if form {
                    for value in [
                        "keep-name",
                        "/tmp",
                        if placement_steps == 1 {
                            "Right"
                        } else {
                            "Down"
                        },
                    ] {
                        assert!(rendered.contains(value), "{rendered}");
                    }
                } else {
                    assert!(rendered.contains(&format!("> {query}")), "{rendered}");
                }
            }
        }
    }
}

#[test]
fn palette_rechecks_directional_splits_before_dispatch() {
    for query in ["Split right", "Split down"] {
        for change in ["floating-visible", "target-floating", "target-moved"] {
            let mut screen = screen();
            let receiver = palette_actions_receiver(&mut screen);
            palette_search(&mut screen, query);
            palette_key(&mut screen, BareKey::Enter);
            assert!(screen.command_palettes.is_empty());
            let action = enqueued_palette_action(&receiver);
            let origin = PaneId::Terminal(TerminalId::new(1));
            let other = PaneId::Terminal(TerminalId::new(2));
            let tab = screen.get_active_tab_mut(CLIENT).unwrap();
            if change == "floating-visible" {
                tab.new_floating_pane(other, None, None, false, true, None, None)
                    .unwrap();
            } else {
                tab.horizontal_split(other, None, None, CLIENT, None, None)
                    .unwrap();
                if change == "target-floating" {
                    tab.toggle_pane_embed_or_floating_for_pane_id(origin, None)
                        .unwrap();
                    tab.hide_floating_panes();
                } else {
                    new_tab(&mut screen, 3, TabId::new(1));
                    screen
                        .move_command_palette_pane(
                            origin,
                            TabId::new(0),
                            Some(Direction::Right),
                            CLIENT,
                        )
                        .unwrap();
                }
            }
            if change == "target-moved" {
                assert!(
                    matches!(receiver.try_recv().unwrap().0, ScreenInstruction::Notify(message, ..) if message == "Completed: Move pane to next tab")
                );
            }
            let (sender, pty_receiver) = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
            screen.dispatch_command_palette_action(
                origin,
                TabId::new(0),
                action,
                CLIENT,
                "Test action".into(),
            );
            assert!(matches!(
                receiver.try_recv().unwrap().0,
                ScreenInstruction::Notify(message, None, None, None)
                    if message == "Directional splits require a tiled pane with floating panes hidden."
            ));
            assert!(pty_receiver.try_recv().is_err());
        }
    }
}

#[test]
fn palette_dispatches_supported_pane_placements() {
    for floating_visible in [false, true] {
        for (query, placement_steps, direction, floating) in [
            ("Split right", None, Some(Direction::Right), false),
            ("Split down", None, Some(Direction::Down), false),
            ("New pane…", Some(1), Some(Direction::Right), false),
            ("New pane…", Some(2), Some(Direction::Down), false),
            ("New pane…", Some(0), None, false),
            ("New pane…", Some(3), None, true),
            ("New floating pane", None, None, true),
            ("New pane", None, None, floating_visible),
        ] {
            if floating_visible && direction.is_some() {
                continue;
            }
            let expected = if floating {
                NewPanePlacement::Floating(None)
            } else {
                NewPanePlacement::Tiled {
                    direction,
                    borderless: None,
                }
            };
            let mut screen = if floating_visible {
                floating_only_screen()
            } else {
                screen()
            };
            let origin = screen.get_active_pane_id(&CLIENT).unwrap();
            let receiver = palette_actions_receiver(&mut screen);
            palette_search(&mut screen, query);
            if let Some(placement_steps) = placement_steps {
                for _ in 0..3 {
                    palette_key(&mut screen, BareKey::Enter);
                }
                for _ in 0..placement_steps {
                    palette_key(&mut screen, BareKey::Right);
                }
                palette_key(&mut screen, BareKey::Enter);
            }
            palette_key(&mut screen, BareKey::Enter);
            assert!(screen.command_palettes.is_empty());
            let ScreenInstruction::ActivateCommandPaletteAction {
                pane_id,
                tab_id,
                action,
                client_id,
                ..
            } = receiver.try_recv().unwrap().0
            else {
                panic!("expected palette activation");
            };
            assert_eq!(pane_id, origin);
            let (sender, pty_receiver) = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
            screen.dispatch_command_palette_action(
                pane_id,
                tab_id,
                action,
                client_id,
                "Test action".into(),
            );
            let instruction = pty_receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .0;
            match instruction {
                PtyInstruction::SpawnTerminal(_, _, placement, _, target, completion, _) => {
                    assert_eq!(placement, expected);
                    assert_eq!(target, ClientTabIndexOrPaneId::PaneId(origin));
                    drop(completion);
                },
                other => panic!("unexpected instruction: {other:?}"),
            }
        }
    }
}

#[test]
fn palette_forms_capture_creation_values() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "New pane…");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"named", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"/tmp", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Right);
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_recv().is_err(), "fields cannot submit");
    palette_key(&mut screen, BareKey::Enter);
    match enqueued_palette_action(&receiver) {
        Action::NewTiledPane {
            direction,
            command,
            pane_name,
            near_current_pane,
            ..
        } => {
            assert_eq!(direction, Some(Direction::Right));
            assert_eq!(pane_name.as_deref(), Some("named"));
            assert_eq!(
                command.unwrap().cwd.as_deref(),
                Some(std::path::Path::new("/tmp"))
            );
            assert!(near_current_pane);
        },
        other => panic!("unexpected creation action: {other:?}"),
    }
}

#[test]
fn palette_close_confirmation_defaults_to_cancel_and_keeps_original_pane() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Close pane…");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_recv().is_err(), "Enter defaults to cancel");
    palette_key(&mut screen, BareKey::Enter);
    let other = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    palette_key(&mut screen, BareKey::Down);
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        enqueued_palette_action(&receiver),
        Action::CloseFocusByPaneId {
            pane_id: PaneId::Terminal(TerminalId::new(1))
        }
    );
}

#[test]
fn palette_never_retargets_a_vanished_origin_or_destination() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Close pane…");
    palette_key(&mut screen, BareKey::Enter);
    let other = PaneId::Terminal(TerminalId::new(2));
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    palette_key(&mut screen, BareKey::Down);
    palette_key(&mut screen, BareKey::Enter);
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        ScreenInstruction::Notify(..)
    ));
    assert!(screen.command_palettes.contains_key(&CLIENT));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(other));
    screen.dispatch_command_palette_action(
        PaneId::Terminal(TerminalId::new(1)),
        TabId::new(0),
        Action::BreakPane,
        CLIENT,
        "Test action".into(),
    );
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        ScreenInstruction::Notify(..)
    ));
}

#[test]
fn palette_dispatch_does_not_wait_for_the_screen_thread() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    screen.dispatch_command_palette_action(
        PaneId::Terminal(TerminalId::new(1)),
        TabId::new(0),
        Action::RenameTabById {
            id: TabId::new(0),
            name: "renamed".into(),
        },
        CLIENT,
        "Test action".into(),
    );
    // Receiving this instruction proves dispatch returned while its worker awaits completion.
    let (instruction, _) = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        matches!(instruction, ScreenInstruction::RenameTabWithId(..)),
        "{instruction:?}"
    );
}

#[test]
fn palette_float_and_embed_reveal_and_focus_the_captured_pane() {
    let mut screen = screen();
    let origin = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    let receiver = palette_actions_receiver(&mut screen);
    for floating in [true, false] {
        screen
            .focus_pane_with_id(origin, false, false, CLIENT)
            .unwrap();
        palette_search(&mut screen, "Float/embed");
        palette_key(&mut screen, BareKey::Enter);
        let action = enqueued_palette_action(&receiver);
        // Focus changes after selection must not retarget the captured action.
        screen
            .focus_pane_with_id(other, false, false, CLIENT)
            .unwrap();
        assert!(!screen.tabs[&TabId::new(0)].are_floating_panes_visible());
        screen.dispatch_command_palette_action(
            origin,
            TabId::new(0),
            action,
            CLIENT,
            "Test action".into(),
        );
        let instruction = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .0;
        match instruction {
            ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(pane_id, completion) => {
                assert_eq!(pane_id, origin);
                screen
                    .tab_containing_pane_mut(pane_id)
                    .unwrap()
                    .toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                    .unwrap();
                // Dropping completion releases the real overlay-action worker.
                drop(completion);
            },
            other => panic!("unexpected instruction: {other:?}"),
        }
        let instruction = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .0;
        match instruction {
            ScreenInstruction::FocusPaneWithId(pane_id, float, in_place, client, completion) => {
                assert_eq!((pane_id, client), (origin, CLIENT));
                screen
                    .focus_pane_with_id(pane_id, float, in_place, client)
                    .unwrap();
                drop(completion);
            },
            other => panic!("unexpected instruction: {other:?}"),
        }
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap().0,
            ScreenInstruction::Notify(message, ..) if message == "Completed: Test action")
        );
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(origin));
        assert_eq!(
            screen.tabs[&TabId::new(0)].are_floating_panes_visible(),
            floating
        );
    }
}

#[test]
fn palette_moves_reveal_and_focus_tiled_and_floating_panes_in_both_directions() {
    for direction in [Direction::Left, Direction::Right] {
        for floating in [false, true] {
            let mut screen = screen();
            let origin = PaneId::Terminal(TerminalId::new(1));
            let other = PaneId::Terminal(TerminalId::new(2));
            let tab = screen.get_active_tab_mut(CLIENT).unwrap();
            tab.horizontal_split(other, None, None, CLIENT, None, None)
                .unwrap();
            if floating {
                tab.toggle_pane_embed_or_floating_for_pane_id(origin, None)
                    .unwrap();
            }
            new_tab(&mut screen, 3, TabId::new(1));
            new_tab(&mut screen, 4, TabId::new(2));
            let destination = if direction == Direction::Left {
                TabId::new(2)
            } else {
                TabId::new(1)
            };
            let previous = PaneId::Terminal(TerminalId::new(if floating {
                5
            } else if direction == Direction::Left {
                4
            } else {
                3
            }));
            if floating {
                // Focus is remembered separately for tiled and floating panes.
                let tab = screen.tabs.get_mut(&destination).unwrap();
                tab.new_floating_pane(previous, None, None, false, true, None, None)
                    .unwrap();
                tab.hide_floating_panes();
            }
            let (sender, receiver) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
            screen.cli_event_subscribers_present = true;
            screen
                .move_command_palette_pane(origin, TabId::new(0), Some(direction), CLIENT)
                .unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT], destination);
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(origin));
            assert_eq!(
                screen.tabs[&destination].are_floating_panes_visible(),
                floating
            );
            let published = receiver
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(events) => Some(events),
                    _ => None,
                })
                .flatten()
                .filter_map(|(_, _, event)| match event {
                    Event::PaneUpdate(manifest) => Some(manifest),
                    _ => None,
                })
                .last()
                .expect("move publishes pane state");
            let cached = &screen.peer_sessions_cache[&screen.session_name].panes;
            for manifest in [&published, cached] {
                let panes = &manifest.panes[&screen.tabs[&destination].position];
                assert!(
                    panes
                        .iter()
                        .find(|pane| pane.id == origin)
                        .unwrap()
                        .is_focused
                );
                assert!(
                    !panes
                        .iter()
                        .find(|pane| pane.id == previous)
                        .unwrap()
                        .is_focused
                );
            }
        }
    }
}

#[test]
fn palette_moves_sync_scroll_mode_to_the_moved_pane() {
    for moved_is_scrolled in [false, true] {
        let mut screen = screen();
        let origin = PaneId::Terminal(TerminalId::new(1));
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .horizontal_split(
                PaneId::Terminal(TerminalId::new(2)),
                None,
                None,
                CLIENT,
                None,
                None,
            )
            .unwrap();
        new_tab(&mut screen, 3, TabId::new(1));
        let scrolled = if moved_is_scrolled {
            origin
        } else {
            PaneId::Terminal(TerminalId::new(3))
        };
        let pane = screen
            .tab_containing_pane_mut(scrolled)
            .unwrap()
            .get_pane_with_id_mut(scrolled)
            .unwrap();
        pane.handle_pty_bytes(b"scrollback\r\n".repeat(100));
        pane.scroll_up(5, Some(CLIENT));
        let (before, after) = if moved_is_scrolled {
            (InputMode::Normal, InputMode::Scroll)
        } else {
            (InputMode::Scroll, InputMode::Normal)
        };
        screen.change_mode(before, None, CLIENT).unwrap();
        let receiver = screen_wired_to_server(&mut screen);
        screen
            .move_command_palette_pane(origin, TabId::new(0), Some(Direction::Right), CLIENT)
            .unwrap();
        assert_eq!(screen.active_pane_is_scrolled(CLIENT), moved_is_scrolled);
        // The server owns the input mode used for keybindings; check the final request to it.
        let mode = receiver
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::ChangeMode(client, mode, _) if client == CLIENT => Some(mode),
                _ => None,
            })
            .last();
        assert_eq!(mode, Some(after));
    }
}

#[test]
fn palette_refused_move_keeps_destination_focus() {
    let mut screen = screen();
    let origin = PaneId::Terminal(TerminalId::new(1));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(
            PaneId::Terminal(TerminalId::new(2)),
            None,
            None,
            CLIENT,
            None,
            None,
        )
        .unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .resize_whole_tab(Size { cols: 4, rows: 2 })
        .unwrap();
    screen
        .move_command_palette_pane(origin, TabId::new(0), Some(Direction::Right), CLIENT)
        .unwrap();
    assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&origin));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    assert_eq!(
        screen.get_active_pane_id(&CLIENT),
        Some(PaneId::Terminal(TerminalId::new(3)))
    );
}

#[test]
fn palette_moves_only_the_captured_pane_and_never_a_replacement() {
    let mut screen = screen();
    let origin = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    screen
        .move_command_palette_pane(origin, TabId::new(0), Some(Direction::Right), CLIENT)
        .unwrap();
    assert!(screen.tabs[&TabId::new(1)].has_pane_with_pid(&origin));
    assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&other));
    // The same stale origin-tab pair cannot move the other pane that remains there.
    assert!(screen
        .move_command_palette_pane(origin, TabId::new(0), None, CLIENT)
        .is_err());
    assert_eq!(screen.tabs.len(), 2);
    assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&other));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(origin));
}

#[test]
fn palette_releases_its_input_lock_on_every_dismissal_path() {
    for dismissal in ["toggle", "escape", "click", "action", "tab", "disconnect"] {
        let mut screen = screen();
        let receiver = screen_wired_to_server(&mut screen);
        screen.toggle_command_palette(CLIENT);
        match dismissal {
            "toggle" => screen.toggle_command_palette(CLIENT),
            "escape" => {
                screen.command_palette_handles_input(
                    &Some(KeyWithModifier::new(BareKey::Esc)),
                    &[],
                    false,
                    CLIENT,
                );
            },
            "click" => {
                screen.command_palette_handles_mouse(
                    &MouseEvent::new_left_press_event(Position::new(0, 0)),
                    CLIENT,
                );
            },
            "action" => {
                screen.command_palette_handles_input(&None, b"Fullscreen", false, CLIENT);
                screen.command_palette_handles_input(
                    &Some(KeyWithModifier::new(BareKey::Enter)),
                    &[],
                    false,
                    CLIENT,
                );
            },
            "tab" => {
                new_tab(&mut screen, 2, TabId::new(1));
                screen.render().unwrap();
            },
            "disconnect" => screen.remove_client(CLIENT).unwrap(),
            _ => unreachable!(),
        }
        assert!(screen.command_palettes.is_empty(), "{dismissal}");
        let lifecycle: Vec<_> = receiver
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::SetCommandPaletteOpen(client, open) => Some((client, open)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lifecycle,
            vec![(CLIENT, true), (CLIENT, false)],
            "{dismissal}"
        );
        assert!(!screen.close_command_palette(CLIENT));
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn palette_keeps_screen_mode_locked_only_for_its_client() {
    let mut screen = screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.toggle_command_palette(CLIENT);
    for mode in [InputMode::Normal, InputMode::Pane, InputMode::Scroll] {
        screen.change_mode(mode, None, CLIENT).unwrap();
        assert_eq!(screen.mode_info[&CLIENT].mode, InputMode::Locked);
        screen.change_mode_for_all_clients(mode, None).unwrap();
        assert_eq!(screen.mode_info[&CLIENT].mode, InputMode::Locked);
        assert_eq!(screen.mode_info[&other].mode, mode);
    }
    screen.close_command_palette(CLIENT);
    screen.change_mode(InputMode::Normal, None, CLIENT).unwrap();
    assert_eq!(screen.mode_info[&CLIENT].mode, InputMode::Normal);
}

#[test]
fn palette_default_shortcuts_prefer_regular_modes() {
    let mut screen = screen();
    screen.default_mode_info.keybinds = Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    for (query, hint) in [
        ("Fullscreen", "Ctrl p → f"),
        ("New pane", "Alt n"),
        ("New tab", "Ctrl t → n"),
        ("Split right", "Ctrl p → r"),
        ("Split down", "Ctrl p → d"),
    ] {
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT);
        let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(rendered.contains(hint), "{query}: {rendered}");
        assert!(!rendered.contains("Ctrl g →"), "{query}: {rendered}");
        screen.close_command_palette(CLIENT);
    }
}

#[test]
fn palette_shows_client_shortcuts_and_refreshes_reconfigured_bindings() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    screen.change_mode(InputMode::Locked, None, CLIENT).unwrap();
    let mode_info = screen.mode_info.get_mut(&CLIENT).unwrap();
    mode_info.keybinds = vec![
        (
            InputMode::Normal,
            vec![(
                KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier(),
                vec![Action::SwitchToMode {
                    input_mode: InputMode::Pane,
                }],
            )],
        ),
        (
            InputMode::Pane,
            vec![(
                KeyWithModifier::new(BareKey::Char('f')),
                vec![
                    Action::ToggleFocusFullscreen,
                    Action::SwitchToMode {
                        input_mode: InputMode::Locked,
                    },
                ],
            )],
        ),
    ];
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"Fullscreen", false, CLIENT);
    let mut grid = grid_for_render_snapshot(SIZE);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("Ctrl g → f"), "{rendered}");
    screen.mode_info.get_mut(&CLIENT).unwrap().keybinds = vec![(
        InputMode::Normal,
        vec![(
            KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier(),
            vec![Action::ToggleFocusFullscreen],
        )],
    )];
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("Alt z"), "{rendered}");
    assert!(!rendered.contains("Ctrl g → f"), "{rendered}");
    let (background_sender, background_receiver) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(background_sender));
    screen
        .reconfigure(test_screen_config(), Keybinds::default(), CLIENT)
        .unwrap();
    assert!(background_receiver
        .try_iter()
        .any(|(job, _)| matches!(job, BackgroundJob::RenderToClients)));
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("> Fullscreen"), "{rendered}");
    assert!(!rendered.contains("Alt z"), "{rendered}");
}

#[test]
fn palette_skips_shortcut_searches_for_entries_without_actions() {
    let mut screen = screen();
    screen.default_mode_info.keybinds = Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    let lookups = screen.command_palettes[&CLIENT].shortcut_lookups;

    screen
        .themes
        .insert("test-theme".into(), screen.style.colors);
    screen.projects.insert("test-project".into());
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_lookups, lookups);

    screen.command_palette_handles_input(&None, b"Change theme", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("test-theme"), "{rendered}");
    screen.close_command_palette(CLIENT);
    screen.toggle_command_palette(CLIENT);
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_lookups, lookups);
}

#[test]
fn palette_shortcut_caches_use_independent_client_bindings() {
    let mut screen = screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    for (client, letter) in [(CLIENT, 'x'), (other, 'y')] {
        screen.change_mode(InputMode::Locked, None, client).unwrap();
        screen.mode_info.get_mut(&client).unwrap().keybinds = vec![(
            InputMode::Normal,
            vec![(
                KeyWithModifier::new(BareKey::Char(letter)).with_alt_modifier(),
                vec![Action::ToggleFocusFullscreen],
            )],
        )];
        screen.toggle_command_palette(client);
        screen.command_palette_handles_input(&None, b"Fullscreen", false, client);
    }
    for (client, hint) in [(CLIENT, "Alt x"), (other, "Alt y")] {
        let rendered = render_frame_into(&mut screen, &receiver, client, &mut grid);
        assert!(rendered.contains(hint), "{rendered}");
    }
    let other_lookups = screen.command_palettes[&other].shortcut_lookups;
    screen.mode_info.get_mut(&CLIENT).unwrap().keybinds.clear();
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!rendered.contains("Alt x"), "{rendered}");
    let rendered = render_frame_into(&mut screen, &receiver, other, &mut grid);
    assert!(rendered.contains("Alt y"), "{rendered}");
    assert_eq!(
        screen.command_palettes[&other].shortcut_lookups,
        other_lookups
    );
}

#[test]
fn palette_new_pane_keeps_default_shortcut_when_floating_panes_are_visible() {
    let mut screen = screen();
    screen.default_mode_info.keybinds = Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"New pane", false, CLIENT);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("Alt n"), "{rendered}");
    assert!(!rendered.contains("Ctrl g →"), "{rendered}");
    let lookups = screen.command_palettes[&CLIENT].shortcut_lookups;
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .new_floating_pane(
            PaneId::Terminal(TerminalId::new(2)),
            None,
            None,
            false,
            true,
            None,
            None,
        )
        .unwrap();
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("Alt n"), "{rendered}");
    assert!(!rendered.contains("Ctrl g →"), "{rendered}");
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        lookups + 2,
        "new-pane original and floating target changed"
    );
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .hide_floating_panes();
    screen.close_command_palette(CLIENT);
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"New floating pane", false, CLIENT);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!rendered.contains("Alt n"), "{rendered}");
}

#[test]
fn palette_session_scan_refreshes_current_age_without_replacing_local_metadata() {
    use std::time::Duration;
    let mut screen = screen();
    let name = screen.session_name.clone();
    let current = screen.peer_sessions_cache.get_mut(&name).unwrap();
    current.creation_time = Duration::from_secs(10);
    current.connected_clients = 7;
    let mut scanned = current.clone();
    scanned.creation_time = Duration::from_secs(70);
    scanned.connected_clients = 99;
    screen
        .update_session_infos(
            [(name.clone(), scanned)].into_iter().collect(),
            Default::default(),
        )
        .unwrap();
    let current = &screen.peer_sessions_cache[&name];
    assert_eq!(current.creation_time, Duration::from_secs(70));
    assert_eq!(current.connected_clients, 7);
}

#[test]
fn project_context_tracks_focus_membership_and_client_lifetime() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    screen.projects = ["alpha", "beta", "gamma"].map(String::from).into();
    screen.pane_projects = [(pane, "beta".into()), (other, "gamma".into())].into();
    screen
        .focus_pane_with_id(pane, false, false, CLIENT)
        .unwrap();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    palette_search(&mut screen, "Switch current project");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"alpha", false, CLIENT);
    palette_key(&mut screen, BareKey::Esc);
    assert_palette_query(
        &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        "Switch current project",
    );
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"alpha", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    assert_palette_query(
        &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        "",
    );
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(pane));
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    let second = ClientId::new(2);
    screen.add_client(second).unwrap();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&second].current.as_deref(),
        Some("beta")
    );
    screen.close_command_palette(CLIENT);
    screen
        .focus_pane_with_id(other, false, false, CLIENT)
        .unwrap();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("gamma")
    );
    screen.pane_projects.insert(other, "beta".into());
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    screen.pane_projects.clear();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    screen.projects.remove("beta");
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    screen.projects.clear();
    screen.reconcile_project_contexts();
    assert_eq!(screen.client_projects[&CLIENT].current, None);
    screen.remove_client(second).unwrap();
    assert!(!screen.client_projects.contains_key(&second));
}

#[test]
fn vanished_project_selection_stays_open_with_an_error() {
    let mut screen = screen();
    screen.projects.insert("gone".into());
    palette_search(&mut screen, "Switch current project");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"gone", false, CLIENT);
    screen.projects.clear();
    palette_key(&mut screen, BareKey::Enter);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_palette_query(&rendered, "gone");
    assert!(rendered.contains("Project no longer exists"), "{rendered}");
    palette_key(&mut screen, BareKey::Esc);
    assert_palette_query(
        &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        "Switch current project",
    );
}

#[test]
fn remembered_project_survives_empty_startup_and_explicit_switching() {
    use yinz_runtime::last_project::LastProject;
    let dir = tempfile::tempdir().unwrap();
    LastProject::load(dir.path().into()).remember("beta");
    let mut first = screen();
    first.last_project = LastProject::load(dir.path().into());
    first.reconcile_project_contexts();
    assert_eq!(first.client_projects[&CLIENT].current, None);
    assert_eq!(LastProject::load(dir.path().into()).name().unwrap(), "beta");
    first.projects = ["alpha", "beta"].map(String::from).into();
    first.reconcile_project_contexts();
    assert_eq!(
        first.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    palette_search(&mut first, "Switch current project");
    palette_key(&mut first, BareKey::Enter);
    palette_key(&mut first, BareKey::Enter);
    assert_eq!(first.last_project.name().unwrap(), "alpha");
    let projects = first.projects.clone();
    drop(first);

    let mut restarted = screen();
    restarted.last_project = LastProject::load(dir.path().into());
    restarted.projects = projects.clone();
    restarted.reconcile_project_contexts();
    assert_eq!(
        restarted.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    LastProject::load(dir.path().into()).remember("beta");
    palette_search(&mut restarted, "Switch current project");
    palette_key(&mut restarted, BareKey::Enter);
    palette_key(&mut restarted, BareKey::Enter);
    assert_eq!(restarted.last_project.name().unwrap(), "alpha");
    drop(restarted);
    assert_eq!(
        LastProject::load(dir.path().into()).name().unwrap(),
        "alpha"
    );

    let mut known_folder = screen();
    known_folder.last_project = LastProject::load(dir.path().into());
    known_folder.projects = projects;
    known_folder
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "beta".into());
    known_folder.reconcile_project_contexts();
    assert_eq!(
        known_folder.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    drop(known_folder);
    assert_eq!(LastProject::load(dir.path().into()).name().unwrap(), "beta");
}

#[test]
fn remembered_project_yields_to_focus_and_late_discovery_without_repeated_writes() {
    use yinz_runtime::last_project::LastProject;
    let dir = tempfile::tempdir().unwrap();
    LastProject::load(dir.path().into()).remember("beta");
    let mut screen = screen();
    screen.last_project = LastProject::load(dir.path().into());
    screen.projects = ["alpha", "beta", "gamma"].map(String::from).into();
    let pane = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .horizontal_split(other, None, None, CLIENT, None, None)
        .unwrap();
    screen
        .focus_pane_with_id(pane, false, false, CLIENT)
        .unwrap();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("beta")
    );
    screen.pane_projects = [(pane, "gamma".into()), (other, "alpha".into())].into();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("gamma")
    );
    assert_eq!(screen.last_project.name().unwrap(), "gamma");

    let second = ClientId::new(2);
    screen.add_client(second).unwrap();
    screen.reconcile_project_contexts();
    screen
        .focus_pane_with_id(other, false, false, CLIENT)
        .unwrap();
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    assert_eq!(
        screen.client_projects[&second].current.as_deref(),
        Some("gamma")
    );
    assert_eq!(screen.last_project.name().unwrap(), "alpha");
    drop(std::mem::take(&mut screen.last_project));
    screen.last_project = LastProject::load(dir.path().into());
    assert_eq!(
        LastProject::load(dir.path().into()).name().unwrap(),
        "alpha"
    );

    // A different session's choice must survive unchanged clients reconciling again.
    LastProject::load(dir.path().into()).remember("beta");
    screen.reconcile_project_contexts();
    assert_eq!(LastProject::load(dir.path().into()).name().unwrap(), "beta");
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    assert_eq!(
        screen.client_projects[&second].current.as_deref(),
        Some("gamma")
    );
}

#[test]
fn remembered_project_fallbacks_do_not_replace_the_saved_preference() {
    use yinz_runtime::last_project::LastProject;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("last-project.json");
    for contents in [None, Some("malformed"), Some("\"deleted\"")] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        let mut screen = screen();
        screen.last_project = LastProject::load(dir.path().into());
        screen.projects = ["alpha", "beta"].map(String::from).into();
        screen.reconcile_project_contexts();
        assert_eq!(
            screen.client_projects[&CLIENT].current.as_deref(),
            Some("alpha")
        );
        screen.reconcile_project_contexts();
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), contents);
    }
    LastProject::load(dir.path().into()).remember("beta");
    let mut screen = screen();
    screen.last_project = LastProject::load(dir.path().into());
    screen.projects = ["alpha", "beta"].map(String::from).into();
    screen.reconcile_project_contexts();
    screen.projects.remove("beta");
    screen.reconcile_project_contexts();
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some("alpha")
    );
    assert_eq!(LastProject::load(dir.path().into()).name().unwrap(), "beta");
}

#[test]
fn global_opacity_reloads_open_palettes_and_applies_when_reopened() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let paint = (1..=SIZE.rows)
        .map(|row| format!("\x1b[{row};1H{}", "w".repeat(SIZE.cols)))
        .collect::<String>();
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(1),
            format!("\x1b[?7l\x1b[38;2;200;100;0m{paint}").into_bytes(),
        )
        .unwrap();
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    screen.toggle_command_palette(CLIENT);
    let mut tinted = None;
    let mut control = None;
    for (index, opacity) in [85, 100, 85, 0, 85].into_iter().enumerate() {
        let mut config = test_screen_config();
        config.background_opacity = opacity;
        screen
            .reconfigure(config, Keybinds::default(), CLIENT)
            .unwrap();
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("Do · Project:"), "{frame}");
        let cells = grid.as_character_lines();
        assert_eq!(*control.get_or_insert(cells[5][11].clone()), cells[5][11]);
        // Sample blank palette space, away from the input, choices and border.
        let (x, y) = (60, 3);
        let cell = cells[y][x].clone();
        assert_eq!(cell.character, 'w');
        if opacity == 100 {
            // The compositor hides backing glyphs by blending foreground and background to the same tint.
            assert_eq!(cell.styles.foreground, cell.styles.background);
        }
        if opacity == 85 {
            assert_eq!(*tinted.get_or_insert(cell.clone()), cell);
        } else if opacity == 0 {
            assert_ne!(
                tinted.as_ref().unwrap().styles.foreground,
                cell.styles.foreground
            );
        }
        screen.close_command_palette(CLIENT);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        // Tabs created after a reload inherit it too.
        let terminal = u32::try_from(index + 3).unwrap();
        new_tab(&mut screen, terminal, TabId::new(index + 1));
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(terminal),
                format!("\x1b[?7l\x1b[38;2;200;100;0m{paint}").into_bytes(),
            )
            .unwrap();
        // A plugin's percentage opts in; a newly created overlay uses the same global tint.
        let overlay = PaneId::Terminal(TerminalId::new(2));
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .new_pane(
                overlay,
                None,
                None,
                false,
                true,
                yinz_types::data::NewPanePlacement::Overlay(yinz_types::data::OverlayPlacement {
                    background_opacity: Some(1),
                    coordinates: Some(yinz_types::geometry::FloatingPaneCoordinates {
                        x: Some(yinz_types::geometry::PercentOrFixed::Fixed(8)),
                        y: Some(yinz_types::geometry::PercentOrFixed::Fixed(3)),
                        width: Some(yinz_types::geometry::PercentOrFixed::Fixed(64)),
                        height: Some(yinz_types::geometry::PercentOrFixed::Fixed(11)),
                        borderless: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                Some(CLIENT),
                None,
            )
            .unwrap();
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let overlay_cell = grid.as_character_lines()[y][x].clone();
        assert_eq!(
            overlay_cell.character,
            if opacity == 100 { ' ' } else { 'w' }
        );
        if opacity < 100 {
            assert_eq!(overlay_cell.styles.foreground, cell.styles.foreground);
            assert_eq!(overlay_cell.styles.background, cell.styles.background);
        }
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .close_pane(overlay, false, None);
        screen.toggle_command_palette(CLIENT);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(grid.as_character_lines()[y][x], cell);
    }
}

#[test]
fn plugin_palette_rejects_obsolete_work_and_scopes_close_to_its_opening() {
    for static_actions in [false, true] {
        plugin_palette_rejects_obsolete_work_and_scopes_close_to_its_opening_with_source(
            static_actions,
        );
    }
}

fn plugin_palette_rejects_obsolete_work_and_scopes_close_to_its_opening_with_source(
    static_actions: bool,
) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_bus::command_palette::{Delivery, Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Action, Command, Message, Reply};
    use yinz_types::internal_ids::PluginId;

    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let owner = Owner {
        plugin_id: PluginId::new(50),
        generation: 9,
        client_id: Some(CLIENT),
    };
    let unavailable = Arc::new(AtomicBool::new(false));
    let command = |command| Instruction::Command {
        owner: owner.clone(),
        client: CLIENT,
        unavailable: unavailable.clone(),
        command,
    };
    screen.plugin_palette_instruction(command(Command::Register {
        static_actions,
        id: "remote".into(),
        title: "Remote".into(),
        registration: "1".into(),
    }));
    screen.toggle_command_palette(CLIENT);
    let next = || {
        requests
            .try_iter()
            .find_map(|(request, _)| match request {
                PluginInstruction::PluginPalette(
                    delivery @ Delivery {
                        message: Message::Discover { .. } | Message::Execute { .. },
                        ..
                    },
                ) => Some(delivery),
                _ => None,
            })
            .expect("private request")
    };
    let first = next();
    let Message::Discover {
        scope: first_scope,
        context,
        ..
    } = &first.message
    else {
        panic!()
    };
    assert_eq!(context.client_id, CLIENT.get());
    // Repainting never requests discovery again.
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!requests.try_iter().any(|(request, _)| matches!(
        request,
        PluginInstruction::PluginPalette(Delivery {
            message: Message::Discover { .. },
            ..
        })
    )));
    screen.command_palette_handles_input(&None, b"remote-query", false, CLIENT);
    assert!(!first.active.unwrap().load(Ordering::Acquire));
    let reply = |scope, label: &str| {
        command(Command::Reply {
            scope,
            result: Ok(Reply::Actions {
                actions: vec![Action {
                    aliases: vec!["remote-queryxpaste".into()],
                    id: "action".into(),
                    label: label.into(),
                    description: String::new(),
                    handle: "handle".into(),
                    cells: None,
                }],
            }),
        })
    };
    assert!(!screen.plugin_palette_instruction(reply(first_scope.clone(), "Obsolete")));
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(request, _)| match request {
            PluginInstruction::PluginPalette(Delivery {
                message: Message::Release { handles },
                ..
            }) => Some(handles),
            _ => None,
        })
        .collect();
    assert_eq!(releases, vec![vec!["handle".to_owned()]]);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("Obsolete"));
    // Drive the debounce deterministically, without sleeping.
    screen.plugin_palette_instruction(Instruction::Discover {
        client: CLIENT,
        opening: first_scope.opening,
        revision: first_scope.revision + 1,
    });
    let Message::Discover { scope, .. } = next().message else {
        panic!()
    };
    assert!(screen.plugin_palette_instruction(reply(scope, "Semantic result")));
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Semantic result"), "{snapshot}");
    // Typing and paste preserve the semantic action while replacement is debounced.
    palette_key(&mut screen, BareKey::Char('x'));
    screen.command_palette_handles_input(&None, b"paste", false, CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Semantic result"), "{snapshot}");
    assert!(snapshot.contains("Loading"), "{snapshot}");
    assert!(!requests.try_iter().any(|(request, _)| matches!(
        request,
        PluginInstruction::PluginPalette(Delivery {
            message: Message::Release { .. },
            ..
        })
    )));
    assert!(
        screen.plugin_palette_instruction(command(Command::Register {
            static_actions,
            id: "unrelated".into(),
            title: "Unrelated".into(),
            registration: "3".into(),
        }))
    );
    for (request, _) in requests.try_iter() {
        assert!(!matches!(
            request,
            PluginInstruction::PluginPalette(Delivery {
                message: Message::Release { .. },
                ..
            })
        ));
    }
    let native_actions = palette_actions_receiver(&mut screen);
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        CLIENT,
    );
    let execution = next();
    let Message::Execute { scope, handle, .. } = execution.message else {
        panic!()
    };
    assert_eq!(handle, "handle");
    assert!(native_actions.try_recv().is_err());
    // Loading an unrelated provider must not cancel an action already running.
    screen.plugin_palette_instruction(command(Command::Register {
        static_actions,
        id: "other".into(),
        title: "Other".into(),
        registration: "2".into(),
    }));
    assert!(execution.active.unwrap().load(Ordering::Acquire));
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        CLIENT,
    );
    assert!(!requests.try_iter().any(|(request, _)| matches!(
        request,
        PluginInstruction::PluginPalette(Delivery {
            message: Message::Execute { .. },
            ..
        })
    )));
    screen.plugin_palette_instruction(command(Command::Close {
        scope: scope.clone(),
    }));
    assert!(!screen.command_palettes.contains_key(&CLIENT));
    screen.toggle_command_palette(CLIENT);
    screen.plugin_palette_instruction(command(Command::Close {
        scope: scope.clone(),
    }));
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::Close),
    }));
    assert!(screen.command_palettes.contains_key(&CLIENT));
    screen.plugin_palette_instruction(Instruction::Dispose {
        plugin_id: owner.plugin_id,
        generation: owner.generation,
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("Remote"));
}

#[test]
fn plugin_palette_sections_sort_by_title_then_owner_and_timeout_independently() {
    for static_actions in [false, true] {
        plugin_palette_sections_sort_by_title_then_owner_and_timeout_independently_with_source(
            static_actions,
        );
    }
}

fn plugin_palette_sections_sort_by_title_then_owner_and_timeout_independently_with_source(
    static_actions: bool,
) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Action, Command, Message, Reply};
    use yinz_types::internal_ids::PluginId;
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let command = |owner, command| Instruction::Command {
        owner,
        client: CLIENT,
        unavailable: Arc::new(AtomicBool::new(false)),
        command,
    };
    for (id, title) in [(3, "Z provider"), (2, "A provider"), (1, "A provider")] {
        screen.plugin_palette_instruction(command(
            Owner {
                plugin_id: PluginId::new(id),
                generation: u64::from(id),
                client_id: None,
            },
            Command::Register {
                static_actions,
                id: "same-id".into(),
                title: title.into(),
                registration: "1".into(),
            },
        ));
    }
    screen.toggle_command_palette(CLIENT);
    let request = requests
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginPalette(delivery) => Some(delivery),
            _ => None,
        })
        .unwrap();
    let Message::Discover { scope, .. } = request.message else {
        panic!()
    };
    screen.command_palette_handles_input(&None, b"zz-no-native", false, CLIENT);
    screen.plugin_palette_instruction(Instruction::Discover {
        client: CLIENT,
        opening: scope.opening,
        revision: scope.revision + 1,
    });
    let mut deliveries: Vec<_> = requests.try_iter().filter_map(|(instruction, _)| match instruction {
        PluginInstruction::PluginPalette(delivery) if matches!(&delivery.message, Message::Discover { scope: current, .. } if current.revision == scope.revision + 1) => Some(delivery), _ => None,
    }).collect();
    deliveries.reverse();
    for delivery in deliveries {
        let Message::Discover { scope, .. } = delivery.message else {
            panic!()
        };
        let id = delivery.owner.plugin_id.get();
        if id == 3 {
            screen.plugin_palette_instruction(Instruction::Timeout {
                owner: delivery.owner,
                client: CLIENT,
                scope,
            });
            assert!(!delivery.active.unwrap().load(Ordering::Acquire));
        } else {
            screen.plugin_palette_instruction(command(
                delivery.owner,
                Command::Reply {
                    scope,
                    result: Ok(Reply::Actions {
                        actions: vec![Action {
                            aliases: vec!["zz-no-native".into()],
                            id: "same-action-id".into(),
                            label: format!("Result {id}"),
                            description: String::new(),
                            handle: "1".into(),
                            cells: None,
                        }],
                    }),
                },
            ));
        }
    }
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.find("Result 1").unwrap() < snapshot.find("Result 2").unwrap(),
        "{snapshot}"
    );
    assert!(
        snapshot.contains("timed out after 10 seconds"),
        "{snapshot}"
    );
    screen.command_palette_handles_input(&None, b"x", false, CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Loading"), "{snapshot}");
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        &[],
        false,
        CLIENT,
    );
    // Escape also cancels a query whose debounce has not dispatched yet.
    screen.plugin_palette_instruction(Instruction::Discover {
        client: CLIENT,
        opening: scope.opening,
        revision: scope.revision + 2,
    });
    assert!(!requests.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::PluginPalette(yinz_bus::command_palette::Delivery {
            message: Message::Discover { .. },
            ..
        })
    )));
    assert!(!screen.command_palettes.contains_key(&CLIENT));
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("Loading"), "{snapshot}");
}

#[test]
fn plugin_palette_availability_and_client_scope_control_discovery_and_rendering() {
    for static_actions in [false, true] {
        plugin_palette_availability_and_client_scope_control_discovery_and_rendering_with_source(
            static_actions,
        );
    }
}

fn plugin_palette_availability_and_client_scope_control_discovery_and_rendering_with_source(
    static_actions: bool,
) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Command, Message};
    use yinz_types::internal_ids::PluginId;

    for shared in [false, true] {
        for during_debounce in [false, true] {
            let mut screen = screen();
            let other = ClientId::new(2);
            screen.add_client(other).unwrap();
            let (sender, requests) = unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
            let unavailable = Arc::new(AtomicBool::new(false));
            let owner = Owner {
                plugin_id: PluginId::new(50),
                generation: 9,
                client_id: if shared { None } else { Some(CLIENT) },
            };
            let register = || Instruction::Command {
                owner: owner.clone(),
                client: CLIENT,
                unavailable: unavailable.clone(),
                command: Command::Register {
                    static_actions,
                    id: "provider".into(),
                    title: "Provider".into(),
                    registration: "1".into(),
                },
            };
            assert!(!screen.plugin_palette_instruction(register()));
            screen.toggle_command_palette(CLIENT);
            screen.toggle_command_palette(other);
            assert!(!screen.plugin_palette_instruction(register()));
            let discoveries: Vec<_> = requests
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    PluginInstruction::PluginPalette(delivery)
                        if matches!(delivery.message, Message::Discover { .. }) =>
                    {
                        Some(delivery)
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(discoveries.len(), if shared { 2 } else { 1 });
            let request = discoveries.iter().find(|d| d.client == CLIENT).unwrap();
            let Message::Discover { scope, .. } = &request.message else {
                panic!()
            };
            if !during_debounce {
                unavailable.store(true, Ordering::Release);
            }
            screen.command_palette_handles_input(&None, b"query", false, CLIENT);
            unavailable.store(true, Ordering::Release);
            let changed = screen.plugin_palette_instruction(Instruction::Discover {
                client: CLIENT,
                opening: scope.opening,
                revision: scope.revision + 1,
            });
            assert_eq!(changed, during_debounce);
            for request in discoveries {
                assert!(!request.active.unwrap().load(Ordering::Acquire));
            }
            assert!(!requests.try_iter().any(|(instruction, _)| matches!(
                instruction, PluginInstruction::PluginPalette(delivery)
                    if matches!(delivery.message, Message::Discover { .. })
                        || matches!(delivery.message, Message::Release { ref handles } if handles.is_empty())
            )));
            assert!(!screen.plugin_palette_instruction(Instruction::Dispose {
                plugin_id: owner.plugin_id,
                generation: owner.generation,
            }));
        }
    }
}

#[test]
fn plugin_palette_filter_submission_refreshes_results_and_releases_old_handles() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply};

    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let owner = Owner {
        plugin_id: PluginId::new(50),
        generation: 9,
        client_id: Some(CLIENT),
    };
    let unavailable = Arc::new(AtomicBool::new(false));
    let command = |command| Instruction::Command {
        owner: owner.clone(),
        client: CLIENT,
        unavailable: unavailable.clone(),
        command,
    };
    let next = || {
        requests
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PluginPalette(delivery) => match delivery.message {
                    Message::Discover { scope, query, .. } => Some((scope, query)),
                    _ => None,
                },
                _ => None,
            })
            .expect("plugin discovery")
    };
    let reply = |scope, label: &str, handle: &str| {
        command(Command::Reply {
            scope,
            result: Ok(Reply::Actions {
                actions: vec![PaletteAction {
                    aliases: vec![],
                    id: "stable".into(),
                    label: label.into(),
                    description: String::new(),
                    handle: handle.into(),
                    cells: None,
                }],
            }),
        })
    };
    screen.plugin_palette_instruction(command(Command::Register {
        static_actions: false,
        id: "provider".into(),
        title: "Provider".into(),
        registration: "1".into(),
    }));
    screen.toggle_command_palette(CLIENT);
    let (scope, _) = next();
    assert!(screen.plugin_palette_instruction(reply(scope, "Previous result", "old")));
    assert!(screen.command_palette_handles_input(&None, b"Agent filters", false, CLIENT));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Previous result"), "{frame}");
    palette_key(&mut screen, BareKey::Enter);
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()),
        &[],
        false,
        CLIENT,
    ));
    assert!(screen.command_palette_handles_input(&None, b"hello provider:codex", false, CLIENT));
    // Up wraps from Search to Apply; submit through the normal input handler.
    palette_key(&mut screen, BareKey::Up);
    palette_key(&mut screen, BareKey::Enter);
    let (scope, query) = next();
    let parsed = yinz_bus::command_palette::query::AgentQuery::parse(
        &query,
        yinz_bus::command_palette::query::LIVE_FIELDS,
    )
    .unwrap();
    assert_eq!(parsed.text, "hello provider:codex");
    assert!(parsed.filters.is_empty());
    assert_eq!(scope.view, 0);
    assert!(screen.plugin_palette_instruction(reply(scope, "Replacement result", "new")));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Replacement result"), "{frame}");
    assert!(!frame.contains("Previous result"), "{frame}");
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginPalette(delivery) => match delivery.message {
                Message::Release { handles } => Some(handles),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(releases, vec![vec!["old".to_owned()]]);
}

#[test]
fn plugin_palette_replacements_release_handles_and_nested_removal_restores_root() {
    for static_actions in [false, true] {
        plugin_palette_replacements_release_handles_and_nested_removal_restores_root_with_source(
            static_actions,
        );
    }
}

fn plugin_palette_replacements_release_handles_and_nested_removal_restores_root_with_source(
    static_actions: bool,
) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply, View};
    use yinz_types::internal_ids::PluginId;

    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let unavailable = Arc::new(AtomicBool::new(false));
    let owner = Owner {
        plugin_id: PluginId::new(50),
        generation: 9,
        client_id: Some(CLIENT),
    };
    let command = |command| Instruction::Command {
        owner: owner.clone(),
        client: CLIENT,
        unavailable: unavailable.clone(),
        command,
    };
    let action = |handle: &str| PaletteAction {
        aliases: vec!["zz-no-nativeretry".into()],
        id: "stable".into(),
        label: "Retained action".into(),
        description: String::new(),
        handle: handle.into(),
        cells: None,
    };
    let next = || {
        requests
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PluginPalette(delivery)
                    if matches!(
                        delivery.message,
                        Message::Discover { .. } | Message::Execute { .. }
                    ) =>
                {
                    Some(delivery)
                },
                _ => None,
            })
            .unwrap()
    };
    screen.plugin_palette_instruction(command(Command::Register {
        static_actions,
        id: "provider".into(),
        title: "Provider".into(),
        registration: "1".into(),
    }));
    screen.toggle_command_palette(CLIENT);
    let Message::Discover { scope, .. } = next().message else {
        panic!()
    };
    assert!(screen.plugin_palette_instruction(command(Command::Reply {
        scope: scope.clone(),
        result: Ok(Reply::Actions {
            actions: vec![action("old")]
        }),
    })));
    screen.command_palette_handles_input(&None, b"zz-no-native", false, CLIENT);
    assert!(!screen.plugin_palette_instruction(Instruction::Discover {
        client: CLIENT,
        opening: scope.opening,
        revision: scope.revision + 1,
    }));
    let Message::Discover { scope, .. } = next().message else {
        panic!()
    };
    assert!(screen.plugin_palette_instruction(command(Command::Reply {
        scope: scope.clone(),
        result: Ok(Reply::Actions {
            actions: vec![action("new")]
        }),
    })));
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginPalette(delivery) => match delivery.message {
                Message::Release { handles } => Some(handles),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(releases, vec![vec!["old".to_owned()]]);
    screen.command_palette_handles_input(&None, b"retry", false, CLIENT);
    assert!(!screen.plugin_palette_instruction(Instruction::Discover {
        client: CLIENT,
        opening: scope.opening,
        revision: scope.revision + 1,
    }));
    let Message::Discover { scope, .. } = next().message else {
        panic!()
    };
    assert!(screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Err("discovery failed".into()),
    })));
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, handle, .. } = next().message else {
        panic!()
    };
    assert_eq!(handle, "new");
    assert!(screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::View {
            view: View::Actions {
                columns: vec![],
                description: "Choose a configuration section.".into(),
                initial_query: "nested query".into(),
                title: "Nested".into(),
                actions: vec![action("nested")],
                source: Some("source".into()),
            }
        }),
    })));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Choose a configuration section."), "{frame}");
    let nested_request = next();
    let Message::Discover { scope, .. } = nested_request.message else {
        panic!()
    };
    assert!(screen.plugin_palette_instruction(Instruction::Timeout {
        owner: owner.clone(),
        client: CLIENT,
        scope,
    }));
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, handle, .. } = next().message else {
        panic!()
    };
    assert_eq!(handle, "nested");
    // Finishing execution without a new view leaves the retained list available.
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Err("retry".into()),
    }));
    assert_palette_query(
        &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        "nested query",
    );
    palette_key(&mut screen, BareKey::Esc);
    assert_palette_query(
        &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        "zz-no-nativeretry",
    );
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, handle, .. } = next().message else {
        panic!()
    };
    assert_eq!(handle, "new");
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::View {
            view: View::Actions {
                columns: vec![],
                description: String::new(),
                initial_query: String::new(),
                title: "Nested again".into(),
                actions: vec![action("nested2")],
                source: Some("source2".into()),
            },
        }),
    }));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        !frame.contains("Choose a configuration section."),
        "{frame}"
    );
    let pending = next();
    unavailable.store(true, Ordering::Release);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!pending.active.unwrap().load(Ordering::Acquire));
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginPalette(delivery) => match delivery.message {
                Message::Release { handles } => Some(handles),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(releases.contains(&vec!["new".to_owned()]));
    assert!(releases.contains(&vec!["nested2".to_owned(), "source2".to_owned()]));
    assert!(releases.iter().all(|handles| !handles.is_empty()));
}

#[test]
fn plugin_palette_completion_clears_saved_searches_and_keeps_the_new_initial_query() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply, View};

    for initial_query in ["", "provided query"] {
        let mut screen = screen();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let (sender, requests) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        let owner = Owner {
            plugin_id: PluginId::new(50),
            generation: 9,
            client_id: Some(CLIENT),
        };
        let unavailable = Arc::new(AtomicBool::new(false));
        let command = |command| Instruction::Command {
            owner: owner.clone(),
            client: CLIENT,
            unavailable: unavailable.clone(),
            command,
        };
        let next = || {
            requests
                .try_iter()
                .find_map(|(instruction, _)| match instruction {
                    PluginInstruction::PluginPalette(delivery)
                        if matches!(
                            delivery.message,
                            Message::Discover { .. } | Message::Execute { .. }
                        ) =>
                    {
                        Some(delivery.message)
                    },
                    _ => None,
                })
                .expect("palette request")
        };
        let actions = || {
            vec![PaletteAction {
                id: "action".into(),
                label: "Provider action".into(),
                description: String::new(),
                aliases: vec![],
                handle: "run".into(),
                cells: None,
            }]
        };
        let view = |query: &str| View::Actions {
            columns: vec![],
            title: "Provider results".into(),
            description: String::new(),
            actions: actions(),
            source: Some("source".into()),
            initial_query: query.into(),
        };
        screen.plugin_palette_instruction(command(Command::Register {
            static_actions: false,
            id: "provider".into(),
            title: "Provider".into(),
            registration: "1".into(),
        }));
        screen.toggle_command_palette(CLIENT);
        let Message::Discover { scope, .. } = next() else {
            panic!()
        };
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Actions { actions: actions() }),
        }));
        screen.command_palette_handles_input(&None, b"plugin-search", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        let Message::Execute { scope, .. } = next() else {
            panic!()
        };
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::View {
                view: view("nested search"),
            }),
        }));
        let Message::Discover { scope, query, .. } = next() else {
            panic!()
        };
        assert_eq!(query, "nested search");
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Actions { actions: actions() }),
        }));

        palette_key(&mut screen, BareKey::Enter);
        let Message::Execute { scope, .. } = next() else {
            panic!()
        };
        palette_key(&mut screen, BareKey::Esc);
        assert!(!screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Complete {
                notification: "Cancelled completion".into(),
                view: Some(view(initial_query)),
            }),
        })));
        assert_palette_query(
            &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            "nested search",
        );

        palette_key(&mut screen, BareKey::Enter);
        let Message::Execute { scope, .. } = next() else {
            panic!()
        };
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Complete {
                notification: "Completed action".into(),
                view: Some(view(initial_query)),
            }),
        }));
        assert_palette_query(
            &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            initial_query,
        );
        let Message::Discover { scope, query, .. } = next() else {
            panic!()
        };
        assert_eq!(query, initial_query);
        screen.plugin_palette_instruction(command(Command::Reply {
            scope: scope.clone(),
            result: Ok(Reply::Actions { actions: actions() }),
        }));
        palette_key(&mut screen, BareKey::Esc);
        assert_palette_query(
            &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            "",
        );
        if !initial_query.is_empty() {
            screen.plugin_palette_instruction(Instruction::Discover {
                client: CLIENT,
                opening: scope.opening,
                revision: scope.revision + 1,
            });
        }
        let Message::Discover { scope, query, .. } = next() else {
            panic!()
        };
        assert_eq!(query, "");
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Actions { actions: actions() }),
        }));
        palette_key(&mut screen, BareKey::Esc);
        assert_palette_query(
            &render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            "",
        );
        let Message::Discover { scope, query, .. } = next() else {
            panic!()
        };
        assert_eq!(scope.view, 0);
        assert_eq!(query, "");
    }
}

#[test]
fn root_palette_escape_cancels_discovery_and_debounce() {
    for static_actions in [false, true] {
        root_palette_escape_cancels_discovery_and_debounce_with_source(static_actions);
    }
}

fn root_palette_escape_cancels_discovery_and_debounce_with_source(static_actions: bool) {
    for debounce in [false, true] {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use yinz_bus::command_palette::{Delivery, Instruction, Owner};
        use yinz_runtime::channels::{unbounded, SenderWithContext};
        use yinz_types::command_palette::{Action, Command, Message, Reply};
        use yinz_types::internal_ids::PluginId;

        let mut screen = screen();
        let receiver = screen_wired_to_server(&mut screen);
        let (sender, requests) = unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        let owner = Owner {
            plugin_id: PluginId::new(50),
            generation: 9,
            client_id: Some(CLIENT),
        };
        let unavailable = Arc::new(AtomicBool::new(false));
        let command = |command| Instruction::Command {
            owner: owner.clone(),
            client: CLIENT,
            unavailable: unavailable.clone(),
            command,
        };
        screen.plugin_palette_instruction(command(Command::Register {
            static_actions,
            id: "remote".into(),
            title: "Remote".into(),
            registration: "1".into(),
        }));
        screen.toggle_command_palette(CLIENT);
        let next = || {
            requests
                .try_iter()
                .find_map(|(request, _)| match request {
                    PluginInstruction::PluginPalette(
                        delivery @ Delivery {
                            message: Message::Discover { .. } | Message::Execute { .. },
                            ..
                        },
                    ) => Some(delivery),
                    _ => None,
                })
                .expect("private request")
        };
        let request = next();
        let Message::Discover { scope, .. } = request.message else {
            panic!()
        };
        if debounce {
            screen.command_palette_handles_input(&None, b"query", false, CLIENT);
        }
        palette_key(&mut screen, BareKey::Esc);
        assert!(!screen.command_palettes.contains_key(&CLIENT));
        assert!(!request.active.unwrap().load(Ordering::Acquire));
        assert!(requests.try_iter().any(|(instruction, _)| matches!(
            instruction,
            PluginInstruction::PluginPalette(Delivery {
                message: Message::Cancel { .. },
                ..
            })
        )));
        assert!(receiver
            .try_iter()
            .any(|(instruction, _)| matches!(instruction,
        ServerInstruction::SetCommandPaletteOpen(client, false) if client == CLIENT)));
        assert!(!screen.command_palette_handles_input(&None, b"unlocked", false, CLIENT));
        screen.plugin_palette_instruction(Instruction::Discover {
            client: CLIENT,
            opening: scope.opening,
            revision: scope.revision + u64::from(debounce),
        });
        screen.plugin_palette_instruction(command(Command::Reply {
            scope,
            result: Ok(Reply::Actions {
                actions: vec![Action {
                    aliases: vec![],
                    id: "late".into(),
                    label: "Late".into(),
                    description: String::new(),
                    handle: "late".into(),
                    cells: None,
                }],
            }),
        }));
        assert!(!screen.command_palettes.contains_key(&CLIENT));
        assert!(!requests.try_iter().any(|(instruction, _)| matches!(
            instruction,
            PluginInstruction::PluginPalette(Delivery {
                message: Message::Discover { .. },
                ..
            })
        )));
    }
}

fn warp_peer(screen: &mut Screen, name: &str) -> SessionName {
    screen.log_and_report_session_state().unwrap();
    let name = SessionName::new(name);
    let mut peer = screen.peer_sessions_cache[&screen.session_name].clone();
    peer.name = name.clone();
    peer.is_current_session = false;
    peer.tabs[0].tab_id = TabId::new(42);
    peer.tabs[0].name = "Remote tab".into();
    screen.peer_sessions_cache.insert(name.clone(), peer);
    name
}

#[test]
fn warp_palette_filters_peers_and_panes_and_revalidates_stale_selections() {
    let mut screen = screen();
    let source = warp_peer(&mut screen, "warp-peer");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen
        .resurrectable_sessions_cache
        .insert(SessionName::new("exited-peer"), Duration::ZERO);
    for command in ["Warp tab", "Warp pane"] {
        palette_search(&mut screen, command);
        palette_key(&mut screen, BareKey::Enter);
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(snapshot.contains("warp-peer"), "{snapshot}");
        assert!(!snapshot.contains("exited-peer"), "{snapshot}");
        for unavailable in [screen.session_name.to_string(), "exited-peer".into()] {
            screen.command_palette_handles_input(&None, unavailable.as_bytes(), false, CLIENT);
            let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
            assert!(snapshot.contains("No matches"), "{snapshot}");
            palette_key(&mut screen, BareKey::Enter);
            screen.command_palette_handles_input(
                &Some(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()),
                &[],
                false,
                CLIENT,
            );
        }
        // Only the peer is offered; the current session cannot be activated here.
        palette_key(&mut screen, BareKey::Enter);
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(snapshot.contains("Remote tab"), "{snapshot}");
        let saved = screen.peer_sessions_cache.remove(&source).unwrap();
        palette_key(&mut screen, BareKey::Enter);
        assert!(screen.command_palettes.contains_key(&CLIENT));
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            snapshot.contains("Source session no longer exists"),
            "{snapshot}"
        );
        screen.close_command_palette(CLIENT);
        screen.peer_sessions_cache.insert(source.clone(), saved);
    }
    for (suppressed, selectable, held, plugin, eligible) in [
        (true, true, false, false, false),
        (false, false, false, false, false),
        (false, true, true, false, true),
        (false, true, false, true, true),
    ] {
        let peer = screen.peer_sessions_cache.get_mut(&source).unwrap();
        let pane = peer.panes.panes.values_mut().flatten().next().unwrap();
        pane.title = "Candidate".into();
        pane.display_title = None;
        pane.is_suppressed = suppressed;
        pane.is_selectable = selectable;
        pane.is_held = held;
        pane.id = if plugin {
            PaneId::Plugin(PluginId::new(9))
        } else {
            PaneId::Terminal(TerminalId::new(1))
        };
        palette_search(&mut screen, "Warp pane");
        palette_key(&mut screen, BareKey::Enter);
        palette_key(&mut screen, BareKey::Enter);
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(snapshot.contains("Candidate"), eligible, "{snapshot}");
        // Remove it after rendering but before activation.
        screen
            .peer_sessions_cache
            .get_mut(&source)
            .unwrap()
            .panes
            .panes
            .values_mut()
            .flatten()
            .next()
            .unwrap()
            .is_selectable = false;
        palette_key(&mut screen, BareKey::Enter);
        assert!(screen.command_palettes.contains_key(&CLIENT));
        screen.close_command_palette(CLIENT);
    }
}

#[test]
fn warp_palette_worker_captures_ordered_ids_and_stops_after_failure() {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    for mode in ["batch", "tab", "pane"] {
        std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
        let directory = tempfile::tempdir_in(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
        let name = directory.path().file_name().unwrap().to_str().unwrap();
        // A unique sibling path is a session socket, while the directory reserves its name.
        let name = format!("{name}-warp");
        let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(&name);
        let listener = ListenerOptions::new()
            .name(path.to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let mut screen = screen();
        let source = warp_peer(&mut screen, &name);
        let destination = screen.session_name.clone();
        let peer = screen.peer_sessions_cache.get_mut(&source).unwrap();
        let mut second = peer.tabs[0].clone();
        second.position = yinz_types::internal_ids::TabPosition::new(1);
        second.tab_id = TabId::new(7);
        let mut third = second.clone();
        third.position = yinz_types::internal_ids::TabPosition::new(2);
        third.tab_id = TabId::new(99);
        peer.tabs = vec![second, peer.tabs[0].clone()];
        let notifications = palette_actions_receiver(&mut screen);
        palette_search(
            &mut screen,
            if mode == "pane" {
                "Warp pane"
            } else {
                "Warp tab"
            },
        );
        palette_key(&mut screen, BareKey::Enter);
        palette_key(&mut screen, BareKey::Enter);
        if mode == "batch" {
            screen.command_palette_handles_input(&None, b"All tabs", false, CLIENT);
            // Capture the latest metadata at activation, including a tab discovered after rendering.
            screen
                .peer_sessions_cache
                .get_mut(&source)
                .unwrap()
                .tabs
                .insert(0, third);
        }
        palette_key(&mut screen, BareKey::Enter);
        assert!(!screen.command_palettes.contains_key(&CLIENT));
        // Execution returned before any server reply: screen remains available for adoption.
        screen
            .peer_sessions_cache
            .get_mut(&source)
            .unwrap()
            .tabs
            .clear();
        for (index, expected) in (if mode == "batch" {
            vec![42, 7]
        } else {
            vec![42]
        })
        .into_iter()
        .enumerate()
        {
            let stream = listener.accept().unwrap();
            let mut sender: IpcSenderWithContext<ServerToClientMsg> =
                IpcSenderWithContext::new(stream);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            let IpcRecv::Msg(message, _) = receiver.recv_client_msg() else {
                panic!("missing warp request");
            };
            if mode == "pane" {
                assert!(matches!(message, ClientToServerMsg::Action {
                action: Action::WarpPane { pane_id: Some(PaneId::Terminal(id)), to_session,
                    placement: yinz_types::data::WarpPlacement::NewTab }, ..
            } if id == TerminalId::new(1) && to_session == destination));
            } else {
                assert!(matches!(message, ClientToServerMsg::Action {
                action: Action::WarpTab { tab_id: Some(id), to_session }, ..
            } if id == TabId::new(expected) && to_session == destination));
            }
            sender
                .send_server_msg(if index == 0 {
                    ServerToClientMsg::Log {
                        lines: vec!["done".into()],
                    }
                } else {
                    ServerToClientMsg::LogError {
                        lines: vec!["back in this session".into()],
                    }
                })
                .unwrap();
        }
        let (notification, _) = notifications.recv_timeout(Duration::from_secs(10)).unwrap();
        let ScreenInstruction::Notify(message, ..) = notification else {
            panic!("expected notification");
        };
        if mode == "batch" {
            assert!(
                message.contains("1/3 completed")
                    && message.contains("tab 7")
                    && message.contains("back in this session"),
                "{message}"
            );
        } else {
            assert!(message.contains("Brought"), "{message}");
        }
        listener
            .set_nonblocking(interprocess::local_socket::ListenerNonblockingMode::Accept)
            .unwrap();
        assert!(
            listener.accept().is_err(),
            "later tabs must not be requested"
        );
    }
}

#[test]
fn warp_palette_live_refresh_preserves_session_and_tab_identity() {
    let mut screen = screen();
    let source = warp_peer(&mut screen, "session");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "Warp tab");
    palette_key(&mut screen, BareKey::Enter);
    // Navigation makes the source choice explicit before discovery reorders it.
    palette_key(&mut screen, BareKey::Up);
    // Inserting an earlier session must not move the selection off the chosen source.
    let earlier = warp_peer(&mut screen, "42");
    screen.peer_sessions_cache.get_mut(&earlier).unwrap().tabs[0].name = "Wrong peer".into();
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    palette_key(&mut screen, BareKey::Enter);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("› Remote tab"), "{snapshot}");
    assert!(!snapshot.contains("Wrong peer"), "{snapshot}");
    palette_key(&mut screen, BareKey::Up);
    let peer = screen.peer_sessions_cache.get_mut(&source).unwrap();
    let mut inserted = peer.tabs[0].clone();
    inserted.name = "Earlier tab".into();
    inserted.tab_id = TabId::new(8);
    peer.tabs[0].position = TabPosition::new(1);
    peer.tabs[0].name = "Renamed tab".into();
    peer.tabs.push(inserted);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("› Renamed tab (tab 2)"), "{snapshot}");
    assert!(snapshot.find("Earlier tab").unwrap() < snapshot.find("Renamed tab").unwrap());
    assert!(snapshot.find("Renamed tab").unwrap() < snapshot.find("All tabs").unwrap());
    screen
        .peer_sessions_cache
        .get_mut(&source)
        .unwrap()
        .tabs
        .retain(|tab| tab.tab_id != TabId::new(42));
    palette_key(&mut screen, BareKey::Enter);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Warp target no longer exists"),
        "{snapshot}"
    );
    screen.peer_sessions_cache.get_mut(&source).unwrap().tabs[0].tab_id = TabId::new(42);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    palette_key(&mut screen, BareKey::Esc);
    palette_key(&mut screen, BareKey::Up);
    // Source "42" must not collide with tab 42 in source "session".
    palette_key(&mut screen, BareKey::Enter);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Wrong peer"), "{snapshot}");
}

#[test]
fn native_agent_palette_search_context_cancellation_and_stale_replies() {
    use yinz_bus::command_palette::{
        AgentAction, AgentChoice, AgentProject, AgentReply, Instruction,
    };
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::agent::provider::{self, AgentProvider};
    let registered = AgentProvider {
        kind: "_custom_".into(),
        executable: "palette-test-agent".into(),
        ..Default::default()
    };
    provider::register(98981, registered).unwrap();
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (tx, rx) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen.projects = ["**project**".into(), "other".into()].into_iter().collect();
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    let esc = Some(KeyWithModifier::new(BareKey::Esc));
    let next = || {
        rx.try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PaletteAgent(request) => Some(*request),
                _ => None,
            })
            .unwrap()
    };
    for kind in ["claude", "codex", "pi", "muse", "agy", "cursor", "_custom_"] {
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(
            &None,
            format!("{kind} project").as_bytes(),
            false,
            CLIENT,
        );
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            snapshot.contains(&format!("Spawn {kind} agent in project…")),
            "{snapshot}"
        );
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        let request = next();
        assert!(
            matches!(&request.action, AgentAction::Projects { kind: selected } if selected == kind)
        );
        screen.close_command_palette(CLIENT);
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(
            &None,
            format!("{kind} current directory").as_bytes(),
            false,
            CLIENT,
        );
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            snapshot.contains(&format!("Spawn {kind} agent in current directory")),
            "{snapshot}"
        );
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        let request = next();
        assert!(
            matches!(&request.action, AgentAction::Current { kind: selected } if selected == kind)
        );
        screen.close_command_palette(CLIENT);
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(
            &None,
            format!("{kind} worktree").as_bytes(),
            false,
            CLIENT,
        );
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            snapshot.contains(&format!("Spawn {kind} agent in worktree…")),
            "{snapshot}"
        );
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        let request = next();
        assert!(
            matches!(&request.action, AgentAction::Start { kind: selected, project: Some(project) } if selected == kind && project == "**project**")
        );
        // Repeated Enter cannot submit the request twice.
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        assert!(!rx
            .try_iter()
            .any(|(i, _)| matches!(i, PluginInstruction::PaletteAgent(_))));
        screen.command_palette_handles_input(&esc, b"", false, CLIENT);
        assert!(!request.active.load(std::sync::atomic::Ordering::Acquire));
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::Choices {
                title: "STALE".into(),
                choices: vec![],
            }),
        });
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(!snapshot.contains("STALE"));
        screen.close_command_palette(CLIENT);
    }
    provider::unregister_owner(98981);
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"claude worktree", false, CLIENT);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let request = next();
    let action = AgentAction::Checkout {
        kind: "claude".into(),
        project: AgentProject::Registered("**project**".into()),
        path: None,
    };
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: Ok(AgentReply::Choices {
            title: "**project** · Worktrees".into(),
            choices: vec![AgentChoice {
                label: "Create new worktree…".into(),
                description: "".into(),
                action: action.clone(),
            }],
        }),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("**project** · Worktrees"), "{snapshot}");
    assert_eq!(
        snapshot.matches("Create new worktree").count(),
        1,
        "{snapshot}"
    );
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let form = next();
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: form.opening,
        step: form.step,
        reply: Ok(AgentReply::Form {
            title: "Create new worktree".into(),
            labels: vec!["Branch name".into()],
            submit: "Launch".into(),
            action: action.clone(),
        }),
    });
    screen.command_palette_handles_input(&None, b"my-branch", false, CLIENT);
    screen.command_palette_handles_input(&esc, b"", false, CLIENT);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let form = next();
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: form.opening,
        step: form.step,
        reply: Ok(AgentReply::Form {
            title: "Create new worktree".into(),
            labels: vec!["Branch name".into()],
            submit: "Launch".into(),
            action,
        }),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("my-branch"), "{snapshot}");
}

#[test]
fn native_new_project_form_preserves_both_fields_and_guards_submission() {
    use yinz_bus::command_palette::{AgentAction, AgentReply, Instruction};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (tx, rx) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || {
        rx.try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PaletteAgent(request) => Some(*request),
                _ => None,
            })
            .unwrap()
    };
    let show_form = |screen: &mut Screen, request: &yinz_bus::command_palette::AgentRequest| {
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::Form {
                title: "New **project**_[name]".into(),
                labels: vec!["Project name".into(), "Destination directory".into()],
                submit: "Create and launch".into(),
                action: request.action.clone(),
            }),
        });
    };
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"claude project", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let projects = next();
    assert!(matches!(projects.action, AgentAction::Projects { .. }));
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: projects.opening,
        step: projects.step,
        reply: Ok(AgentReply::Choices {
            title: "Choose project".into(),
            choices: vec![yinz_bus::command_palette::AgentChoice {
                label: "Create new project…".into(),
                description: String::new(),
                action: AgentAction::NewProject {
                    kind: "claude".into(),
                },
            }],
        }),
    });
    palette_key(&mut screen, BareKey::Enter);
    show_form(&mut screen, &next());
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("New **project**_[name]"), "{snapshot}");
    assert!(snapshot.contains("Project name"), "{snapshot}");
    assert!(snapshot.contains("Destination directory"), "{snapshot}");
    assert!(snapshot.contains("Create and launch"), "{snapshot}");
    screen.command_palette_handles_input(&None, b"new-app", false, CLIENT);
    palette_key(&mut screen, BareKey::Tab);
    screen.command_palette_handles_input(&None, b"/tmp/new-app", false, CLIENT);
    palette_key(&mut screen, BareKey::Esc);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Choose project"), "{snapshot}");
    assert!(snapshot.contains("Create new project"), "{snapshot}");
    palette_key(&mut screen, BareKey::Enter);
    show_form(&mut screen, &next());
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("new-app"), "{snapshot}");
    assert!(snapshot.contains("/tmp/new-app"), "{snapshot}");
    for _ in 0..3 {
        palette_key(&mut screen, BareKey::Enter);
    }
    let submitted = next();
    assert!(matches!(submitted.action, AgentAction::NewProject { .. }));
    assert_eq!(submitted.values, ["new-app", "/tmp/new-app"]);
    palette_key(&mut screen, BareKey::Enter);
    assert!(!rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: submitted.opening,
        step: submitted.step,
        reply: Ok(AgentReply::Launched(
            yinz_types::agent::control::AgentResponse {
                outcome: yinz_types::agent::control::AgentOutcome::Rejected,
                message: Some("project destination already exists".into()),
                ..Default::default()
            },
        )),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("project destination already exists"),
        "{snapshot}"
    );
    assert!(snapshot.contains("new-app"), "{snapshot}");
    assert!(snapshot.contains("/tmp/new-app"), "{snapshot}");
    palette_key(&mut screen, BareKey::Enter);
    let submitted = next();
    assert_eq!(submitted.values, ["new-app", "/tmp/new-app"]);
    palette_key(&mut screen, BareKey::Esc);
    assert!(!submitted.active.load(std::sync::atomic::Ordering::Acquire));
    palette_key(&mut screen, BareKey::Enter);
    assert!(!rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Launch already submitted"), "{snapshot}");
}

#[test]
fn native_agent_launch_replies_are_client_and_opening_scoped_and_errors_do_not_retry() {
    use yinz_bus::command_palette::{AgentReply, Instruction};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::agent::control::{AgentOutcome, AgentResponse};
    let mut screen = screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (tx, rx) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    screen.toggle_command_palette(CLIENT);
    screen.toggle_command_palette(other);
    screen.command_palette_handles_input(&None, b"claude current directory", false, CLIENT);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let request = rx
        .try_iter()
        .find_map(|(i, _)| match i {
            PluginInstruction::PaletteAgent(r) => Some(r),
            _ => None,
        })
        .unwrap();
    let failure = || {
        Ok(AgentReply::Launched(AgentResponse {
            outcome: AgentOutcome::TimedOut,
            message: Some("Launch outcome unknown".into()),
            launch_resources: Some(yinz_types::agent::AgentLaunchResources {
                cwd: Some("/created/checkout".into()),
                branch: Some("_topic_".into()),
                ..Default::default()
            }),
            ..Default::default()
        }))
    };
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: other,
        opening: request.opening,
        step: request.step,
        reply: failure(),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, other, &mut grid);
    assert!(!snapshot.contains("outcome unknown"));
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: failure(),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Launch outcome unknown"), "{snapshot}");
    assert!(snapshot.contains("/created/checkout"), "{snapshot}");
    assert!(snapshot.contains("Branch: _topic_"), "{snapshot}");
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    assert!(!rx
        .try_iter()
        .any(|(i, _)| matches!(i, PluginInstruction::PaletteAgent(_))));
    screen.close_command_palette(CLIENT);
    screen.toggle_command_palette(CLIENT);
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: failure(),
    });
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("outcome unknown"));
}

#[test]
fn live_agent_results_refresh_panel_presentation_in_root_and_picker() {
    use yinz_bus::command_palette::{AgentLabels, AgentLabelsRequest, Instruction};
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

    for picker in [false, true] {
        let mut screen = screen();
        let pane = PaneId::Terminal(TerminalId::new(1));
        let mut agent = AgentInfo {
            kind: "codex".into(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: Some("Run `cargo` **tests**".into()),
            waiting_on: Some("Choose [a] or *b*".into()),
            wait_needs_a_person: true,
            cwd: Some("/work/tree".into()),
            session_id: None,
            run_id: None,
        };
        screen.with_pane(pane, |pane| {
            pane.update_name("**task**_[title]");
        });
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .set_pane_agent(pane, Some(agent.clone()));
        let receiver = screen_wired_to_server(&mut screen);
        let (plugin_tx, plugin_rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        let next_labels = || loop {
            let (instruction, _) = plugin_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgentLabels(request) = instruction {
                break request;
            }
        };
        let reply =
            |request: &AgentLabelsRequest, project: &str, branch: &str| Instruction::AgentLabels {
                client: request.client,
                opening: request.opening,
                revision: request.revision,
                labels: Ok(request
                    .sources
                    .iter()
                    .map(|source| {
                        (
                            source.id.clone(),
                            AgentLabels {
                                project: project.into(),
                                branch: branch.into(),
                            },
                        )
                    })
                    .collect()),
            };
        let mut grid = grid_for_render_snapshot(SIZE);
        palette_search(
            &mut screen,
            if picker { "Agents" } else { "provider:codex" },
        );
        if picker {
            palette_key(&mut screen, BareKey::Enter);
            // The filter action leads the picker; select the live pane explicitly.
            palette_key(&mut screen, BareKey::Down);
        }
        let request = next_labels();
        assert_eq!(request.sources.len(), 1);
        assert_eq!(request.sources[0].cwd.as_deref(), Some("/work/tree"));
        screen.plugin_palette_instruction(reply(&request, "**project**", "topic/雪"));
        for (state, main, activity) in [
            (
                AgentState::Busy,
                "● codex  working",
                "Run `cargo` **tests**",
            ),
            (
                AgentState::Waiting,
                "! codex  needs input",
                "Choose [a] or *b*",
            ),
            (AgentState::Idle, "○ codex  idle", "**task**_[title]"),
        ] {
            agent.state = state;
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .set_pane_agent(pane, Some(agent.clone()));
            let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
            assert!(frame.contains(main), "{frame}");
            assert!(frame.contains("› **project** · topic/雪"), "{frame}");
            assert!(frame.contains(activity), "{frame}");
            assert!(frame.contains("[1]"), "{frame}");
        }
        // Project renames and branch changes at the same cwd invalidate the labels.
        screen.invalidate_agent_event_projects();
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let refresh = next_labels();
        assert!(refresh.revision > request.revision);
        screen.plugin_palette_instruction(reply(&request, "stale", "old"));
        screen.plugin_palette_instruction(reply(&refresh, "renamed", "new-branch"));
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("› renamed · new-branch"), "{frame}");
        assert!(!frame.contains("stale"), "{frame}");

        // A reply for the previous checkout cannot overwrite a new location.
        screen.invalidate_agent_event_projects();
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let previous = next_labels();
        agent.cwd = Some("/work/other".into());
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .set_pane_agent(pane, Some(agent));
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        screen.plugin_palette_instruction(reply(&previous, "wrong checkout", "old"));
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let moved = next_labels();
        assert_eq!(moved.sources[0].cwd.as_deref(), Some("/work/other"));
        screen.plugin_palette_instruction(reply(&moved, "other", "resolved-worktree"));
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("› other"), "{frame}");
        assert!(!frame.contains("wrong checkout"), "{frame}");
        // Resolved labels are searchable even when absent from the pane metadata.
        assert!(screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()),
            &[],
            false,
            CLIENT,
        ));
        assert!(screen.command_palette_handles_input(&None, b"resolved-worktree", false, CLIENT));
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("other · resolved-worktree"), "{frame}");
        assert!(frame.contains("[1]"), "{frame}");
    }
}

#[test]
fn agent_info_keeps_missing_history_readable_and_rejects_replaced_identity() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    let mut screen = screen();
    let pane = PaneId::Terminal(TerminalId::new(1));
    let agent = AgentInfo {
        kind: "codex".into(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: Some("inspection task".into()),
        waiting_on: Some("permission".into()),
        wait_needs_a_person: true,
        cwd: Some("/work/café space".into()),
        session_id: None,
        run_id: None,
    };
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_agent(pane, Some(agent.clone()));
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.with_pane(pane, |pane| {
        pane.update_name("shell");
        pane.set_title_prefix(Some("project:branch".into()));
    });
    palette_search(&mut screen, "state:waiting directory:\"café space\"");
    let result = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(result.contains("codex"), "{result}");
    assert!(result.contains("! codex  needs input"), "{result}");
    assert!(result.contains("permission"), "{result}");
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    screen.command_palette_handles_input(&enter, b"\r", false, CLIENT);
    let info = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        info.contains("No recorded history") && info.contains("Focus"),
        "{info}"
    );
    assert!(info.contains("! codex  needs input"), "{info}");
    let mut busy = agent.clone();
    busy.state = AgentState::Busy;
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_agent(pane, Some(busy));
    let updated = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(updated.contains("● codex  working"), "{updated}");
    assert!(updated.contains("No recorded history"), "{updated}");
    let mut replacement = agent;
    replacement.session_id = Some("replacement".into());
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_agent(pane, Some(replacement));
    screen.command_palette_handles_input(&enter, b"\r", false, CLIENT);
    let timeline = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(timeline.contains("Agent info"), "{timeline}");
    assert!(!timeline.contains("changed identity"), "{timeline}");
    // Search -> empty messages: Enter must not invoke Focus.
    palette_key(&mut screen, BareKey::Tab);
    palette_key(&mut screen, BareKey::Enter);
    // Messages -> prompt -> Focus.
    palette_key(&mut screen, BareKey::Tab);
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Tab)),
        b"\t",
        false,
        CLIENT,
    );
    screen.command_palette_handles_input(&enter, b"\r", false, CLIENT);
    let stale = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(stale.contains("changed identity"), "{stale}");
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        b"\x1b",
        false,
        CLIENT,
    );
    palette_key(&mut screen, BareKey::Esc); // After collapsing the expanded search, return.
    let restored = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(restored.contains("state:waiting"), "{restored}");
}

#[test]
fn archive_queries_discard_stale_replies_and_resume_cannot_be_submitted_twice() {
    use yinz_bus::command_palette::{
        AgentReply, HistoryAction, HistoryReply, HistoryRow, Instruction,
    };
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || loop {
        let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let PluginInstruction::PaletteAgent(request) = instruction {
            if matches!(
                request.action,
                yinz_bus::command_palette::AgentAction::Usage { .. }
            ) {
                continue;
            }
            break *request;
        }
    };
    let reply = |request: &yinz_bus::command_palette::AgentRequest, label: &str, action| {
        Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::History(HistoryReply::Rows {
                rows: vec![HistoryRow {
                    id: "run".into(),
                    label: label.into(),
                    description: String::new(),
                    action,
                }],
                status: String::new(),
            })),
        }
    };
    let run = HistoryAction::Run {
        id: "recorded".into(),
        from: None,
        query: String::new(),
    };
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    palette_search(&mut screen, "Archived agents");
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let old = next();
    screen.command_palette_handles_input(&None, b"needle", false, CLIENT);
    let current = next();
    assert!(!old.active.load(std::sync::atomic::Ordering::Acquire));
    screen.plugin_palette_instruction(reply(&old, "STALE ARCHIVE", run.clone()));
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!snapshot.contains("STALE ARCHIVE"));
    let mut response = reply(&current, "[report]: /tmp/output", run);
    if let Instruction::AgentReply {
        reply: Ok(AgentReply::History(HistoryReply::Rows { rows, .. })),
        ..
    } = &mut response
    {
        rows[0].description = "dev_test · /tmp/my_dir".into();
    }
    screen.plugin_palette_instruction(response);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("[report]: /tmp/output"), "{snapshot}");
    assert!(snapshot.contains("dev_test · /tmp/my_dir"), "{snapshot}");
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Down)),
        b"",
        false,
        CLIENT,
    );
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let details = next();
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: details.opening,
        step: details.step,
        reply: Ok(AgentReply::History(history_run(vec![HistoryRow {
            id: "resume".into(),
            label: "Resume".into(),
            description: String::new(),
            action: HistoryAction::Resume {
                id: "recorded".into(),
            },
        }]))),
    });
    palette_key(&mut screen, BareKey::Tab); // Search -> messages.
    palette_key(&mut screen, BareKey::Tab); // Messages -> Resume.
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let submitted = next();
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        b"",
        false,
        CLIENT,
    );
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("may still complete"), "{snapshot}");
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    assert!(!submitted.active.load(std::sync::atomic::Ordering::Acquire));
    assert!(!rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
}

#[test]
fn history_live_run_id_updates_retarget_reads_and_reset_pagination() {
    use yinz_bus::command_palette::{
        AgentAction, AgentReply, HistoryAction, HistoryRow, Instruction,
    };
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

    for session_id in [None, Some("provider-session".to_owned())] {
        let mut screen = screen();
        let pane = PaneId::Terminal(TerminalId::new(1));
        let mut agent = AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id,
            run_id: None,
        };
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .set_pane_agent(pane, Some(agent.clone()));
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        let next = || loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                if matches!(
                    request.action,
                    yinz_bus::command_palette::AgentAction::Usage { .. }
                ) {
                    continue;
                }
                break *request;
            }
        };
        let (tick_tx, tick_rx) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(tick_tx));
        palette_search(&mut screen, "provider:codex");
        screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Enter)),
            b"",
            false,
            CLIENT,
        );
        if agent.session_id.is_some() {
            let request = next();
            assert!(
                matches!(&request.action, AgentAction::History(HistoryAction::Run { id, .. }) if id == "codex:provider-session")
            );
            screen.plugin_palette_instruction(Instruction::AgentReply {
                client: CLIENT,
                opening: request.opening,
                step: request.step,
                reply: Ok(AgentReply::History(history_run(vec![HistoryRow {
                    id: "event:90".into(),
                    label: "Old event".into(),
                    description: String::new(),
                    action: HistoryAction::Event {
                        id: "codex:provider-session".into(),
                        revision: 90,
                    },
                }]))),
            });
        }
        palette_key(&mut screen, BareKey::Enter); // Expand Search.
        screen.command_palette_handles_input(&None, b"needle", false, CLIENT);
        let query = next();
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: query.opening,
            step: query.step,
            reply: Ok(AgentReply::History(history_run(vec![]))),
        });
        agent.run_id = Some("explicit-run".into());
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .set_pane_agent(pane, Some(agent));
        loop {
            let (instruction, _) = tick_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let ScreenInstruction::CommandPalette(tick @ Instruction::HistoryTick { .. }) =
                instruction
            {
                let Instruction::HistoryTick { generation, .. } = tick else {
                    unreachable!()
                };
                screen.plugin_palette_instruction(tick);
                if generation == 2 {
                    break;
                }
                assert!(!rx.try_iter().any(|(instruction, _)| matches!(
                    instruction,
                    PluginInstruction::PaletteAgent(_)
                )));
            }
        }
        let updated = next();
        assert!(
            matches!(&updated.action, AgentAction::History(HistoryAction::Run { id, from: None, query }) if id == "explicit-run" && query == "needle")
        );
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: updated.opening,
            step: updated.step,
            reply: Ok(AgentReply::History(history_run(vec![]))),
        });
        palette_key(&mut screen, BareKey::Esc);
        let cleared = next();
        assert!(
            matches!(&cleared.action, AgentAction::History(HistoryAction::Run { id, query, .. }) if id == "explicit-run" && query.is_empty())
        );
    }
}

#[test]
fn history_typing_preserves_pending_hit_event_and_tools_replies() {
    use std::sync::atomic::Ordering;
    use yinz_bus::command_palette::{
        AgentReply, HistoryAction, HistoryReply, HistoryRow, Instruction,
    };

    for action in [
        HistoryAction::Hit {
            id: "recorded".into(),
            revision: 7,
        },
        HistoryAction::Event {
            id: "recorded".into(),
            revision: 7,
        },
        HistoryAction::Tools {
            id: "recorded".into(),
            revisions: vec![7, 8],
        },
    ] {
        let mut screen = screen();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        let next = || loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                if matches!(
                    request.action,
                    yinz_bus::command_palette::AgentAction::Usage { .. }
                ) {
                    continue;
                }
                break *request;
            }
        };
        let enter = Some(KeyWithModifier::new(BareKey::Enter));
        palette_search(&mut screen, "Archived agents");
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        let search = next();
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: search.opening,
            step: search.step,
            reply: Ok(AgentReply::History(HistoryReply::Rows {
                rows: vec![HistoryRow {
                    id: "result".into(),
                    label: "Result".into(),
                    description: String::new(),
                    action: action.clone(),
                }],
                status: String::new(),
            })),
        });
        screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Down)),
            b"",
            false,
            CLIENT,
        );
        screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        let request = loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                if matches!(
                    request.action,
                    yinz_bus::command_palette::AgentAction::Usage { .. }
                ) {
                    continue;
                }
                break *request;
            }
        };
        palette_key(&mut screen, BareKey::Char('n'));
        assert!(request.active.load(Ordering::Acquire));
        screen.command_palette_handles_input(&None, b"eedle", false, CLIENT);
        assert!(request.active.load(Ordering::Acquire));
        let loading = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(loading.contains("Loading"), "{loading}");
        let reply = match action {
            HistoryAction::Hit { .. } => history_run(vec![HistoryRow {
                id: "event:7".into(),
                label: "Loaded history content".into(),
                description: String::new(),
                action: HistoryAction::Event {
                    id: "recorded".into(),
                    revision: 7,
                },
            }]),
            _ => HistoryReply::Reader {
                header: "Event".into(),
                markdown: "Loaded history content".into(),
                tool: String::new(),
            },
        };
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::History(reply)),
        });
        let loaded = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(loaded.contains("Loaded history content"), "{loaded}");
        assert!(!loaded.contains("Loading"), "{loaded}");
    }
}

#[test]
fn history_poll_generations_reject_ticks_after_search_refresh_and_event_return() {
    use yinz_bus::command_palette::{
        AgentReply, HistoryAction, HistoryReply, HistoryRow, Instruction,
    };
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    let mut screen = screen();
    screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
        PaneId::Terminal(TerminalId::new(1)),
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: Some("session".into()),
            run_id: Some("run".into()),
        }),
    );
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || loop {
        let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let PluginInstruction::PaletteAgent(request) = instruction {
            if matches!(
                request.action,
                yinz_bus::command_palette::AgentAction::Usage { .. }
            ) {
                continue;
            }
            break *request;
        }
    };
    let (tx, ticks) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(tx));
    let next_tick = || loop {
        let (instruction, _) = ticks.recv_timeout(Duration::from_secs(5)).unwrap();
        if let ScreenInstruction::CommandPalette(tick @ Instruction::HistoryTick { .. }) =
            instruction
        {
            break tick;
        }
    };
    let reply = |request: &yinz_bus::command_palette::AgentRequest| Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: Ok(AgentReply::History(history_run(vec![
            HistoryRow {
                id: "latest".into(),
                label: "Latest / Refresh".into(),
                description: String::new(),
                action: HistoryAction::Run {
                    id: "run".into(),
                    from: None,
                    query: String::new(),
                },
            },
            HistoryRow {
                id: "event:1".into(),
                label: "Event".into(),
                description: String::new(),
                action: HistoryAction::Event {
                    id: "run".into(),
                    revision: 1,
                },
            },
        ]))),
    };
    palette_search(&mut screen, "provider:codex");
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    screen.plugin_palette_instruction(reply(&next()));
    let mut obsolete = vec![next_tick()];
    for operation in ["search", "refresh", "event"] {
        if operation == "search" {
            palette_key(&mut screen, BareKey::Enter); // Expand Search.
            screen.command_palette_handles_input(&None, b"needle", false, CLIENT);
        } else {
            if operation == "refresh" {
                // Search -> messages -> prompt -> Focus -> Latest / Refresh.
                for key in [BareKey::Tab, BareKey::Tab, BareKey::Tab, BareKey::Right] {
                    palette_key(&mut screen, key);
                }
            }
            screen.command_palette_handles_input(&enter, b"", false, CLIENT);
        }
        let request = next();
        if operation == "event" {
            screen.plugin_palette_instruction(Instruction::AgentReply {
                client: CLIENT,
                opening: request.opening,
                step: request.step,
                reply: Ok(AgentReply::History(HistoryReply::Reader {
                    header: "Event".into(),
                    markdown: "Event detail".into(),
                    tool: String::new(),
                })),
            });
            screen.command_palette_handles_input(
                &Some(KeyWithModifier::new(BareKey::Esc)),
                b"",
                false,
                CLIENT,
            );
            screen.plugin_palette_instruction(reply(&next()));
        } else {
            screen.plugin_palette_instruction(reply(&request));
        }
        for tick in &obsolete {
            screen.plugin_palette_instruction(tick.clone());
        }
        assert!(!rx
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
        // Wait for the replacement timer before checking the archive channel, so a
        // wrongly accepted tick has time to pass the request debounce.
        let current = next_tick();
        assert!(!rx
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
        screen.plugin_palette_instruction(current.clone());
        let request = next();
        // A consumed generation cannot launch a second request after this reply.
        screen.plugin_palette_instruction(reply(&request));
        obsolete.push(current);
        for tick in &obsolete {
            screen.plugin_palette_instruction(tick.clone());
        }
        obsolete.push(next_tick());
        assert!(!rx
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    }
}

#[test]
fn archive_filter_forms_preserve_literal_search_text() {
    use yinz_bus::command_palette::{
        query::{AgentQuery, ARCHIVE_FIELDS},
        AgentAction, AgentReply, HistoryAction, HistoryReply, Instruction,
    };
    let mut screen = screen();
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || loop {
        let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let PluginInstruction::PaletteAgent(request) = instruction {
            if matches!(
                request.action,
                yinz_bus::command_palette::AgentAction::Usage { .. }
            ) {
                continue;
            }
            break *request;
        }
    };
    let reply = |request: &yinz_bus::command_palette::AgentRequest| Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: Ok(AgentReply::History(HistoryReply::Rows {
            rows: vec![],
            status: String::new(),
        })),
    };
    palette_search(&mut screen, "Archived agents");
    palette_key(&mut screen, BareKey::Enter);
    let initial = next();
    screen.plugin_palette_instruction(reply(&initial));
    let input = r#""hello provider:codex" provider:claude"#;
    screen.command_palette_handles_input(&None, input.as_bytes(), false, CLIENT);
    let request = next();
    screen.plugin_palette_instruction(reply(&request));
    let mut expected = AgentQuery::parse(input, ARCHIVE_FIELDS).unwrap();
    for text in [
        None,
        Some("provider:codex"),
        Some("  café \"quoted\" C:\\work\\  "),
    ] {
        // Filters is the first archive action. Opening it preserves the existing query.
        palette_key(&mut screen, BareKey::Enter);
        if let Some(text) = text {
            screen.command_palette_handles_input(
                &Some(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier()),
                &[],
                false,
                CLIENT,
            );
            screen.command_palette_handles_input(&None, text.as_bytes(), false, CLIENT);
            expected.text = text.into();
        }
        // Up wraps from Search to Apply; the first iteration applies without edits.
        palette_key(&mut screen, BareKey::Up);
        palette_key(&mut screen, BareKey::Enter);
        let request = next();
        let AgentAction::History(HistoryAction::Search(query)) = &request.action else {
            panic!("expected archive search")
        };
        assert_eq!(query, &expected);
        screen.plugin_palette_instruction(reply(&request));
    }
}

#[test]
fn modal_overlay_placement_keeps_its_tab_and_only_focuses_its_requester() {
    use yinz_types::data::OverlayPlacement;
    for switched in [false, true] {
        let mut screen = screen();
        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        let destination = TabId::new(0);
        let original = screen.tabs[&destination].get_active_pane_id(other);
        if switched {
            new_tab(&mut screen, 2, TabId::new(1));
        }
        let requester_tab = screen.active_tab_ids[&CLIENT];
        let requester_focus = screen.tabs[&requester_tab].get_active_pane_id(CLIENT);
        let editor = PaneId::Terminal(TerminalId::new(9));
        screen
            .place_new_pane(
                editor,
                Some("Editor".into()),
                None,
                None,
                NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
                false,
                ClientTabIndexOrPaneId::TabIndexForClient(destination, CLIENT),
                None,
                false,
                &mut super::super::thread_main::PendingEvents::default(),
            )
            .unwrap();
        assert!(screen.tabs[&destination].has_pane_with_pid(&editor));
        assert_eq!(screen.active_tab_ids[&CLIENT], requester_tab);
        assert_eq!(screen.active_tab_ids[&other], destination);
        assert_eq!(
            screen.tabs[&destination].get_active_pane_id(other),
            original
        );
        assert_eq!(
            screen.tabs[&requester_tab].get_active_pane_id(CLIENT),
            if switched {
                requester_focus
            } else {
                Some(editor)
            }
        );
    }
}

#[test]
fn palette_rename_keeps_raw_title() {
    let mut screen = screen();
    let pane = PaneId::Terminal(TerminalId::new(1));
    screen.with_pane(pane, |pane| {
        pane.set_title_prefix(Some("project:branch".into()));
        pane.update_name("shell");
    });
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let actions = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Rename pane");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        enqueued_palette_action(&actions),
        Action::RenamePaneByPaneId {
            pane_id: Some(pane),
            name: b"shell".to_vec()
        }
    );

    palette_search(&mut screen, "Close pane");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        frame.contains("Close [1] project:branch: shell?"),
        "{frame}"
    );
}

#[test]
fn warp_palette_uses_peer_display_titles_and_falls_back_for_old_metadata() {
    for display_title in [None, Some("host project:branch: shell".to_owned())] {
        let mut screen = screen();
        let source = warp_peer(&mut screen, "warp-peer");
        let peer = screen.peer_sessions_cache.get_mut(&source).unwrap();
        for pane in peer.panes.panes.values_mut().flatten() {
            pane.title = "shell".into();
            pane.display_title = display_title.clone();
        }
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        palette_search(&mut screen, "Warp pane");
        palette_key(&mut screen, BareKey::Enter);
        palette_key(&mut screen, BareKey::Enter);
        screen.command_palette_handles_input(&None, b"terminal_1", false, CLIENT);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let title = display_title.as_deref().unwrap_or("shell");
        assert!(frame.contains(&format!("› [1] {title}")), "{frame}");
    }
}

fn theme_screen() -> Screen {
    let mut screen = screen();
    let mut alpha = screen.style.colors;
    alpha.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((20, 30, 40));
    let mut beta = alpha;
    beta.text_unselected.background = yinz_types::colors::PaletteColor::Rgb((220, 230, 240));
    screen.themes =
        std::collections::BTreeMap::from([("alpha".into(), alpha), ("beta".into(), beta)]);
    screen.terminal_themes.insert(
        "alpha".into(),
        yinz_types::style::TerminalTheme {
            foreground: (1, 2, 3),
            background: (4, 5, 6),
            ansi: [(7, 8, 9); 16],
        },
    );
    screen.theme_name = Some("beta".into());
    screen.repaint_theme(beta).unwrap();
    screen
}

fn open_themes(screen: &mut Screen) {
    palette_search(screen, "Change theme");
    palette_key(screen, BareKey::Enter);
}

#[test]
fn native_theme_form_preserves_literal_name_and_saves_raw_value() {
    let mut screen = theme_screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let name = "**theme**_[name]";
    screen.themes = std::collections::BTreeMap::from([(name.into(), screen.style.colors)]);
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains(&format!("Apply theme {name}")), "{frame}");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ServerInstruction::SavePaletteTheme { name: saved, .. } if saved == name
    )));
}

#[test]
fn theme_preview_initial_selection_filter_empty_and_cancel() {
    let mut screen = theme_screen();
    let receiver = screen_wired_to_server(&mut screen);
    let original = screen.style.colors;
    assert_eq!(screen.style.terminal, None);
    open_themes(&mut screen);
    assert_eq!(screen.style.colors, original);
    palette_key(&mut screen, BareKey::Up);
    assert_eq!(screen.style.terminal, Some(screen.terminal_themes["alpha"]));
    assert_eq!(screen.style.colors, screen.themes["alpha"]);
    assert_eq!(
        screen.default_mode_info.style.colors,
        screen.themes["alpha"]
    );
    screen.command_palette_handles_input(&None, b"missing", false, CLIENT);
    assert_eq!(screen.style.colors, original);
    palette_key(&mut screen, BareKey::Esc);
    assert!(screen.theme_preview.is_none());
    assert_eq!(screen.style.terminal, None);
    assert_eq!(screen.style.colors, original);
    assert!(!receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::SavePaletteTheme { .. })));
}

#[test]
fn theme_preview_excludes_other_clients_and_restores_latest_host_report() {
    use yinz_types::style::HostTerminalThemeMode;
    let mut screen = theme_screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.host_theme_dark_styling = Some(screen.themes["alpha"]);
    screen.host_theme_light_styling = Some(screen.themes["beta"]);
    screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Light)
        .unwrap();
    open_themes(&mut screen);
    screen.toggle_command_palette(other);
    screen.command_palette_handles_input(&None, b"Change theme", false, other);
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        other,
    );
    let snapshot = render_frame_into(&mut screen, &receiver, other, &mut grid);
    assert!(
        snapshot.contains("Theme picker is open in another client"),
        "{snapshot}"
    );
    assert_eq!(screen.theme_preview.as_ref().unwrap().client, CLIENT);
    screen
        .update_host_terminal_theme_mode(HostTerminalThemeMode::Dark)
        .unwrap();
    assert_eq!(screen.style.colors, screen.themes["beta"]);
    screen.close_command_palette(CLIENT);
    assert_eq!(screen.style.colors, screen.themes["alpha"]);
    assert_eq!(
        screen.mode_info[&other].style.colors,
        screen.themes["alpha"]
    );
}

#[test]
fn theme_preview_disconnect_reload_and_failed_save() {
    let mut screen = theme_screen();
    let receiver = screen_wired_to_server(&mut screen);
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Up);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Right);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    let request = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ServerInstruction::SavePaletteTheme { request, name, .. } => {
                assert_eq!(name, "alpha");
                Some(request)
            },
            _ => None,
        })
        .unwrap();
    screen.palette_theme_saved(CLIENT, uuid::Uuid::new_v4(), Ok(()));
    assert!(screen.theme_preview.is_some());
    screen.palette_theme_saved(CLIENT, request, Err("disk is read-only".into()));
    let mut grid = grid_for_render_snapshot(SIZE);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("disk is read-only"), "{snapshot}");
    palette_key(&mut screen, BareKey::Enter);
    let retry = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ServerInstruction::SavePaletteTheme { request, .. } => Some(request),
            _ => None,
        })
        .unwrap();
    assert_ne!(retry, request);
    screen.palette_theme_saved(CLIENT, retry, Err("still read-only".into()));
    assert_eq!(screen.style.colors, screen.themes["beta"]);
    assert_eq!(screen.style.terminal, None);
    palette_key(&mut screen, BareKey::Esc);
    assert_eq!(screen.style.colors, screen.themes["beta"]);
    screen.close_command_palette(CLIENT);
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Up);
    let mut config = test_screen_config();
    config.theme = Styling::default();
    screen
        .reconfigure(config, Keybinds::default(), CLIENT)
        .unwrap();
    assert!(screen.theme_preview.is_none());
    assert_eq!(screen.style.colors, Styling::default());
    screen.themes.insert("one".into(), Styling::default());
    open_themes(&mut screen);
    screen.remove_client(CLIENT).unwrap();
    assert!(screen.theme_preview.is_none());
}

#[test]
fn theme_preview_mouse_selection_toggle_and_outside_click_restore() {
    for outside_click in [false, true] {
        let mut screen = theme_screen();
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        open_themes(&mut screen);
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let row = snapshot
            .lines()
            .position(|line| line.contains("alpha"))
            .unwrap();
        let col = snapshot.lines().nth(row).unwrap().find("alpha").unwrap();
        screen.command_palette_handles_mouse(
            &MouseEvent::new_left_press_event(Position::new(row as i32, col as u16)),
            CLIENT,
        );
        assert_eq!(screen.style.colors, screen.themes["alpha"]);
        if outside_click {
            screen.command_palette_handles_mouse(
                &MouseEvent::new_left_press_event(Position::new(0, 0)),
                CLIENT,
            );
        } else {
            screen.toggle_command_palette(CLIENT);
        }
        assert!(screen.theme_preview.is_none());
        assert_eq!(screen.style.colors, screen.themes["beta"]);
    }
}

#[test]
fn theme_preview_cancel_invalidates_queued_save() {
    let mut screen = theme_screen();
    let receiver = screen_wired_to_server(&mut screen);
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Up);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    let cancelled = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ServerInstruction::SavePaletteTheme { cancelled, .. } => Some(cancelled),
            _ => None,
        })
        .unwrap();
    assert!(!cancelled.load(std::sync::atomic::Ordering::Acquire));
    palette_key(&mut screen, BareKey::Esc);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(screen.style.colors, screen.themes["beta"]);
}

fn history_run(
    rows: Vec<yinz_bus::command_palette::HistoryRow>,
) -> yinz_bus::command_palette::HistoryReply {
    use yinz_bus::command_palette::{HistoryAction, HistoryEvent, HistoryHeader, HistoryReply};
    let events = rows
        .iter()
        .filter_map(|row| match row.action {
            HistoryAction::Event { revision, .. } => Some(HistoryEvent {
                revision,
                revisions: vec![revision],
                tool_calls: 0,
                created_at_ms: None,
                kind: "Message".into(),
                time: String::new(),
                markdown: row.label.clone(),
                truncated: false,
            }),
            _ => None,
        })
        .collect();
    HistoryReply::Run {
        header: HistoryHeader {
            name: "Recorded run".into(),
            ..Default::default()
        },
        events,
        rows,
        status: String::new(),
    }
}

#[test]
fn history_backfill_merges_pages_and_polling_preserves_manual_scroll() {
    use yinz_bus::command_palette::{
        AgentAction, AgentReply, HistoryAction, HistoryRow, Instruction,
    };
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    let mut screen = screen();
    screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
        PaneId::Terminal(TerminalId::new(1)),
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: Some("session".into()),
            run_id: Some("run".into()),
        }),
    );
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || loop {
        let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let PluginInstruction::PaletteAgent(request) = instruction {
            if matches!(
                request.action,
                yinz_bus::command_palette::AgentAction::Usage { .. }
            ) {
                continue;
            }
            break *request;
        }
    };
    let (tick_tx, tick_rx) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(tick_tx));
    palette_search(&mut screen, "provider:codex");
    palette_key(&mut screen, BareKey::Enter);
    let initial = next();
    let reply =
        |request: &yinz_bus::command_palette::AgentRequest, start: u64| Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::History(history_run(
                (start..start + 30)
                    .map(|revision| HistoryRow {
                        id: format!("event:{revision}"),
                        label: format!("Event {revision}"),
                        description: String::new(),
                        action: HistoryAction::Event {
                            id: "run".into(),
                            revision,
                        },
                    })
                    .collect(),
            ))),
        };
    let mut first = reply(&initial, 50);
    if let Instruction::AgentReply {
        reply: Ok(AgentReply::History(yinz_bus::command_palette::HistoryReply::Run { rows, .. })),
        ..
    } = &mut first
    {
        rows.push(HistoryRow {
            id: "continue".into(),
            label: String::new(),
            description: String::new(),
            action: HistoryAction::Run {
                id: "run".into(),
                from: Some(1),
                query: String::new(),
            },
        });
    }
    screen.plugin_palette_instruction(first);
    let backfill = next();
    assert!(matches!(
        backfill.action,
        AgentAction::History(HistoryAction::Run { from: Some(1), .. })
    ));
    palette_key(&mut screen, BareKey::Tab);
    palette_key(&mut screen, BareKey::Up);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    screen.plugin_palette_instruction(reply(&backfill, 1));
    assert_eq!(
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        before
    );
    let view = loop {
        let (instruction, _) = tick_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let ScreenInstruction::CommandPalette(Instruction::HistoryTick { view, .. }) =
            instruction
        {
            break view;
        }
    };
    screen.plugin_palette_instruction(Instruction::HistoryTick {
        client: CLIENT,
        opening: initial.opening,
        view,
        generation: 2,
    });
    let latest = next();
    assert!(matches!(
        latest.action,
        AgentAction::History(HistoryAction::Run { from: Some(80), .. })
    ));
    screen.plugin_palette_instruction(reply(&latest, 80));
    assert_eq!(
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
        before
    );
    palette_key(&mut screen, BareKey::End);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Event 109"), "{frame}");
    drop(tick_rx);
}

#[test]
fn palette_completion_notifications_are_scoped_and_readable() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Delivery, Instruction, Owner};
    use yinz_types::command_palette::{Action, Command, Message, Reply, View};
    for early_close in [false, true] {
        for kind in [
            "default",
            "custom",
            "view",
            "error",
            "cancel",
            "reload",
            "disconnect",
            "form",
            "form-error",
            "nested",
            "nested-error",
            "invalid",
            "oversized",
        ] {
            let mut screen = screen();
            let notifications = palette_actions_receiver(&mut screen);
            let (sender, requests) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
            let owner = Owner {
                plugin_id: PluginId::new(50),
                generation: 9,
                client_id: Some(CLIENT),
            };
            let command = |command| Instruction::Command {
                owner: owner.clone(),
                client: CLIENT,
                unavailable: Arc::new(AtomicBool::new(false)),
                command,
            };
            screen.plugin_palette_instruction(command(Command::Register {
                id: "test".into(),
                title: "Test".into(),
                registration: "1".into(),
                static_actions: true,
            }));
            screen.toggle_command_palette(CLIENT);
            let next_scope = || {
                requests
                    .try_iter()
                    .find_map(|(request, _)| match request {
                        PluginInstruction::PluginPalette(Delivery {
                            message:
                                Message::Discover { scope, .. } | Message::Execute { scope, .. },
                            ..
                        }) => Some(scope),
                        _ => None,
                    })
                    .unwrap()
            };
            screen.plugin_palette_instruction(command(Command::Reply {
                scope: next_scope(),
                result: Ok(Reply::Actions {
                    actions: vec![Action {
                        id: "save".into(),
                        label: "**Save** report\\-file".into(),
                        description: String::new(),
                        aliases: vec![],
                        handle: "save".into(),
                        cells: None,
                    }],
                }),
            }));
            assert!(notifications.try_recv().is_err(), "discovery is quiet");
            screen.command_palette_handles_input(&None, b"Save report-file", false, CLIENT);
            palette_key(&mut screen, BareKey::Enter);
            let mut scope = next_scope();
            if matches!(kind, "form" | "form-error") {
                screen.plugin_palette_instruction(command(Command::Reply {
                    scope,
                    result: Ok(Reply::View {
                        view: View::Form {
                            description: String::new(),
                            actions: vec![],
                            confirmation: false,
                            title: "**Edit** report\\-file".into(),
                            fields: vec![],
                            submit_label: "Save".into(),
                            handle: "submit".into(),
                        },
                    }),
                }));
                assert!(notifications.try_recv().is_err());
                palette_key(&mut screen, BareKey::Enter);
                scope = next_scope();
            }
            if matches!(kind, "nested" | "nested-error") {
                screen.plugin_palette_instruction(command(Command::Reply {
                    scope,
                    result: Ok(Reply::View {
                        view: View::Actions {
                            columns: vec![],
                            title: "Report".into(),
                            description: String::new(),
                            actions: vec![Action {
                                id: "export".into(),
                                label: "**Export** report\\-file".into(),
                                description: String::new(),
                                aliases: vec![],
                                handle: "export".into(),
                                cells: None,
                            }],
                            source: None,
                            initial_query: String::new(),
                        },
                    }),
                }));
                assert!(notifications.try_recv().is_err());
                palette_key(&mut screen, BareKey::Enter);
                scope = next_scope();
            }
            if early_close {
                screen.plugin_palette_instruction(command(Command::Close {
                    scope: scope.clone(),
                }));
                assert!(
                    notifications.try_recv().is_err(),
                    "early close waits for settlement"
                );
            }
            match kind {
                "cancel" if !early_close => {
                    screen.close_command_palette(CLIENT);
                },
                "reload" => {
                    screen.plugin_palette_instruction(Instruction::Dispose {
                        plugin_id: owner.plugin_id,
                        generation: owner.generation,
                    });
                },
                "disconnect" => {
                    screen.close_command_palette(CLIENT);
                    screen.disconnect_palette_client(CLIENT);
                },
                _ => {},
            }
            let result = match kind {
                "custom" => Ok(Reply::Complete {
                    notification: "Report saved".into(),
                    view: Some(View::Text {
                        title: "Report".into(),
                        text: "Contents".into(),
                    }),
                }),
                "view" => Ok(Reply::View {
                    view: View::Text {
                        title: "Report".into(),
                        text: "Contents".into(),
                    },
                }),
                "error" | "form-error" | "nested-error" => Err("Disk full".into()),
                "invalid" => Ok(Reply::Complete {
                    notification: " ".into(),
                    view: None,
                }),
                "oversized" => Ok(Reply::Complete {
                    notification: "x".repeat(yinz_types::command_palette::MAX_DISPLAY_BYTES),
                    view: None,
                }),
                _ => Ok(Reply::Close),
            };
            let reply = command(Command::Reply { scope, result });
            screen.plugin_palette_instruction(reply.clone());
            let messages: Vec<_> = notifications
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    ScreenInstruction::Notify(message, pane, ..) => {
                        assert_eq!(pane, Some(PaneId::Plugin(owner.plugin_id)));
                        Some(message)
                    },
                    _ => None,
                })
                .collect();
            let expected = match kind {
                "reload" | "disconnect" => None,
                "cancel" if !early_close => None,
                "view" if !early_close => None,
                "custom" => Some("Report saved"),
                "form" => Some("Completed: Edit report-file"),
                "form-error" => Some("Could not edit report-file: Disk full"),
                "nested" => Some("Completed: Export report-file"),
                "nested-error" => Some("Could not export report-file: Disk full"),
                "invalid" => Some("Could not save report-file: Palette labels and IDs must be nonempty text without controls"),
                "oversized" => Some("Could not save report-file: Palette display data exceeds 256 KiB"),
                "error" => Some("Could not save report-file: Disk full"),
                _ => Some("Completed: Save report-file"),
            };
            assert_eq!(
                messages,
                expected.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                "{kind}, early close {early_close}"
            );
            screen.plugin_palette_instruction(reply);
            assert!(
                notifications.try_recv().is_err(),
                "duplicate reply is quiet"
            );
        }
    }
}

#[test]
fn palette_cursor_hides_under_toasts_and_returns_when_uncovered() {
    use crate::screen::Notification;
    use yinz_types::pane_size::Viewport;
    use yinz_types::{animation::Tween, input::options::NotificationPosition};

    for dismiss in [false, true] {
        for watching in [false, true] {
            let mut screen = screen();
            let watcher = ClientId::new(2);
            screen.add_watcher_client(watcher).unwrap();
            screen.set_watcher_size(watcher, SIZE);
            screen.set_followed_client(CLIENT).unwrap();
            let client = if watching { watcher } else { CLIENT };
            let receiver = screen_wired_to_server(&mut screen);
            let mut grid = grid_for_render_snapshot(SIZE);
            let now = std::time::Instant::now();
            screen.pinned_now = Some(now);
            screen.toggle_command_palette(CLIENT);
            let (cursor, _) =
                render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
            let (x, y) = cursor.expect("palette cursor is visible");

            let id = NotificationId::new(99);
            let mut notification =
                Notification::new(id, "Toast covering palette cursor".into(), None, None, now);
            notification.reveal = Tween::settled(1.0);
            screen.notification_position = NotificationPosition::TopLeft;
            screen.notifications.push(notification);
            let surface = Viewport {
                x: 0,
                y: 0,
                cols: SIZE.cols,
                rows: SIZE.rows,
            };
            let toast_y = screen.notification_layout(surface, now, ClientId::new(1))[0].y;
            screen.notifications[0].y_offset = Tween::settled(y as f32 - toast_y as f32);
            assert!(screen
                .notification_cell_rects(surface, now, ClientId::new(1))
                .iter()
                .any(|rect| rect.contains(x, y)));
            let (covered_cursor, _) =
                render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
            assert_eq!(covered_cursor, if watching { cursor } else { None });

            if dismiss {
                screen.dismiss_notification(id, now).unwrap();
                screen.pinned_now = Some(now + Duration::from_secs(1));
            } else {
                screen.notifications[0].y_offset = Tween::settled((y + 2) as f32 - toast_y as f32);
            }
            let (restored_cursor, _) =
                render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
            assert_eq!(restored_cursor, cursor);
        }
    }
}

#[test]
fn palette_popup_is_visible_preserves_cursor_and_restores_covered_content() {
    use crate::screen::Notification;
    use yinz_types::{animation::Tween, internal_ids::NotificationId};
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    let (cursor, before) = render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    let now = std::time::Instant::now();
    screen.pinned_now = Some(now);
    let mut notification = Notification::new(
        NotificationId::new(99),
        "Report successfully saved above palette".into(),
        None,
        None,
        now,
    );
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(8.0);
    screen.notification_position = yinz_types::input::options::NotificationPosition::TopRight;
    screen.notifications.push(notification);
    let (after_cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Report successfully saved above palette"),
        "{snapshot}"
    );
    assert_eq!(after_cursor, cursor);
    let line = snapshot
        .lines()
        .position(|line| line.contains("Report successfully saved above palette"))
        .unwrap();
    let col = snapshot
        .lines()
        .nth(line)
        .unwrap()
        .find("Report successfully saved above palette")
        .unwrap();
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(line as i32, col as u16)),
        CLIENT,
    );
    assert!(screen.command_palettes.contains_key(&CLIENT));
    screen
        .dismiss_notification(NotificationId::new(99), now)
        .unwrap();
    screen.pinned_now = Some(now + Duration::from_secs(1));
    let (after_cursor, after) =
        render_frame_and_cursor_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(after, before);
    assert_eq!(after_cursor, cursor);
}

#[test]
fn palette_moves_without_another_tab_report_a_noop() {
    let mut screen = screen();
    let notifications = palette_actions_receiver(&mut screen);
    screen
        .move_command_palette_pane(
            PaneId::Terminal(TerminalId::new(1)),
            TabId::new(0),
            Some(Direction::Right),
            CLIENT,
        )
        .unwrap();
    assert!(
        matches!(notifications.try_recv().unwrap().0, ScreenInstruction::Notify(message, ..) if message == "Pane was not moved: no other tab")
    );
    screen.dispatch_command_palette_action(
        PaneId::Terminal(TerminalId::new(1)),
        TabId::new(0),
        Action::MoveTabByTabId {
            id: TabId::new(0),
            direction: Direction::Right,
        },
        CLIENT,
        "Move tab right".into(),
    );
    assert!(
        matches!(notifications.try_recv().unwrap().0, ScreenInstruction::Notify(message, ..) if message == "Tab was not moved: no other tab")
    );
}

#[test]
fn palette_nonzero_completion_without_message_does_not_focus_or_report_success() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    let pane = PaneId::Terminal(TerminalId::new(1));
    screen.dispatch_command_palette_action(
        pane,
        TabId::new(0),
        Action::TogglePaneEmbedOrFloatingByPaneId { pane_id: pane },
        CLIENT,
        "Float/embed pane".into(),
    );
    match receiver.recv_timeout(Duration::from_secs(5)).unwrap().0 {
        ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(_, mut completion) => {
            completion.as_mut().unwrap().set_exit_status(1);
            drop(completion);
        },
        other => panic!("unexpected instruction: {other:?}"),
    }
    assert!(
        matches!(receiver.recv_timeout(Duration::from_secs(5)).unwrap().0,
        ScreenInstruction::Notify(message, ..) if message.starts_with("Could not float/embed pane:"))
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn suspended_palette_keeps_notifications_hidden_and_releases_mode_only_on_close() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply};

    for resume in [false, true] {
        let mut screen = screen();
        let receiver = screen_wired_to_server(&mut screen);
        let (sender, requests) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        let command = |command| Instruction::Command {
            owner: Owner {
                plugin_id: PluginId::new(50),
                generation: 9,
                client_id: Some(CLIENT),
            },
            client: CLIENT,
            unavailable: Arc::new(AtomicBool::new(false)),
            command,
        };
        let next = || {
            requests
                .try_iter()
                .find_map(|(instruction, _)| match instruction {
                    PluginInstruction::PluginPalette(delivery) => match delivery.message {
                        Message::Discover { scope, .. } | Message::Execute { scope, .. } => {
                            Some(scope)
                        },
                        _ => None,
                    },
                    _ => None,
                })
                .expect("palette request")
        };
        screen
            .change_mode(InputMode::Notifications, None, CLIENT)
            .unwrap();
        screen.plugin_palette_instruction(command(Command::Register {
            id: "test".into(),
            title: "Test".into(),
            registration: "1".into(),
            static_actions: true,
        }));
        screen.toggle_command_palette(CLIENT);
        screen.plugin_palette_instruction(command(Command::Reply {
            scope: next(),
            result: Ok(Reply::Actions {
                actions: vec![PaletteAction {
                    id: "browse".into(),
                    label: "Unique browse action".into(),
                    handle: "browse".into(),
                    description: String::new(),
                    aliases: vec![],
                    cells: None,
                }],
            }),
        }));
        screen.command_palette_handles_input(&None, b"Unique browse action", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        let scope = next();
        assert!(screen.plugin_palette_instruction(command(Command::Suspend {
            scope: scope.clone(),
            suspended: true,
        })));
        assert!(!screen.palette_owns_input(CLIENT));
        assert!(!screen.notification_browser_is_visible(CLIENT));
        for mode in [InputMode::Normal, InputMode::Notifications] {
            screen.change_mode(mode, None, CLIENT).unwrap();
            screen.change_mode_for_all_clients(mode, None).unwrap();
            assert_eq!(screen.mode_info[&CLIENT].mode, InputMode::Locked);
            assert!(screen.notification_browsers.contains_key(&CLIENT));
            assert!(!screen.notification_browser_is_visible(CLIENT));
        }
        for key in [
            BareKey::Up,
            BareKey::Down,
            BareKey::Enter,
            BareKey::Char(' '),
        ] {
            assert!(!screen.command_palette_handles_input(
                &Some(KeyWithModifier::new(key)),
                b"picker input",
                false,
                CLIENT,
            ));
        }
        if resume {
            assert!(screen.plugin_palette_instruction(command(Command::Suspend {
                scope: scope.clone(),
                suspended: false,
            })));
            assert!(screen.palette_owns_input(CLIENT));
            assert!(!screen.notification_browser_is_visible(CLIENT));
        }
        assert!(!receiver.try_iter().any(|(instruction, _)| matches!(
            instruction,
            ServerInstruction::SetCommandPaletteOpen(CLIENT, false)
        )));
        screen.close_command_palette(CLIENT);
        assert_eq!(
            receiver
                .try_iter()
                .filter(|(instruction, _)| matches!(
                    instruction,
                    ServerInstruction::SetCommandPaletteOpen(CLIENT, false)
                ))
                .count(),
            1
        );
        screen
            .change_mode(InputMode::Notifications, None, CLIENT)
            .unwrap();
        assert!(screen.notification_browser_is_visible(CLIENT));
        assert!(
            !screen.plugin_palette_instruction(command(Command::Suspend {
                scope,
                suspended: false,
            }))
        );
    }
}

#[test]
fn dynamic_palette_refresh_recovers_after_execution_error_and_stops_after_close() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply, View};
    let mut screen = screen();
    let ticks = palette_actions_receiver(&mut screen);
    let (sender, requests) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let command = |command| Instruction::Command {
        owner: Owner {
            plugin_id: PluginId::new(50),
            generation: 9,
            client_id: Some(CLIENT),
        },
        client: CLIENT,
        unavailable: Arc::new(AtomicBool::new(false)),
        command,
    };
    let next = || {
        requests
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PluginPalette(delivery) => match delivery.message {
                    message @ (Message::Discover { .. } | Message::Execute { .. }) => Some(message),
                    _ => None,
                },
                _ => None,
            })
            .expect("palette request")
    };
    let action = |id: &str| PaletteAction {
        id: id.into(),
        label: id.into(),
        handle: id.into(),
        description: String::new(),
        aliases: vec![],
        cells: None,
    };
    screen.plugin_palette_instruction(command(Command::Register {
        id: "test".into(),
        title: "Test".into(),
        registration: "1".into(),
        static_actions: true,
    }));
    screen.toggle_command_palette(CLIENT);
    let Message::Discover { scope, .. } = next() else {
        panic!("discover")
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::Actions {
            actions: vec![action("unique-provider")],
        }),
    }));
    screen.command_palette_handles_input(&None, b"unique-provider", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, .. } = next() else {
        panic!("execute")
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::View {
            view: View::Actions {
                columns: vec![],
                description: String::new(),
                initial_query: String::new(),
                title: "Dynamic".into(),
                actions: vec![],
                source: Some("source".into()),
            },
        }),
    }));
    let Message::Discover { scope, .. } = next() else {
        panic!("discover")
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: scope.clone(),
        result: Ok(Reply::Actions {
            actions: vec![action("first"), action("second")],
        }),
    }));
    palette_key(&mut screen, BareKey::Down);
    assert!(screen.plugin_palette_instruction(Instruction::Refresh {
        client: CLIENT,
        scope: scope.clone()
    }));
    let Message::Discover {
        scope: refreshed,
        query,
        ..
    } = next()
    else {
        panic!("refresh")
    };
    assert_eq!(query, "");
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: refreshed.clone(),
        result: Ok(Reply::Actions {
            actions: vec![action("new"), action("first"), action("second")],
        }),
    }));
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute {
        handle,
        scope: execution,
        ..
    } = next()
    else {
        panic!("execute")
    };
    assert_eq!(handle, "second");
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: execution.clone(),
        result: Err("Destination disappeared".into()),
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (instruction, _) = ticks
            .recv_deadline(deadline)
            .expect("execution error must re-arm dynamic refresh");
        if let ScreenInstruction::CommandPalette(instruction @ Instruction::Refresh { .. }) =
            instruction
            && screen.plugin_palette_instruction(instruction)
        {
            break;
        }
    }
    let Message::Discover {
        scope: recovered,
        query,
        source,
        ..
    } = next()
    else {
        panic!("discovery after execution error")
    };
    assert!(recovered.revision > execution.revision);
    assert_eq!(query, "");
    assert_eq!(source.as_deref(), Some("source"));
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: recovered.clone(),
        result: Ok(Reply::Actions {
            actions: vec![action("first")],
        }),
    }));
    palette_key(&mut screen, BareKey::Esc);
    assert!(!screen.plugin_palette_instruction(Instruction::Refresh {
        client: CLIENT,
        scope: recovered.clone()
    }));
    screen.close_command_palette(CLIENT);
    assert!(!screen.plugin_palette_instruction(Instruction::Refresh {
        client: CLIENT,
        scope: recovered
    }));
    screen.toggle_command_palette(CLIENT);
    assert!(!screen.plugin_palette_instruction(Instruction::Refresh {
        client: CLIENT,
        scope
    }));
}

#[test]
fn palette_uses_displayed_tab_names_but_renames_the_stored_name() {
    let mut screen = screen();
    screen
        .tab_projects
        .insert(TabId::new(0), "yinz:topic".into());
    screen.log_and_report_session_state().unwrap();
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);

    palette_search(&mut screen, "Close tab");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(
        frame.contains("Close yinz:topic (tab 1, 1 panes)?"),
        "{frame}"
    );
    screen.close_command_palette(CLIENT);

    palette_search(&mut screen, "Rename tab");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Tab #1"), "{frame}");
    assert!(!frame.contains("yinz:topic"), "{frame}");
}

#[test]
fn palette_tab_group_actions_capture_identity_and_confirm_closure() {
    let mut screen = screen();
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "New tab group");
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(enqueued_palette_action(&receiver), Action::NewTabGroup);

    palette_search(&mut screen, "Rename tab group");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Esc);
    assert!(receiver.try_recv().is_err(), "cancel must not rename");
    screen.close_command_palette(CLIENT);

    palette_search(&mut screen, "Close tab group");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    assert!(
        receiver.try_recv().is_err(),
        "confirmation defaults to cancel"
    );
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Down);
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        enqueued_palette_action(&receiver),
        Action::CloseTabGroup { id: 0 }
    );
}

#[test]
fn palette_closes_groups_by_project_name_but_edits_the_fallback() {
    let mut screen = screen();
    screen
        .pane_projects
        .insert(PaneId::Terminal(TerminalId::new(1)), "project".into());
    screen.log_and_report_session_state().unwrap();
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "Close tab group");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(
        frame.contains("Close project (1 tabs, 1 panes)?"),
        "{frame}"
    );
    screen.close_command_palette(CLIENT);
    palette_search(&mut screen, "Rename tab group");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Default"), "{frame}");
}

#[test]
fn disabled_tab_groups_are_absent_from_the_command_palette() {
    let mut screen = screen();
    screen.default_mode_info.tab_groups = Some(false);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "tab group");
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("No matches"), "{frame}");
}

#[test]
fn plugin_markdown_survives_results_forms_and_text_view_transitions() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Delivery, Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Action, Command, Field, Message, Reply, View};
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let command = |command| Instruction::Command {
        owner: Owner {
            plugin_id: PluginId::new(50),
            generation: 9,
            client_id: Some(CLIENT),
        },
        client: CLIENT,
        unavailable: Arc::new(AtomicBool::new(false)),
        command,
    };
    screen.plugin_palette_instruction(command(Command::Register {
        id: "markdown".into(),
        title: "**Provider**".into(),
        registration: "1".into(),
        static_actions: true,
    }));
    screen.toggle_command_palette(CLIENT);
    let next_scope = || {
        requests
            .try_iter()
            .find_map(|(request, _)| match request {
                PluginInstruction::PluginPalette(Delivery {
                    message: Message::Discover { scope, .. } | Message::Execute { scope, .. },
                    ..
                }) => Some(scope),
                _ => None,
            })
            .unwrap()
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: next_scope(),
        result: Ok(Reply::Actions {
            actions: vec![Action {
                id: "edit".into(),
                label: "**Edit** report".into(),
                description: "Use `Markdown`".into(),
                aliases: vec![],
                handle: "edit".into(),
                cells: None,
            }],
        }),
    }));
    screen.command_palette_handles_input(&None, b"Edit report", false, CLIENT);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Edit report — Use Markdown"), "{frame}");
    assert!(!frame.contains("**Edit**"));
    palette_key(&mut screen, BareKey::Enter);
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: next_scope(),
        result: Ok(Reply::View {
            view: View::Form {
                title: "**Report form**".into(),
                description: "# Instructions\n\nUse **care**.".into(),
                fields: vec![Field::Text {
                    id: "value".into(),
                    label: "**Value**".into(),
                    initial_value: "**literal**".into(),
                }],
                submit_label: "**Save**".into(),
                handle: "submit".into(),
                actions: vec![],
                confirmation: false,
            },
        }),
    }));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Value: **literal**"), "{frame}");
    assert!(frame.contains("Report form"));
    assert!(!frame.contains("**Value**"));
    palette_key(&mut screen, BareKey::Tab);
    palette_key(&mut screen, BareKey::Enter);
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: next_scope(),
        result: Ok(Reply::View {
            view: View::Text {
                title: "**Saved**".into(),
                text: "# Report\n\n**Finished**\n\n- first item".into(),
            },
        }),
    }));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Finished"), "{frame}");
    assert!(frame.contains("first item"));
    assert!(!frame.contains("**Finished**"));
}

#[test]
fn native_project_names_remain_literal_in_markdown_results() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let name = "**project**_[name]";
    screen.projects.insert(name.into());
    palette_search(&mut screen, "Switch current project");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, name.as_bytes(), false, CLIENT);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains(name), "{frame}");
    assert!(frame.contains(&format!("› {name}")), "{frame}");
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        screen.client_projects[&CLIENT].current.as_deref(),
        Some(name)
    );
}

#[test]
fn project_picker_shows_literal_sources_and_current_marker() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.projects = ["alpha", "beta", "gamma"].map(String::from).into();
    screen.project_sources = [
        ("alpha".into(), "git@host:雪/[a].git".into()),
        ("beta".into(), "/home/*notes*".into()),
    ]
    .into();
    screen.reconcile_project_contexts();
    palette_search(&mut screen, "Switch current project");
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Current · git@host:雪/[a].git"), "{frame}");
    assert!(frame.contains("/home/*notes*"), "{frame}");
    assert!(frame.contains("gamma"), "{frame}");
    screen.project_sources.clear();
    screen.reconcile_project_contexts();
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Current"), "{frame}");
    assert!(!frame.contains("git@host"), "{frame}");
    screen
        .project_sources
        .insert("alpha".into(), "git@host:雪/[a].git".repeat(10));
    screen
        .resize_to_screen(Size { cols: 35, rows: 24 })
        .unwrap();
    grid.change_size(24, 35);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("alpha"), "{frame}");
    assert!(grid
        .as_character_lines()
        .iter()
        .all(|line| line.len() <= 35));
}

#[test]
fn peer_agent_quota_is_unavailable_without_local_usage_collection() {
    use yinz_bus::command_palette::{AgentAction, AgentReply, HistoryReply, Instruction};
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

    for conversation in [None, Some("peer-conversation")] {
        let mut screen = screen();
        screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
            PaneId::Terminal(TerminalId::new(1)),
            Some(AgentInfo {
                kind: "codex".into(),
                state: AgentState::Idle,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: conversation.map(str::to_owned),
                run_id: None,
            }),
        );
        warp_peer(&mut screen, "quota-peer");
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        palette_search(&mut screen, "provider:codex session:quota-peer");
        palette_key(&mut screen, BareKey::Enter);
        if conversation.is_some() {
            let request = loop {
                let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if let PluginInstruction::PaletteAgent(request) = instruction {
                    assert!(matches!(request.action, AgentAction::History(_)));
                    break request;
                }
            };
            screen.plugin_palette_instruction(Instruction::AgentReply {
                client: CLIENT,
                opening: request.opening,
                step: request.step,
                reply: Ok(AgentReply::History(HistoryReply::Run {
                    header: Default::default(),
                    events: vec![],
                    rows: vec![],
                    status: "No recorded history".into(),
                })),
            });
        }
        assert!(!rx.try_iter().any(|(instruction, _)| matches!(
            instruction,
            PluginInstruction::PaletteAgent(request)
                if matches!(request.action, AgentAction::Usage { .. })
        )));
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("Usage unavailable"), "{frame}");
        assert!(frame.contains("Quota unavailable…"), "{frame}");
        // The two-column header clips the explanation; the compact header has room for it.
        screen
            .resize_to_screen(Size { cols: 80, rows: 11 })
            .unwrap();
        grid.change_size(11, 80);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            frame.contains("Quota unavailable for peer-session agents"),
            "{frame}"
        );
    }
}

#[test]
fn agent_info_usage_and_prompt_are_scoped_to_the_selected_agent() {
    use yinz_bus::command_palette::{AgentAction, AgentReply, HistoryReply, Instruction};
    use yinz_types::agent::control::{
        AgentOutcome, AgentResponse, ProviderUsage, ProviderUsageStatus, ProviderUsageWindow,
    };
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    for conversation in [None, Some("selected-conversation")] {
        let mut screen = screen();
        let pane = PaneId::Terminal(TerminalId::new(1));
        screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
            pane,
            Some(AgentInfo {
                kind: "codex".into(),
                state: AgentState::Idle,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: conversation.map(str::to_owned),
                run_id: None,
            }),
        );
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        palette_search(&mut screen, "provider:codex");
        palette_key(&mut screen, BareKey::Enter);
        let usage_request = loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                if matches!(request.action, AgentAction::Usage { .. }) {
                    break request;
                }
                assert!(matches!(request.action, AgentAction::History(_)));
                // A pinned conversation loads its retained history before usage.
                screen.plugin_palette_instruction(Instruction::AgentReply {
                    client: CLIENT,
                    opening: request.opening,
                    step: request.step,
                    reply: Ok(AgentReply::History(HistoryReply::Run {
                        header: Default::default(),
                        events: vec![],
                        rows: vec![],
                        status: "No recorded history".into(),
                    })),
                });
            }
        };
        let view = usage_request.step;
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let usage = ProviderUsage {
            provider: "codex".into(),
            status: ProviderUsageStatus::Ok,
            observed_at_ms: None,
            plan: None,
            message: None,
            windows: vec![ProviderUsageWindow {
                label: "7d".into(),
                used_percent: 25.0,
                resets_at_ms: None,
            }],
        };
        screen.plugin_palette_instruction(Instruction::UsageReply {
            client: CLIENT,
            opening: usage_request.opening,
            view,
            usage,
        });
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("Weekly") && frame.contains("75%"), "{frame}");
        for key in [BareKey::Tab, BareKey::Tab] {
            palette_key(&mut screen, key);
        }
        screen.command_palette_handles_input(&None, b"continue\nwith tests", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        let prompt_request = loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                break request;
            }
        };
        assert!(
            matches!(&prompt_request.action, AgentAction::Prompt { pane_id, session_id, submission_id, target_session: None } if *pane_id == pane && session_id.as_deref() == conversation && !submission_id.is_empty())
        );
        assert_eq!(prompt_request.values, vec!["continue with tests"]);
        screen.plugin_palette_instruction(Instruction::AgentReply {
            client: CLIENT,
            opening: prompt_request.opening,
            step: prompt_request.step,
            reply: if conversation.is_some() {
                Ok(AgentReply::Prompted(AgentResponse::new(AgentOutcome::Ok)))
            } else {
                Err(
                    "Agent has no conversation ID; refresh agent info before sending a prompt"
                        .into(),
                )
            },
        });
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        if conversation.is_some() {
            assert!(!frame.contains("continue"), "{frame}");
            assert!(frame.contains("Agent info"), "{frame}");
            assert!(frame.contains("Weekly") && frame.contains("75%"), "{frame}");
        } else {
            assert!(frame.contains("Prompt:"), "{frame}");
            assert!(frame.contains("no conversation ID"), "{frame}");
            assert!(frame.contains("continue"), "{frame}");
            assert!(frame.contains("with tests"), "{frame}");
            // A provider-level rejection also retains the draft, and a later success clears it.
            for outcome in [AgentOutcome::Rejected, AgentOutcome::Ok] {
                palette_key(&mut screen, BareKey::Enter);
                let retry = loop {
                    let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    if let PluginInstruction::PaletteAgent(request) = instruction {
                        break request;
                    }
                };
                assert!(
                    matches!((&retry.action, &prompt_request.action), (AgentAction::Prompt { submission_id: retry, .. }, AgentAction::Prompt { submission_id: original, .. }) if retry == original)
                );
                assert_eq!(retry.values, vec!["continue with tests"]);
                screen.plugin_palette_instruction(Instruction::AgentReply {
                    client: CLIENT,
                    opening: retry.opening,
                    step: retry.step,
                    reply: Ok(AgentReply::Prompted(AgentResponse::new(outcome))),
                });
                let retried = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
                assert!(retried.contains("Agent info"), "{retried}");
                assert_eq!(
                    retried.contains("continue"),
                    outcome != AgentOutcome::Ok,
                    "{retried}"
                );
                if outcome == AgentOutcome::Rejected {
                    assert!(retried.contains("Rejected"), "{retried}");
                }
            }
        }
    }
}

#[test]
fn agent_prompt_retries_keep_identity_and_unsuccessful_submissions_restart_polling() {
    use std::sync::atomic::Ordering;
    use yinz_bus::command_palette::{AgentAction, AgentReply, AgentRequest, Instruction};
    use yinz_types::agent::control::{AgentOutcome, AgentResponse};
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

    for failure in ["error", "rejected", "cancelled", "dispatch"] {
        let mut screen = screen();
        screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
            PaneId::Terminal(TerminalId::new(1)),
            Some(AgentInfo {
                kind: "codex".into(),
                state: AgentState::Idle,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: Some("conversation".into()),
                run_id: Some("run".into()),
            }),
        );
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        let next = || loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction
                && !matches!(request.action, AgentAction::Usage { .. })
            {
                break *request;
            }
        };
        let (tx, ticks) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(tx));
        let next_tick = || loop {
            let (instruction, _) = ticks.recv_timeout(Duration::from_secs(5)).unwrap();
            if let ScreenInstruction::CommandPalette(tick @ Instruction::HistoryTick { .. }) =
                instruction
            {
                break tick;
            }
        };
        let reply = |request: &AgentRequest, reply| Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply,
        };
        let id = |request: &AgentRequest| {
            let AgentAction::Prompt { submission_id, .. } = &request.action else {
                panic!("expected prompt, got {:?}", request.action);
            };
            submission_id.clone()
        };
        palette_search(&mut screen, "provider:codex");
        palette_key(&mut screen, BareKey::Enter);
        screen.plugin_palette_instruction(reply(
            &next(),
            Ok(AgentReply::History(history_run(vec![]))),
        ));
        let old_tick = next_tick();
        screen.plugin_palette_instruction(old_tick.clone());
        let poll = next();
        assert!(matches!(poll.action, AgentAction::History(_)));
        for _ in 0..2 {
            palette_key(&mut screen, BareKey::Tab);
        }
        screen.command_palette_handles_input(&None, b"continue", false, CLIENT);
        let original = if failure == "dispatch" {
            let saved = screen.bus.senders.to_plugin.clone();
            let (tx, disconnected) = channels::unbounded();
            drop(disconnected);
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
            screen.bus.senders.should_silently_fail = false;
            palette_key(&mut screen, BareKey::Enter);
            screen.bus.senders.should_silently_fail = true;
            screen.bus.senders.to_plugin = saved;
            None
        } else {
            palette_key(&mut screen, BareKey::Enter);
            let request = next();
            if failure == "cancelled" {
                palette_key(&mut screen, BareKey::Esc);
                assert!(!request.active.load(Ordering::Acquire));
                // A late success after cancelling the wait must not clear the draft.
                screen.plugin_palette_instruction(reply(
                    &request,
                    Ok(AgentReply::Prompted(AgentResponse::new(AgentOutcome::Ok))),
                ));
            } else {
                let response = if failure == "error" {
                    Err("Uncertain peer reply".into())
                } else {
                    Ok(AgentReply::Prompted(AgentResponse::new(
                        AgentOutcome::Rejected,
                    )))
                };
                screen.plugin_palette_instruction(reply(&request, response));
            }
            Some(request)
        };
        assert!(!poll.active.load(Ordering::Acquire));
        screen
            .plugin_palette_instruction(reply(&poll, Ok(AgentReply::History(history_run(vec![])))));
        screen.plugin_palette_instruction(old_tick);
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(before.contains("continue"), "{failure}: {before}");
        let error = match failure {
            "error" => "Uncertain peer reply",
            "rejected" => "Rejected",
            "dispatch" => "Could not request agent action",
            _ => "",
        };
        assert!(before.contains(error), "{failure}: {before}");
        // Only the re-armed timer can restart history: the cancelled reply and
        // previously consumed tick above cannot schedule or start another poll.
        screen.plugin_palette_instruction(next_tick());
        let refreshed = next();
        assert!(matches!(refreshed.action, AgentAction::History(_)));
        screen.plugin_palette_instruction(reply(
            &refreshed,
            Ok(AgentReply::History(history_run(vec![]))),
        ));
        let after = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(
            after.contains("continue") && after.contains(error),
            "{failure}: {after}"
        );
        palette_key(&mut screen, BareKey::Enter);
        let retry = next();
        if let Some(original) = original {
            assert_eq!(id(&retry), id(&original));
        }
        assert_eq!(retry.values, vec!["continue"]);
        palette_key(&mut screen, BareKey::Esc);
        screen.command_palette_handles_input(&None, b" again", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        let edited = next();
        assert_ne!(id(&edited), id(&retry));
        assert_eq!(edited.values, vec!["continue again"]);
        screen.plugin_palette_instruction(reply(
            &edited,
            Ok(AgentReply::Prompted(AgentResponse::new(AgentOutcome::Ok))),
        ));
        let cleared = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(!cleared.contains("continue"), "{cleared}");
        screen.plugin_palette_instruction(reply(
            &next(),
            Ok(AgentReply::History(history_run(vec![]))),
        ));
        screen.command_palette_handles_input(&None, b"continue again", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        let fresh = next();
        assert_ne!(id(&fresh), id(&edited));
        assert_eq!(fresh.values, edited.values);
    }
}

#[test]
fn live_agent_order_uses_peer_timestamps_and_refreshes_root_and_picker() {
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    for picker in [false, true] {
        let mut screen = screen();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        screen.with_pane(pane_id, |pane| {
            pane.set_agent_info(Some(AgentInfo {
                kind: "codex".into(),
                state: AgentState::Busy,
                source: AgentSource::Reported,
                detail: Some("local task".into()),
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: None,
                run_id: None,
            }));
            pane.set_last_hook_at_ms(100);
        });
        screen.log_and_report_session_state().unwrap();
        let older = warp_peer(&mut screen, "older");
        let newer = warp_peer(&mut screen, "newer");
        for (name, stamp) in [(&older, Some(50)), (&newer, Some(200))] {
            for pane in screen
                .peer_sessions_cache
                .get_mut(name)
                .unwrap()
                .panes
                .panes
                .values_mut()
                .flatten()
            {
                pane.last_hook_at_ms = stamp;
                pane.agent.as_mut().unwrap().detail = Some(format!("{name} task"));
            }
        }
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        palette_search(
            &mut screen,
            if picker { "Agents" } else { "provider:codex" },
        );
        if picker {
            palette_key(&mut screen, BareKey::Enter);
        }
        let assert_order = |frame: &str, names: [&str; 3]| {
            let positions = names.map(|name| {
                frame
                    .find(name)
                    .unwrap_or_else(|| panic!("missing {name}:\n{frame}"))
            });
            assert!(
                positions.windows(2).all(|pair| pair[0] < pair[1]),
                "{frame}"
            );
        };
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_order(&frame, ["newer task", "local task", "older task"]);
        screen.with_pane(pane_id, |pane| pane.set_last_hook_at_ms(300));
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_order(&frame, ["local task", "newer task", "older task"]);
        // State wins over recency, and equal pane IDs are ordered by session on a tie.
        for name in [&older, &newer] {
            for pane in screen
                .peer_sessions_cache
                .get_mut(name)
                .unwrap()
                .panes
                .panes
                .values_mut()
                .flatten()
            {
                pane.last_hook_at_ms = None;
                let agent = pane.agent.as_mut().unwrap();
                agent.state = AgentState::Waiting;
                agent.waiting_on = agent.detail.clone();
            }
        }
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_order(&frame, ["newer task", "older task", "local task"]);
    }
}

#[test]
fn expanded_palette_quota_clips_notifications_and_releases_rasters_with_the_view() {
    use crate::screen::Notification;
    use yinz_bus::command_palette::{AgentAction, Instruction};
    use yinz_types::agent::control::{ProviderUsage, ProviderUsageStatus, ProviderUsageWindow};
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    use yinz_types::pane_size::{SizeInPixels, Viewport};
    use yinz_types::{
        animation::Tween, input::options::NotificationPosition, internal_ids::NotificationId,
    };
    let mut screen = screen();
    screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
        PaneId::Terminal(TerminalId::new(1)),
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
    );
    screen
        .kitty_host_capabilities
        .borrow_mut()
        .insert(CLIENT, true);
    screen.client_cell_pixel_sizes.insert(
        CLIENT,
        Some(SizeInPixels {
            width: 8,
            height: 18,
        }),
    );
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let open = |screen: &mut Screen| {
        palette_search(screen, "provider:codex");
        palette_key(screen, BareKey::Enter);
        let request = loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction
                && matches!(request.action, AgentAction::Usage { .. })
            {
                break request;
            }
        };
        screen.plugin_palette_instruction(Instruction::UsageReply {
            client: CLIENT,
            opening: request.opening,
            view: request.step,
            usage: ProviderUsage {
                provider: "codex".into(),
                status: ProviderUsageStatus::Ok,
                observed_at_ms: None,
                plan: None,
                message: None,
                windows: vec![ProviderUsageWindow {
                    label: "7d".into(),
                    used_percent: 75.0,
                    resets_at_ms: None,
                }],
            },
        });
    };
    open(&mut screen);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 1);
    let full = screen.client_kitty_host_state.borrow()[&CLIENT]
        .live_placements
        .values()
        .next()
        .unwrap()
        .geometry;
    assert_eq!((full.source_px_width, full.source_px_height), (144, 36));
    let now = std::time::Instant::now();
    screen.pinned_now = Some(now);
    let id = NotificationId::new(99);
    let mut notification = Notification::new(
        id,
        "Quota refresh finished; notification above the quota".into(),
        None,
        None,
        now,
    );
    notification.reveal = Tween::settled(1.0);
    screen.notifications.push(notification);
    screen.notification_position = NotificationPosition::TopRight;
    let surface = Viewport {
        x: 0,
        y: 0,
        cols: SIZE.cols,
        rows: SIZE.rows,
    };
    let settled_y = screen.notification_layout(surface, now, CLIENT)[0].y;
    for (y, visible_rows) in [
        (full.cell_y + 1, 1),
        (full.cell_y, 0),
        (full.cell_y + 3, 2),
        (full.cell_y, 0),
    ] {
        screen.notifications[0].y_offset = Tween::settled(y as f32 - settled_y as f32);
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        let visible_pixels: usize = screen.client_kitty_host_state.borrow()[&CLIENT]
            .live_placements
            .values()
            .filter(|record| record.geometry.internal_image_id == full.internal_image_id)
            .map(|record| record.geometry.source_px_width * record.geometry.source_px_height)
            .sum();
        assert_eq!(visible_pixels, 144 * 18 * visible_rows);
        assert_eq!(screen.kitty_image_store.borrow().image_count(), 1);
    }
    screen.dismiss_notification(id, now).unwrap();
    screen.pinned_now = Some(now + Duration::from_secs(1));
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    let restored: Vec<_> = screen.client_kitty_host_state.borrow()[&CLIENT]
        .live_placements
        .values()
        .map(|record| record.geometry)
        .collect();
    assert_eq!(restored, [full]);
    screen
        .resize_to_screen(Size { cols: 40, rows: 24 })
        .unwrap();
    grid.change_size(24, 40);
    let compact = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 0);
    assert!(compact.contains("Weekly · 25%"), "{compact}");
    screen.resize_to_screen(SIZE).unwrap();
    grid.change_size(SIZE.rows, SIZE.cols);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 1);
    palette_key(&mut screen, BareKey::Esc);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 0);
    screen.close_command_palette(CLIENT);
    open(&mut screen);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 1);
    screen.close_command_palette(CLIENT);
    assert_eq!(screen.kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn history_hit_merges_and_archived_backfill_resumes_after_reader() {
    use std::sync::atomic::Ordering;
    use yinz_bus::command_palette::{
        AgentAction, AgentReply, AgentRequest, HistoryAction, HistoryReply, HistoryRow, Instruction,
    };
    for query in [None, Some(""), Some("needle")] {
        let mut screen = screen();
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        let next = || loop {
            let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if let PluginInstruction::PaletteAgent(request) = instruction {
                break *request;
            }
        };
        let reply = |request: &AgentRequest, reply| Instruction::AgentReply {
            client: CLIENT,
            opening: request.opening,
            step: request.step,
            reply: Ok(AgentReply::History(reply)),
        };
        screen.toggle_command_palette(CLIENT);
        screen.open_palette_history(
            CLIENT,
            None,
            Some(match query {
                None => HistoryAction::Hit {
                    id: "run".into(),
                    revision: 7,
                },
                Some(query) => HistoryAction::Run {
                    id: "run".into(),
                    from: None,
                    query: query.into(),
                },
            }),
        );
        let initial = next();
        let event = |revision| HistoryRow {
            id: format!("event:{revision}"),
            label: format!("Event {revision}"),
            description: String::new(),
            action: HistoryAction::Event {
                id: "run".into(),
                revision,
            },
        };
        let continuation = HistoryAction::Run {
            id: "run".into(),
            from: Some(10),
            query: query.unwrap_or("").into(),
        };
        screen.plugin_palette_instruction(reply(
            &initial,
            history_run(vec![
                event(7),
                event(8),
                HistoryRow {
                    id: "continue".into(),
                    label: String::new(),
                    description: String::new(),
                    action: continuation.clone(),
                },
            ]),
        ));
        let pending = next();
        assert!(
            matches!(&pending.action, AgentAction::History(HistoryAction::Run { from: Some(10), query: actual, .. }) if actual == query.unwrap_or(""))
        );
        screen.open_palette_history(
            CLIENT,
            None,
            Some(HistoryAction::Event {
                id: "run".into(),
                revision: 7,
            }),
        );
        assert!(!pending.active.load(Ordering::Acquire));
        let reader = next();
        // An obsolete page must not replace the reader or consume its request.
        screen.plugin_palette_instruction(reply(&pending, history_run(vec![event(99)])));
        screen.plugin_palette_instruction(reply(
            &reader,
            HistoryReply::Reader {
                header: "Event".into(),
                markdown: "Reader".into(),
                tool: String::new(),
            },
        ));
        palette_key(&mut screen, BareKey::Esc);
        let resumed = next();
        assert!(
            matches!(&resumed.action, AgentAction::History(HistoryAction::Run { from: Some(10), query: actual, .. }) if actual == query.unwrap_or(""))
        );
        screen.plugin_palette_instruction(reply(&resumed, history_run(vec![event(8), event(10)])));
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(SIZE);
        let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(frame.contains("Event 7"), "{frame}");
        assert!(frame.contains("Event 10"), "{frame}");
        assert!(!frame.contains("Event 99"), "{frame}");
        assert!(!frame.contains("Loading history"), "{frame}");
        if query.is_none() {
            palette_key(&mut screen, BareKey::Tab);
            palette_key(&mut screen, BareKey::Enter);
            assert!(matches!(
                next().action,
                AgentAction::History(HistoryAction::Event { revision: 7, .. })
            ));
        }
    }
}

#[test]
fn history_hit_prefix_backfill_preserves_selection_and_backward_navigation() {
    use yinz_bus::command_palette::{
        AgentAction, AgentReply, AgentRequest, HistoryAction, HistoryRow, Instruction,
    };
    let mut screen = screen();
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let next = || loop {
        let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if let PluginInstruction::PaletteAgent(request) = instruction {
            break *request;
        }
    };
    let reply = |request: &AgentRequest, rows| Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: Ok(AgentReply::History(history_run(rows))),
    };
    let event = |revision| HistoryRow {
        id: format!("event:{revision}"),
        label: format!("Event {revision}"),
        description: String::new(),
        action: HistoryAction::Event {
            id: "run".into(),
            revision,
        },
    };
    screen.toggle_command_palette(CLIENT);
    screen.open_palette_history(
        CLIENT,
        None,
        Some(HistoryAction::Hit {
            id: "run".into(),
            revision: 100,
        }),
    );
    let initial = next();
    let mut rows: Vec<_> = (98..=102).map(event).collect();
    rows.push(HistoryRow {
        id: "continue".into(),
        label: String::new(),
        description: String::new(),
        action: HistoryAction::Run {
            id: "run".into(),
            from: Some(1),
            query: String::new(),
        },
    });
    screen.plugin_palette_instruction(reply(&initial, rows));
    let backfill = next();
    assert!(matches!(
        backfill.action,
        AgentAction::History(HistoryAction::Run { from: Some(1), .. })
    ));
    screen.plugin_palette_instruction(reply(&backfill, (1..=102).map(event).collect()));
    palette_key(&mut screen, BareKey::Tab);
    palette_key(&mut screen, BareKey::Enter);
    let selected = next();
    assert!(matches!(
        selected.action,
        AgentAction::History(HistoryAction::Event { revision: 100, .. })
    ));
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: selected.opening,
        step: selected.step,
        reply: Ok(AgentReply::History(
            yinz_bus::command_palette::HistoryReply::Reader {
                header: "Event".into(),
                markdown: "Event 100".into(),
                tool: String::new(),
            },
        )),
    });
    palette_key(&mut screen, BareKey::Esc);
    palette_key(&mut screen, BareKey::Home);
    palette_key(&mut screen, BareKey::Enter);
    assert!(matches!(
        next().action,
        AgentAction::History(HistoryAction::Event { revision: 1, .. })
    ));
}

#[test]
fn hook_flash_renders_matching_agent_in_both_views_and_clock_restores_dot() {
    use crate::screen::thread_main::apply_queued_agent_report;
    use yinz_types::agent::report::{AgentReport, AgentReportState};
    use yinz_types::agent::{AgentEvent, HookEvent};
    use yinz_types::animation::FRAME_PERIOD;
    let start = std::time::Instant::now();
    let (mut screen, jobs) = screen_with_background_jobs(SIZE);
    for colors in [
        &mut screen.style.colors.text_unselected,
        &mut screen.style.colors.text_selected,
    ] {
        colors.emphasis_0 = yinz_types::colors::PaletteColor::Rgb((17, 80, 190));
        colors.emphasis_2 = yinz_types::colors::PaletteColor::Rgb((40, 170, 50));
    }
    new_tab(&mut screen, 1, TabId::new(0));
    screen.pinned_now = Some(start);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let busy = yinz_types::agent::AgentInfo {
        kind: "codex".into(),
        state: yinz_types::agent::AgentState::Busy,
        source: yinz_types::agent::AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: None,
        session_id: None,
        run_id: None,
    };
    crate::screen::apply_pane_agent_report(&mut screen, pane, Some(busy)).unwrap();
    // A peer with the same pane ID must retain its ordinary dot.
    warp_peer(&mut screen, "peer");
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "provider:codex");
    let dots = |grid: &crate::panes::grid::Grid| {
        grid.as_character_lines()
            .into_iter()
            .flatten()
            .filter(|cell| cell.character == '●')
            .map(|cell| cell.styles.foreground)
            .collect::<Vec<_>>()
    };
    let send_hook = |screen: &mut Screen| {
        let hook = HookEvent {
            hook_event_name: "PreCompact".into(),
            ..Default::default()
        };
        apply_queued_agent_report(
            screen,
            &AgentReport {
                report_id: "flash".into(),
                process_id: ProcessId::new(1),
                pane_id: pane,
                kind: "codex".into(),
                state: AgentReportState::Unchanged,
                event: Some(AgentEvent::from_hook(
                    "codex",
                    TerminalId::new(1),
                    &hook,
                    serde_json::to_string(&hook).unwrap(),
                )),
                trace: false,
            },
            start,
        )
        .unwrap();
    };
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(dots(&grid).len(), 2);
    send_hook(&mut screen);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    let accent = Some(screen.style.colors.text_unselected.emphasis_0.into());
    let ordinary = Some(screen.style.colors.text_unselected.emphasis_2.into());
    assert_eq!(dots(&grid), vec![accent, ordinary]);
    assert_eq!(screen.animation_clock_period(start), Some(FRAME_PERIOD));
    drain_background_jobs(&jobs);
    let expired = start + Duration::from_millis(150);
    screen.pinned_now = Some(expired);
    screen.on_animation_tick(expired).unwrap();
    let sent = drain_background_jobs(&jobs);
    assert!(sent.contains(&BackgroundJob::RenderToClients));
    assert!(sent.contains(&BackgroundJob::StopAnimationClock));
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(dots(&grid), vec![ordinary, ordinary]);

    // The most recent local agent remains selected when its info view opens.
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(frame.contains("Agent info"), "{frame}");
    send_hook(&mut screen);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(dots(&grid), vec![accent]);
    assert_eq!(screen.animation_clock_period(expired), Some(FRAME_PERIOD));
    screen.pinned_now = Some(expired + Duration::from_millis(150));
    screen.on_animation_tick(screen.frame_now()).unwrap();
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(dots(&grid), vec![ordinary]);
    assert_eq!(screen.animation_clock_period(screen.frame_now()), None);

    send_hook(&mut screen);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        screen.animation_clock_period(screen.frame_now()),
        Some(FRAME_PERIOD)
    );
    screen.close_command_palette(CLIENT);
    screen.sync_animation_clock(screen.frame_now()).unwrap();
    // This fixture has no pane titles, so closing the palette removes the only flash surface.
    assert_eq!(screen.animation_clock_period(screen.frame_now()), None);
    palette_search(&mut screen, "provider:codex");
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        screen.animation_clock_period(screen.frame_now()),
        Some(FRAME_PERIOD)
    );
    screen.close_pane_explicitly(pane, None);
    screen.sync_animation_clock(screen.frame_now()).unwrap();
    assert_eq!(screen.animation_clock_period(screen.frame_now()), None);
}

#[test]
fn native_worktree_search_launches_typed_branch_once_without_a_form() {
    use yinz_bus::command_palette::{
        AgentAction, AgentChoice, AgentProject, AgentReply, Instruction,
    };
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    let mut screen = screen();
    let (tx, rx) = unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let enter = Some(KeyWithModifier::new(BareKey::Enter));
    let next = || {
        rx.try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PaletteAgent(request) => Some(*request),
                _ => None,
            })
            .unwrap()
    };
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"claude worktree", false, CLIENT);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let request = next();
    screen.plugin_palette_instruction(Instruction::AgentReply {
        client: CLIENT,
        opening: request.opening,
        step: request.step,
        reply: Ok(AgentReply::Choices {
            title: "project · Worktrees".into(),
            choices: vec![AgentChoice {
                label: "Create new worktree…".into(),
                description: String::new(),
                action: AgentAction::Checkout {
                    kind: "claude".into(),
                    project: AgentProject::Registered("project".into()),
                    path: None,
                },
            }],
        }),
    });
    screen.command_palette_handles_input(&None, b"feature/typed-branch", false, CLIENT);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    let launch = next();
    assert!(matches!(launch.action, AgentAction::Checkout {
        kind, project: AgentProject::Registered(project), path: None,
    } if kind == "claude" && project == "project"));
    assert_eq!(launch.values, ["feature/typed-branch"]);
    screen.command_palette_handles_input(&enter, b"", false, CLIENT);
    assert!(!rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
}

#[test]
fn open_agents_starts_empty_returns_to_root_and_is_client_local() {
    let mut screen = screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.toggle_command_palette(other);
    screen.command_palette_handles_input(&None, b"private query", false, other);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let other_before = render_frame_into(&mut screen, &receiver, other, &mut grid);

    screen.open_agents(CLIENT);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        frame.contains("Agents") && frame.contains("Agent filters…"),
        "{frame}"
    );
    screen.command_palette_handles_input(&None, b"stale search", false, CLIENT);
    screen.open_agents(CLIENT);
    let reopened = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        frame, reopened,
        "opening an existing picker clears its search"
    );
    assert_eq!(
        render_frame_into(&mut screen, &receiver, other, &mut grid),
        other_before
    );

    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        &[],
        false,
        CLIENT,
    );
    let root = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(root.contains("Do · Project:"), "{root}");
    assert!(screen.command_palette_is_open(CLIENT));
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Esc)),
        &[],
        false,
        CLIENT,
    );
    assert!(!screen.command_palette_is_open(CLIENT));
    assert!(screen.command_palette_is_open(other));
}

#[test]
fn open_agents_cancels_theme_preview_and_releases_other_clients() {
    let mut screen = theme_screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    let original_colors = screen.style.colors;
    let original_terminal = screen.style.terminal;
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Up);
    assert_ne!(screen.style.colors, original_colors);
    assert_ne!(screen.style.terminal, original_terminal);

    screen.open_agents(CLIENT);

    assert!(screen.theme_preview.is_none());
    assert_eq!(screen.style.colors, original_colors);
    assert_eq!(screen.style.terminal, original_terminal);
    assert_eq!(screen.default_mode_info.style.colors, original_colors);
    assert_eq!(screen.mode_info[&other].style.colors, original_colors);
    let mut grid = grid_for_render_snapshot(SIZE);
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        frame.contains("Agents") && frame.contains("Agent filters…"),
        "{frame}"
    );

    screen.toggle_command_palette(other);
    screen.command_palette_handles_input(&None, b"Change theme", false, other);
    screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(BareKey::Enter)),
        &[],
        false,
        other,
    );
    assert_eq!(screen.theme_preview.as_ref().unwrap().client, other);
}

#[test]
fn open_agents_preserves_another_clients_theme_preview() {
    let mut screen = theme_screen();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let receiver = screen_wired_to_server(&mut screen);
    open_themes(&mut screen);
    palette_key(&mut screen, BareKey::Up);
    let preview_colors = screen.style.colors;
    let preview_terminal = screen.style.terminal;

    screen.open_agents(other);

    assert_eq!(screen.theme_preview.as_ref().unwrap().client, CLIENT);
    assert_eq!(screen.style.colors, preview_colors);
    assert_eq!(screen.style.terminal, preview_terminal);
    let mut grid = grid_for_render_snapshot(SIZE);
    let frame = render_frame_into(&mut screen, &receiver, other, &mut grid);
    assert!(
        frame.contains("Agents") && frame.contains("Agent filters…"),
        "{frame}"
    );
}

#[test]
fn open_agents_respects_permission_modal() {
    use yinz_types::data::{PermissionType, PluginPermission};
    let mut screen = screen();
    screen.open_permission_modal(
        PluginId::new(9),
        PluginPermission::new(
            "fixture.scm".into(),
            vec![PermissionType::ReadApplicationState],
        ),
    );
    screen.assign_permission_modals();
    assert!(screen.permission_modal_open(CLIENT));
    screen.open_agents(CLIENT);
    assert!(!screen.command_palette_is_open(CLIENT));
}

#[test]
fn plugin_table_refresh_validates_shape_and_preserves_original_handle() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_types::command_palette::{
        Action as PaletteAction, Column, Command, Message, Reply, View,
    };
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let (sender, requests) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let command = |command| Instruction::Command {
        owner: Owner {
            plugin_id: PluginId::new(50),
            generation: 9,
            client_id: Some(CLIENT),
        },
        client: CLIENT,
        unavailable: Arc::new(AtomicBool::new(false)),
        command,
    };
    let next = || {
        requests
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::PluginPalette(delivery) => match delivery.message {
                    message @ (Message::Discover { .. } | Message::Execute { .. }) => Some(message),
                    _ => None,
                },
                _ => None,
            })
            .expect("palette request")
    };
    let action = |id: &str, cells| PaletteAction {
        id: id.into(),
        label: id.into(),
        handle: format!("handle-{id}"),
        description: String::new(),
        aliases: vec![],
        cells,
    };
    screen.plugin_palette_instruction(command(Command::Register {
        id: "table".into(),
        title: "Table".into(),
        registration: "1".into(),
        static_actions: true,
    }));
    screen.toggle_command_palette(CLIENT);
    let Message::Discover { scope, .. } = next() else {
        panic!()
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::Actions {
            actions: vec![action("unique-table", None)],
        }),
    }));
    screen.command_palette_handles_input(&None, b"unique-table", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, .. } = next() else {
        panic!()
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::View {
            view: View::Actions {
                title: "Records".into(),
                description: String::new(),
                initial_query: String::new(),
                source: Some("source".into()),
                columns: vec![Column {
                    header: "Subject".into(),
                    width: None,
                    max_lines: 3,
                }],
                actions: vec![],
            },
        }),
    }));
    let Message::Discover { scope, .. } = next() else {
        panic!()
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope: scope.clone(),
        result: Ok(Reply::Actions {
            actions: vec![
                action(
                    "original",
                    Some(vec!["A long Unicode 雪 subject ".repeat(20)]),
                ),
                action("jump", None),
            ],
        }),
    }));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        frame.contains("Subject") && frame.contains("Unicode 雪"),
        "{frame}"
    );
    assert!(screen.plugin_palette_instruction(Instruction::Refresh {
        client: CLIENT,
        scope
    }));
    let Message::Discover { scope, .. } = next() else {
        panic!()
    };
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::Actions {
            actions: vec![action(
                "malformed",
                Some(vec!["extra".into(), "cell".into()]),
            )],
        }),
    }));
    let frame = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        frame.contains("Unicode 雪") && !frame.contains("malformed"),
        "{frame}"
    );
    assert!(requests
        .try_iter()
        .any(|(instruction, _)| matches!(instruction,
        PluginInstruction::PluginPalette(yinz_bus::command_palette::Delivery {
            message: Message::Release { handles }, ..
        }) if handles == vec!["handle-malformed".to_string()])));
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { handle, .. } = next() else {
        panic!()
    };
    assert_eq!(handle, "handle-original");
}
