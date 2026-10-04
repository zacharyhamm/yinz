use super::*;
use yinz_types::internal_ids::{SessionName, TabNumber};
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
                assert!(
                    receiver.try_recv().is_err(),
                    "{target}, {query}, form={form}"
                );
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
            let (sender, pty_receiver) = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
            screen.dispatch_command_palette_action(origin, TabId::new(0), action, false, CLIENT);
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
                create_session,
                client_id,
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
                create_session,
                client_id,
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
        ("Fullscreen", "Ctrl g → Ctrl p → f"),
        ("New tab", "Ctrl g → Ctrl t → n"),
        ("Split right", "Ctrl g → Ctrl p → r"),
        ("Split down", "Ctrl g → Ctrl p → d"),
    ] {
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(&None, query.as_bytes(), false, CLIENT);
        let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(rendered.contains(hint), "{query}: {rendered}");
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
            InputMode::Locked,
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
        InputMode::Locked,
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
fn palette_shortcut_cache_reuses_hits_and_misses_and_tracks_live_targets() {
    let mut screen = screen();
    new_tab(&mut screen, 2, TabId::new(1));
    screen.update_client_tab_focus(CLIENT, TabId::new(0));
    screen.default_mode_info.keybinds = vec![(
        InputMode::Locked,
        vec![(
            KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier(),
            vec![Action::GoToTab {
                index: TabNumber::new(1),
            }],
        )],
    )];
    let receiver = screen_wired_to_server(&mut screen);
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"Switch tab", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let mut grid = grid_for_render_snapshot(SIZE);
    let before = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        before
            .lines()
            .any(|line| line.contains("tab 1") && line.contains("Alt x")),
        "{before}"
    );
    assert!(
        before
            .lines()
            .any(|line| line.contains("tab 2") && !line.contains("Alt x")),
        "{before}"
    );
    let lookups = screen.command_palettes[&CLIENT].shortcut_lookups;
    assert!(lookups > 2, "both bound and unbound entries were searched");
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_keymap_filters, 1);
    for _ in 0..3 {
        assert_eq!(
            render_frame_into(&mut screen, &receiver, CLIENT, &mut grid),
            before
        );
    }
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_lookups, lookups);
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_keymap_filters, 1);

    screen.tabs.get_mut(&TabId::new(1)).unwrap().position = TabPosition::new(0);
    screen.tabs.get_mut(&TabId::new(1)).unwrap().name = "renamed destination".into();
    screen.tabs.get_mut(&TabId::new(0)).unwrap().position = TabPosition::new(1);
    let after = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_ne!(before, after);
    assert!(
        after
            .lines()
            .any(|line| line.contains("renamed destination") && line.contains("Alt x")),
        "{after}"
    );
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        lookups + 2
    );
    assert_eq!(screen.command_palettes[&CLIENT].shortcut_keymap_filters, 2);

    let removed = screen.tabs.remove(&TabId::new(1)).unwrap();
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        lookups + 2
    );
    screen.tabs.insert(TabId::new(1), removed);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        lookups + 4,
        "removed tab and pane hints must be evicted"
    );

    screen.default_mode_info.keybinds.clear();
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        2 * lookups + 4
    );
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
            InputMode::Locked,
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
    assert!(rendered.contains("Ctrl g → Alt n"), "{rendered}");
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
    assert!(rendered.contains("Ctrl g → Alt n"), "{rendered}");
    assert_eq!(
        screen.command_palettes[&CLIENT].shortcut_lookups,
        lookups + 3,
        "new-pane original, floating target, and new pane entry changed"
    );
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .hide_floating_panes();
    screen.close_command_palette(CLIENT);
    screen.toggle_command_palette(CLIENT);
    screen.command_palette_handles_input(&None, b"New floating pane", false, CLIENT);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!rendered.contains("Ctrl g → Alt n"), "{rendered}");
}

#[test]
fn palette_sessions_show_ages_and_sort_newest_first_with_exited_separate() {
    use std::time::Duration;
    use yinz_types::data::SessionInfo;
    let mut screen = screen();
    screen.peer_sessions_cache.clear();
    for (name, seconds) in [("aa-old", 8060), ("bb-new", 20), ("cc-new", 20)] {
        let mut info = SessionInfo::new(SessionName::new(name));
        info.creation_time = Duration::from_secs(seconds);
        info.connected_clients = 1;
        screen.peer_sessions_cache.insert(info.name.clone(), info);
    }
    for (name, seconds) in [("aa-old", 1), ("dd-exited", 80), ("ee-exited", 10)] {
        screen
            .resurrectable_sessions_cache
            .insert(SessionName::new(name), Duration::from_secs(seconds));
    }
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    palette_search(&mut screen, "Switch session…");
    palette_key(&mut screen, BareKey::Enter);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Live sessions"), "{snapshot}");
    assert!(
        snapshot.find("bb-new").unwrap() < snapshot.find("cc-new").unwrap(),
        "{snapshot}"
    );
    assert!(
        snapshot.find("cc-new").unwrap() < snapshot.find("aa-old").unwrap(),
        "{snapshot}"
    );
    assert!(
        snapshot.contains("up 20s · 1 connected client"),
        "{snapshot}"
    );
    assert!(snapshot.contains("up 2h 14m"), "{snapshot}");
    for _ in 0..3 {
        palette_key(&mut screen, BareKey::Down);
    }
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(snapshot.contains("Exited sessions"), "{snapshot}");
    assert!(
        snapshot.contains("ee-exited — Exited · Enter to restore"),
        "{snapshot}"
    );
    palette_key(&mut screen, BareKey::Down);
    let snapshot = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(
        snapshot.find("ee-exited").unwrap() < snapshot.find("dd-exited").unwrap(),
        "{snapshot}"
    );
    // The duplicate live name must not add an exited entry before these two.
    assert!(snapshot.contains("› dd-exited"), "{snapshot}");
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
fn palette_restores_exited_sessions_and_rejects_deleted_checkpoints() {
    use yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR;
    use yinz_runtime::session_checkpoint::{write_checkpoint, SessionCheckpoint};

    let mut screen = screen();
    std::fs::create_dir_all(&*YINZ_SESSION_INFO_CACHE_DIR).unwrap();
    let saved = tempfile::tempdir_in(&*YINZ_SESSION_INFO_CACHE_DIR).unwrap();
    let name = SessionName::new(saved.path().file_name().unwrap().to_str().unwrap());
    write_checkpoint(
        saved.path(),
        "",
        &("layout { pane; }".into(), Default::default()),
    )
    .unwrap();
    screen
        .resurrectable_sessions_cache
        .insert(name.clone(), Default::default());
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Switch session…");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, name.as_str().as_bytes(), false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    let action = enqueued_palette_action(&receiver);
    assert!(matches!(&action, Action::SwitchSession { name: target, .. } if target == &name));

    let layout = SessionCheckpoint::read(saved.path()).unwrap().layout_path();
    std::fs::remove_file(layout).unwrap();
    screen.dispatch_command_palette_action(
        PaneId::Terminal(TerminalId::new(1)),
        TabId::new(0),
        action,
        false,
        CLIENT,
    );
    assert!(matches!(
        receiver.try_recv().unwrap().0,
        ScreenInstruction::Notify(..)
    ));
    palette_search(&mut screen, "Switch session…");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, name.as_str().as_bytes(), false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_recv().is_err());
    assert!(screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn project_context_tracks_focus_membership_and_client_lifetime() {
    let mut screen = screen();
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
    palette_key(&mut screen, BareKey::Enter);
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
    screen.projects.clear();
    palette_key(&mut screen, BareKey::Enter);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    let rendered = render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(rendered.contains("Project no longer exists"), "{rendered}");
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
        assert!(frame.contains("New pane"), "{frame}");
        let cells = grid.as_character_lines();
        assert_eq!(*control.get_or_insert(cells[6][11].clone()), cells[6][11]);
        // Sample blank palette space, away from the input, choices and border.
        let (x, y) = (60, 5);
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
        id: "remote".into(),
        title: "Remote".into(),
        registration: "1".into(),
    }));
    screen.toggle_command_palette(CLIENT);
    let next = || {
        requests
            .try_iter()
            .find_map(|(request, _)| match request {
                PluginInstruction::DenoPalette(
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
    assert_eq!(context.client_id, u32::from(CLIENT.get()));
    // Repainting never requests discovery again.
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!requests.try_iter().any(|(request, _)| matches!(
        request,
        PluginInstruction::DenoPalette(Delivery {
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
                    id: "action".into(),
                    label: label.into(),
                    description: String::new(),
                    handle: "handle".into(),
                }],
            }),
        })
    };
    assert!(!screen.plugin_palette_instruction(reply(first_scope.clone(), "Obsolete")));
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(request, _)| match request {
            PluginInstruction::DenoPalette(Delivery {
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
        PluginInstruction::DenoPalette(Delivery {
            message: Message::Release { .. },
            ..
        })
    )));
    assert!(
        screen.plugin_palette_instruction(command(Command::Register {
            id: "unrelated".into(),
            title: "Unrelated".into(),
            registration: "3".into(),
        }))
    );
    for (request, _) in requests.try_iter() {
        assert!(!matches!(
            request,
            PluginInstruction::DenoPalette(Delivery {
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
        PluginInstruction::DenoPalette(Delivery {
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
            PluginInstruction::DenoPalette(delivery) => Some(delivery),
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
        PluginInstruction::DenoPalette(delivery) if matches!(&delivery.message, Message::Discover { scope: current, .. } if current.revision == scope.revision + 1) => Some(delivery), _ => None,
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
                            id: "same-action-id".into(),
                            label: format!("Result {id}"),
                            description: String::new(),
                            handle: "1".into(),
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
        PluginInstruction::DenoPalette(yinz_bus::command_palette::Delivery {
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
                    PluginInstruction::DenoPalette(delivery)
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
                instruction, PluginInstruction::DenoPalette(delivery)
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
fn plugin_palette_replacements_release_handles_and_nested_removal_restores_root() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use yinz_bus::command_palette::{Instruction, Owner};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Action as PaletteAction, Command, Message, Reply, View};
    use yinz_types::internal_ids::PluginId;

    let mut screen = screen();
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
        id: "stable".into(),
        label: "Retained action".into(),
        description: String::new(),
        handle: handle.into(),
    };
    let next = || {
        requests
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::DenoPalette(delivery)
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
            PluginInstruction::DenoPalette(delivery) => match delivery.message {
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
                title: "Nested".into(),
                actions: vec![action("nested")],
                source: Some("source".into()),
            }
        }),
    })));
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
    palette_key(&mut screen, BareKey::Esc);
    palette_key(&mut screen, BareKey::Enter);
    let Message::Execute { scope, handle, .. } = next().message else {
        panic!()
    };
    assert_eq!(handle, "new");
    screen.plugin_palette_instruction(command(Command::Reply {
        scope,
        result: Ok(Reply::View {
            view: View::Actions {
                title: "Nested again".into(),
                actions: vec![action("nested2")],
                source: Some("source2".into()),
            },
        }),
    }));
    let pending = next();
    unavailable.store(true, Ordering::Release);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
    assert!(!pending.active.unwrap().load(Ordering::Acquire));
    let releases: Vec<_> = requests
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::DenoPalette(delivery) => match delivery.message {
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
fn root_palette_escape_cancels_discovery_and_debounce() {
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
            id: "remote".into(),
            title: "Remote".into(),
            registration: "1".into(),
        }));
        screen.toggle_command_palette(CLIENT);
        let next = || {
            requests
                .try_iter()
                .find_map(|(request, _)| match request {
                    PluginInstruction::DenoPalette(
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
            PluginInstruction::DenoPalette(Delivery {
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
                    id: "late".into(),
                    label: "Late".into(),
                    description: String::new(),
                    handle: "late".into(),
                }],
            }),
        }));
        assert!(!screen.command_palettes.contains_key(&CLIENT));
        assert!(!requests.try_iter().any(|(instruction, _)| matches!(
            instruction,
            PluginInstruction::DenoPalette(Delivery {
                message: Message::Discover { .. },
                ..
            })
        )));
    }
}

#[test]
fn control_agent_picker_uses_live_local_state_and_rejects_removed_registration() {
    use yinz_types::agent::control::AgentRequest;
    use yinz_types::agent::{AgentInfo, AgentState};
    let mut screen = screen();
    let pane = PaneId::Terminal(TerminalId::new(1));
    screen.tabs.get_mut(&TabId::new(0)).unwrap().set_pane_agent(
        pane,
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Idle,
            source: yinz_types::agent::AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: Some("conversation".into()),
            run_id: None,
        }),
    );
    screen.agent_control(
        AgentRequest::SetControlAgent {
            pane_id: pane,
            enabled: true,
        },
        CLIENT,
    );
    // Stale discovery data for this session cannot erase live registration.
    screen
        .peer_sessions_cache
        .get_mut(&screen.session_name)
        .unwrap()
        .control_agents
        .clear();
    let receiver = palette_actions_receiver(&mut screen);
    palette_search(&mut screen, "Focus control agent");
    palette_key(&mut screen, BareKey::Enter);
    palette_key(&mut screen, BareKey::Enter);
    assert_eq!(
        enqueued_palette_action(&receiver),
        Action::FocusPaneByPaneId { pane_id: pane }
    );
    assert!(!screen.command_palettes.contains_key(&CLIENT));
    palette_search(&mut screen, "Focus control agent");
    palette_key(&mut screen, BareKey::Enter);
    screen.control_agents.clear(); // Lose registration between render and activation.
    palette_key(&mut screen, BareKey::Enter);
    assert!(receiver.try_recv().is_err());
    assert!(screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn control_agent_picker_qualifies_peer_ids_filters_panes_and_targets_other_sessions() {
    use yinz_types::agent::{AgentInfo, AgentState};
    let mut screen = screen();
    let pane = PaneId::Terminal(TerminalId::new(1));
    let agent = AgentInfo {
        kind: "codex".into(),
        state: AgentState::Idle,
        source: yinz_types::agent::AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: None,
        session_id: Some("conversation".into()),
        run_id: None,
    };
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .set_pane_agent(pane, Some(agent));
    screen.control_agents.insert(TerminalId::new(1));
    screen.log_and_report_session_state().unwrap();
    // A real local discovery entry is required before switching sessions.
    std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let socket = tempfile::NamedTempFile::new_in(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let name = SessionName::new(socket.path().file_name().unwrap().to_str().unwrap());
    let mut peer = screen.peer_sessions_cache[&screen.session_name].clone();
    peer.name = name.clone();
    peer.is_current_session = false;
    for panes in peer.panes.panes.values_mut() {
        for pane in panes {
            pane.title = "remote-controller".into();
        }
    }
    screen.peer_sessions_cache.insert(name.clone(), peer);
    let receiver = palette_actions_receiver(&mut screen);
    for excluded in [
        "suppressed",
        "unselectable",
        "exited",
        "unregistered",
        "missing",
    ] {
        let original = screen.peer_sessions_cache[&name].clone();
        let peer = screen.peer_sessions_cache.get_mut(&name).unwrap();
        match excluded {
            "unregistered" => peer.control_agents.clear(),
            "missing" => peer.panes.panes.clear(),
            _ => {
                for pane in peer.panes.panes.values_mut().flatten() {
                    match excluded {
                        "suppressed" => pane.is_suppressed = true,
                        "unselectable" => pane.is_selectable = false,
                        _ => pane.exited = true,
                    }
                }
            },
        }
        palette_search(&mut screen, "Focus control agent");
        palette_key(&mut screen, BareKey::Enter);
        screen.command_palette_handles_input(&None, b"remote-controller", false, CLIENT);
        palette_key(&mut screen, BareKey::Enter);
        assert!(receiver.try_recv().is_err(), "{excluded}");
        screen.close_command_palette(CLIENT);
        screen.peer_sessions_cache.insert(name.clone(), original);
    }
    palette_search(&mut screen, "Focus control agent");
    palette_key(&mut screen, BareKey::Enter);
    screen.command_palette_handles_input(&None, b"remote-controller", false, CLIENT);
    palette_key(&mut screen, BareKey::Enter);
    assert!(
        matches!(enqueued_palette_action(&receiver), Action::SwitchSession { name: target, pane_id: Some(id), .. } if target == name && id == pane)
    );
}
