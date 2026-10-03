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
    assert!(opened.contains("Switch session"), "{opened}");
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
    assert!(snapshot.contains("Switch session"), "{snapshot}");
    assert!(!snapshot.contains("private"), "{snapshot}");
    assert_eq!(cursor, Some((11, 4)));
    screen.command_palette_handles_input(&None, b"Switch tab", false, CLIENT);
    let (cursor, snapshot) =
        render_frame_and_cursor_into(&mut screen, &receiver, watcher, &mut grid);
    assert!(
        snapshot.contains("> Switch tab") && snapshot.contains("Switch tab"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("New pane"), "{snapshot}");
    assert_eq!(cursor, Some((21, 4)));

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
    assert!(snapshot.contains("New pane") && snapshot.contains("Switch session"));
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
        !snapshot.contains("Selected:") && !snapshot.contains("Switch session"),
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
    assert!(!snapshot.contains("Switch session"), "{snapshot}");
    assert_eq!(cursor, Some(pane_cursor(&mut screen, second)));
    screen.toggle_command_palette(second);
    screen.command_palette_handles_input(&None, b"Switch tab", false, CLIENT);
    screen.command_palette_handles_input(&None, b"Focus pane", false, second);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.contains("Switch tab") && !snapshot.contains("Focus pane"),
        "{snapshot}"
    );
    let snapshot = render_frame_into(&mut screen, &receiver, second, &mut grid);
    assert!(
        snapshot.contains("Focus pane") && !snapshot.contains("Switch tab"),
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
        snapshot.contains("background two") && !snapshot.contains("Switch session"),
        "{snapshot}"
    );
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
        ("Save session", Action::SaveSession),
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
fn palette_forms_validate_session_names_and_capture_creation_values() {
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
    palette_search(&mut screen, "New session…");
    palette_key(&mut screen, BareKey::Enter);
    for _ in 0..3 {
        palette_key(&mut screen, BareKey::Enter);
    }
    assert!(screen.command_palettes.contains_key(&CLIENT));
    assert!(receiver.try_recv().is_err());
    palette_key(&mut screen, BareKey::Up);
    palette_key(&mut screen, BareKey::Up);
    let existing = screen.session_name.clone();
    screen.command_palette_handles_input(&None, existing.as_str().as_bytes(), false, CLIENT);
    for _ in 0..3 {
        palette_key(&mut screen, BareKey::Enter);
    }
    assert!(screen.command_palettes.contains_key(&CLIENT));
    assert!(
        receiver.try_recv().is_err(),
        "existing sessions cannot be created again"
    );
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
    assert!(receiver.try_recv().is_err());
    assert!(screen.command_palettes.contains_key(&CLIENT));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(other));
    screen.dispatch_command_palette_action(
        PaneId::Terminal(TerminalId::new(1)),
        TabId::new(0),
        Action::BreakPane,
        false,
        CLIENT,
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
        false,
        CLIENT,
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
fn palette_destination_selection_survives_tab_reordering_and_rename() {
    let mut screen = screen();
    new_tab(&mut screen, 2, TabId::new(1));
    screen.update_client_tab_focus(CLIENT, TabId::new(0));
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Switch tab…");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Down);
    screen.tabs.get_mut(&TabId::new(1)).unwrap().position = TabPosition::new(0);
    screen.tabs.get_mut(&TabId::new(1)).unwrap().name = "renamed destination".into();
    screen.tabs.get_mut(&TabId::new(0)).unwrap().position = TabPosition::new(1);
    screen.render().unwrap();
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        enqueued_palette_action(&receiver),
        Action::GoToTabById { id: TabId::new(1) }
    );
}

#[test]
fn palette_drops_a_destination_removed_after_selection() {
    let mut screen = screen();
    new_tab(&mut screen, 2, TabId::new(1));
    screen.update_client_tab_focus(CLIENT, TabId::new(0));
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Switch tab…");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Down);
    screen.tabs.remove(&TabId::new(1));
    // No intervening paint: Enter still refers to the now-stale displayed ID.
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_recv().is_err());
    assert!(screen.command_palettes.contains_key(&CLIENT));
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
        screen.dispatch_command_palette_action(origin, TabId::new(0), action, false, CLIENT);
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
        pane.scroll_up(5, CLIENT);
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
    screen
        .move_command_palette_pane(origin, TabId::new(0), None, CLIENT)
        .unwrap();
    assert_eq!(screen.tabs.len(), 2);
    assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&other));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(origin));
}
