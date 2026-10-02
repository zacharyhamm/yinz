use super::*;
use yinz_types::keys::{BareKey, KeyWithModifier};

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size { cols: 80, rows: 24 };

fn screen() -> Screen {
    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
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
    assert!(opened.contains("Open settings"), "{opened}");
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
    assert!(snapshot.contains("Open settings"), "{snapshot}");
    assert!(!snapshot.contains("private"), "{snapshot}");
    assert_eq!(cursor, Some((11, 4)));
    screen.command_palette_handles_input(&None, b"file", false, CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        snapshot.contains("> file") && snapshot.contains("Find file"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(cursor, Some((15, 4)));

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
    assert!(snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(cursor, Some((11, 4)));
    screen.command_palette_handles_input(&None, b"long query", false, CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(snapshot.contains("> long quer"), "{snapshot}");
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
    assert_eq!(cursor, Some((11, 4)));
    assert!(!screen.watcher_clients[&watcher].cursor_was_cropped());
    screen.close_command_palette(CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(!snapshot.contains("long quer"), "{snapshot}");
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
    assert_eq!(cursor, Some((11, 4)), "{snapshot}");
    assert!(snapshot.contains("New pane") && snapshot.contains("Open settings"));
    assert_eq!(screen.get_active_pane_id(&CLIENT), pane);
    // Even an intercepting plugin must not get the palette's keys.
    assert!(!screen
        .write_character(
            Some(KeyWithModifier::new(BareKey::Char('h'))),
            b"h".to_vec(),
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
    assert!(snapshot.contains("Selected: Switch session"), "{snapshot}");
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
        !snapshot.contains("Selected:") && !snapshot.contains("Open settings"),
        "{snapshot}"
    );
    screen.toggle_command_palette(CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("New pane") && !snapshot.contains("Selected:"),
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
    assert!(!snapshot.contains("Open settings"), "{snapshot}");
    assert_eq!(cursor, Some(pane_cursor(&mut screen, second)));
    screen.toggle_command_palette(second);
    screen.command_palette_handles_input(&None, b"file", false, CLIENT);
    screen.command_palette_handles_input(&None, b"Help", false, second);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Find file") && !snapshot.contains("Help"),
        "{snapshot}"
    );
    let snapshot = render_frame_into(&mut screen, &receiver, second, &mut grid);
    assert!(
        snapshot.contains("Help") && !snapshot.contains("Find file"),
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
fn palette_paste_resize_and_outside_click_repaint_without_changing_focus() {
    let mut screen = screen();
    let other_pane = PaneId::Terminal(TerminalId::new(2));
    let tab = screen.get_active_tab_mut(CLIENT).unwrap();
    tab.horizontal_split(other_pane, None, CLIENT, None, None)
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
fn palette_repaints_its_background_on_frames_without_new_pane_output() {
    let mut screen = screen();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    screen.toggle_command_palette(CLIENT);
    for text in ["background one", "background two"] {
        screen
            .get_active_tab_mut(CLIENT)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(1), format!("\x1b[9;40H{text}").into_bytes())
            .unwrap();
        let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(snapshot.contains(text), "{snapshot}");
        // A query-only frame must tint fresh pane cells too, rather than clearing the backing text.
        screen.command_palette_handles_input(
            &Some(KeyWithModifier::new(BareKey::Left)),
            &[],
            false,
            CLIENT,
        );
        let unchanged = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(snapshot, unchanged);
    }
    screen.close_command_palette(CLIENT);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("background two") && !snapshot.contains("Open settings"),
        "{snapshot}"
    );
}
