//! `Notifications` mode: the in-place notification browser.

use super::*;
use crate::screen::{Notification, NotificationClick, NotificationRecord};
use crate::ui::damage::CellRect;
use crate::ui::notification::{notification_browser_layout, notification_height};
use std::time::Instant;
use yinz_types::animation::Tween;
use yinz_types::colors::PaletteColor;
use yinz_types::data::NotificationInfo;
use yinz_types::input::options::NotificationPosition;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::TabId;
use yinz_types::pane_size::Viewport;

const CORNERS: [NotificationPosition; 4] = [
    NotificationPosition::TopLeft,
    NotificationPosition::TopRight,
    NotificationPosition::BottomLeft,
    NotificationPosition::BottomRight,
];

fn record(
    id: u64,
    text: &str,
    pane_id: Option<PaneId>,
    on_click: Option<NotificationClick>,
) -> NotificationRecord {
    NotificationRecord {
        info: NotificationInfo {
            id: NotificationId::new(id),
            message: text.to_owned(),
            received_at_unix_ms: yinz_types::units::UnixMillis::new(0),
            outcome: None,
        },
        pane_id,
        on_click,
    }
}

fn settled_toast(id: u64, text: &str) -> Notification {
    let mut notification = Notification::new(
        NotificationId::new(id),
        text.to_owned(),
        None,
        None,
        Instant::now(),
    );
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(0.0);
    notification
}

fn click_target(payload: &str) -> NotificationClick {
    NotificationClick {
        plugin_id: crate::PluginId::new(7),
        client_id: ClientId::new(1),
        message: "agent_toast_click".to_owned(),
        payload: payload.to_owned(),
    }
}

/// A screen with one tab, `records` in its history (oldest first) and client 1 browsing them.
fn browsing(records: Vec<NotificationRecord>) -> (Screen, ClientId) {
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 30 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.notification_history = records;
    screen
        .change_mode(InputMode::Notifications, Some(InputMode::Normal), client_id)
        .expect("TEST");
    (screen, client_id)
}

fn listed(screen: &Screen, client_id: ClientId) -> (Vec<NotificationId>, usize) {
    let viewport = screen
        .notification_browser_viewport(client_id)
        .expect("the client has a tab to browse over");
    let frame = screen
        .notification_browser_frame(client_id, viewport)
        .expect("the client is browsing");
    (frame.ids, frame.selected)
}

fn selected_id(screen: &Screen, client_id: ClientId) -> NotificationId {
    let (ids, selected) = listed(screen, client_id);
    ids[selected]
}

fn mode_changes(server_rx: &Receiver<(ServerInstruction, ErrorContext)>) -> Vec<InputMode> {
    server_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::ChangeMode(_, mode, _) => Some(mode),
            _ => None,
        })
        .collect()
}

#[test]
fn entering_the_mode_lists_every_notification_newest_first() {
    let (screen, client_id) = browsing(vec![
        record(0, "first", None, None),
        record(1, "second", None, None),
        record(2, "third", None, None),
    ]);

    assert_eq!(
        listed(&screen, client_id),
        (
            vec![
                NotificationId::new(2),
                NotificationId::new(1),
                NotificationId::new(0)
            ],
            0
        )
    );
}

#[test]
fn leaving_the_mode_closes_the_browser() {
    let (mut screen, client_id) = browsing(vec![record(0, "first", None, None)]);
    assert!(screen.notification_browsers.contains_key(&client_id));

    screen
        .change_mode(InputMode::Normal, Some(InputMode::Normal), client_id)
        .expect("TEST");

    assert!(!screen.notification_browsers.contains_key(&client_id));
}

#[test]
fn the_selection_moves_down_the_list_and_stops_at_both_ends() {
    let (mut screen, client_id) = browsing(vec![
        record(0, "oldest", None, None),
        record(1, "middle", None, None),
        record(2, "newest", None, None),
    ]);

    // the default corner is at the top, so down is towards older notifications
    screen.move_notification_selection(Direction::Down, client_id);
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(1));
    screen.move_notification_selection(Direction::Down, client_id);
    screen.move_notification_selection(Direction::Down, client_id);
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));

    screen.move_notification_selection(Direction::Up, client_id);
    screen.move_notification_selection(Direction::Up, client_id);
    screen.move_notification_selection(Direction::Up, client_id);
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(2));
}

#[test]
fn only_the_selected_box_shifts_inward_within_viewport_margins() {
    let text =
        "a notification long enough to reach the maximum box width and wrap onto several rows";
    for corner in CORNERS {
        let (mut screen, client_id) =
            browsing((0..12).map(|id| record(id, text, None, None)).collect());
        screen.notification_position = corner;
        for selected in 0..12 {
            for expanded in [false, true] {
                if expanded {
                    screen.toggle_notification_detail(client_id);
                }
                for (cols, shift) in [(7, 0), (20, 0), (52, 0), (53, 1), (54, 2), (80, 2)] {
                    let viewport = Viewport {
                        x: 11,
                        y: 5,
                        cols,
                        rows: 9,
                    };
                    let frame = screen
                        .notification_browser_frame(client_id, viewport)
                        .unwrap();
                    let unshifted = notification_browser_layout(
                        viewport,
                        corner,
                        &[text; 12],
                        selected,
                        expanded,
                        frame.layout.first,
                    )
                    .unwrap();
                    assert_eq!(frame.selected, selected);
                    assert_eq!(frame.layout.width, unshifted.width);
                    assert_eq!(frame.layout.entries.len(), unshifted.entries.len());
                    assert!(frame
                        .layout
                        .entries
                        .iter()
                        .any(|entry| entry.index == selected));
                    if selected == 11 {
                        assert!(
                            frame.layout.first > 0,
                            "the history scrolls beyond the visible window"
                        );
                    }
                    for (entry, original) in frame.layout.entries.iter().zip(unshifted.entries) {
                        let offset = if entry.index == selected { shift } else { 0 };
                        let expected_x = if corner.is_left() {
                            original.x + offset
                        } else {
                            original.x - offset
                        };
                        assert_eq!(
                            entry.x, expected_x,
                            "{corner:?}, {cols} columns, selected {selected}"
                        );
                        assert_eq!(entry.y, original.y);
                        assert_eq!(entry.lines, original.lines);
                        assert!(entry.x > viewport.x);
                        assert!(entry.x + frame.layout.width < viewport.x + viewport.cols);
                        assert!(entry.y >= viewport.y);
                        assert!(
                            entry.y + notification_height(entry.lines.len())
                                <= viewport.y + viewport.rows
                        );
                    }
                }
            }
            screen.move_notification_selection(
                if corner.is_top() {
                    Direction::Down
                } else {
                    Direction::Up
                },
                client_id,
            );
        }
    }
}

#[test]
fn the_empty_history_placeholder_stays_at_the_anchor() {
    let (mut screen, client_id) = browsing(vec![]);
    let viewport = Viewport {
        x: 11,
        y: 5,
        cols: 80,
        rows: 9,
    };
    for corner in CORNERS {
        screen.notification_position = corner;
        let frame = screen
            .notification_browser_frame(client_id, viewport)
            .unwrap();
        assert!(frame.ids.is_empty());
        assert_eq!(
            frame.layout,
            notification_browser_layout(viewport, corner, &["No notifications"], 0, false, 0,)
                .unwrap()
        );
    }
}

#[test]
fn a_notification_arriving_mid_browse_does_not_move_the_selection() {
    let (mut screen, client_id) = browsing(vec![
        record(0, "older", None, None),
        record(1, "newer", None, None),
    ]);
    screen.move_notification_selection(Direction::Down, client_id);
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));

    screen
        .notification_history
        .push(record(2, "just arrived", None, None));
    screen.notification_browsers_saw_arrival();

    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));
}

#[test]
fn a_notification_arriving_before_any_move_does_not_move_the_selection() {
    let (mut screen, client_id) = browsing(vec![
        record(0, "older", None, None),
        record(1, "newer", None, None),
    ]);
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(1));

    screen
        .notification_history
        .push(record(2, "just arrived", None, None));
    screen.notification_browsers_saw_arrival();

    assert_eq!(selected_id(&screen, client_id), NotificationId::new(1));
}

#[test]
fn a_notification_arriving_in_an_empty_browser_is_selected_and_kept() {
    let (mut screen, client_id) = browsing(vec![]);

    screen
        .notification_history
        .push(record(0, "first", None, None));
    screen.notification_browsers_saw_arrival();
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));

    screen
        .notification_history
        .push(record(1, "second", None, None));
    screen.notification_browsers_saw_arrival();
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));
}

#[test]
fn a_move_that_goes_nowhere_keeps_the_detail_open() {
    let long = "one two three four five six seven eight nine ten eleven twelve thirteen \
        fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two \
        twenty-three twenty-four twenty-five twenty-six twenty-seven twenty-eight";
    let (mut screen, client_id) = browsing(vec![record(0, long, None, None)]);
    let rows = |screen: &Screen| {
        let viewport = screen.notification_browser_viewport(client_id).unwrap();
        let frame = screen
            .notification_browser_frame(client_id, viewport)
            .unwrap();
        frame.layout.entries[0].lines.clone()
    };
    assert!(screen.toggle_notification_detail(client_id));
    let expanded = rows(&screen);

    // the default corner is at the top, so up is towards newer notifications - there are none
    assert!(!screen.move_notification_selection(Direction::Up, client_id));

    assert_eq!(rows(&screen), expanded);
}

#[test]
fn toggling_the_detail_shows_the_selected_notifications_whole_text() {
    let long = "one two three four five six seven eight nine ten eleven twelve thirteen \
        fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two \
        twenty-three twenty-four twenty-five twenty-six twenty-seven twenty-eight";
    let (mut screen, client_id) = browsing(vec![record(0, long, None, None)]);
    let rows = |screen: &Screen| {
        let viewport = screen.notification_browser_viewport(client_id).unwrap();
        let frame = screen
            .notification_browser_frame(client_id, viewport)
            .unwrap();
        frame.layout.entries[0].lines.clone()
    };
    let collapsed = rows(&screen);
    assert_eq!(collapsed.len(), 2);
    assert!(collapsed[1].ends_with('…'));

    assert!(screen.toggle_notification_detail(client_id));
    let expanded = rows(&screen);
    assert!(expanded.len() > 2, "{expanded:?}");
    assert!(expanded.last().unwrap().ends_with("twenty-eight"));

    assert!(screen.toggle_notification_detail(client_id));
    assert_eq!(rows(&screen), collapsed);
}

#[test]
fn activating_an_entry_does_what_clicking_its_toast_does_and_leaves_the_mode() {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![
        record(
            0,
            "agent has a plan",
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            Some(click_target("42")),
        ),
        record(1, "newer", None, None),
    ]);
    new_tab(&mut screen, 2, TabId::new(1));
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    screen
        .notifications
        .push(settled_toast(0, "agent has a plan"));
    let now = Instant::now();
    screen.pinned_now = Some(now);

    for _ in 0..2 {
        screen
            .focus_pane_with_id(PaneId::Terminal(TerminalId::new(2)), true, false, client_id)
            .unwrap();
        screen.move_notification_selection(Direction::Down, client_id);
        screen.activate_selected_notification(client_id, now);

        let payloads: Vec<_> = plugin_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::NotificationClicked { payload, .. } => Some(payload),
                _ => None,
            })
            .collect();
        assert_eq!(payloads, vec!["42"]);
        assert_eq!(
            screen.get_active_pane_id(&client_id),
            Some(PaneId::Terminal(TerminalId::new(1)))
        );
        assert!(screen.notifications[0].is_dismissing());
        assert!(!screen.notification_browsers.contains_key(&client_id));
        assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);

        screen.open_notification_browser(client_id);
        assert_eq!(
            listed(&screen, client_id),
            (vec![NotificationId::new(1), NotificationId::new(0)], 0)
        );
    }
}

#[test]
fn activating_a_notification_with_no_target_keeps_it_and_leaves_the_mode() {
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![record(0, "just saying", None, None)]);
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));

    screen.activate_selected_notification(client_id, Instant::now());

    assert!(!screen.notification_browsers.contains_key(&client_id));
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
    screen.open_notification_browser(client_id);
    assert_eq!(
        listed(&screen, client_id),
        (vec![NotificationId::new(0)], 0)
    );
}

#[test]
fn a_toast_with_a_missing_pane_can_be_activated_again_from_history() {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![record(
        0,
        "gone",
        Some(PaneId::Terminal(TerminalId::new(99))),
        Some(click_target("gone")),
    )]);
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let focused = screen.get_active_pane_id(&client_id);
    let mut toast = settled_toast(0, "gone");
    toast.pane_id = screen.notification_history[0].pane_id;
    toast.on_click = screen.notification_history[0].on_click.clone();
    screen.notifications.push(toast);
    screen.close_notification_browser(client_id);
    let viewport = screen.notification_browser_viewport(client_id).unwrap();
    let now = Instant::now();
    let rect = screen.notification_cell_rects(viewport, now)[0];
    assert!(screen.handle_pane_notification_click(
        &MouseEvent::new_left_press_event(Position::new((rect.y + 1) as i32, rect.x as u16)),
        viewport,
        client_id,
        now,
    ));

    screen.open_notification_browser(client_id);
    assert_eq!(
        listed(&screen, client_id),
        (vec![NotificationId::new(0)], 0)
    );
    screen.activate_selected_notification(client_id, now);

    let payloads: Vec<_> = plugin_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::NotificationClicked { payload, .. } => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(payloads, vec!["gone", "gone"]);
    assert_eq!(screen.get_active_pane_id(&client_id), focused);
    assert!(!screen.notification_browsers.contains_key(&client_id));
    screen.open_notification_browser(client_id);
    assert_eq!(
        listed(&screen, client_id),
        (vec![NotificationId::new(0)], 0)
    );
}

#[test]
fn activating_an_empty_browser_just_leaves_the_mode() {
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![]);
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));

    screen.activate_selected_notification(client_id, Instant::now());

    assert!(!screen.notification_browsers.contains_key(&client_id));
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
}

#[test]
fn clicking_an_entry_activates_that_entry() {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![
        record(0, "older", None, Some(click_target("older"))),
        record(1, "newer", None, Some(click_target("newer"))),
    ]);
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    let viewport = screen.notification_browser_viewport(client_id).unwrap();
    let frame = screen
        .notification_browser_frame(client_id, viewport)
        .unwrap();
    let older = frame
        .layout
        .entries
        .iter()
        .find(|entry| frame.ids[entry.index] == NotificationId::new(0))
        .unwrap();

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new((older.y + 1) as i32, (older.x + 2) as u16)),
        client_id,
    );

    let payloads: Vec<String> = plugin_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::NotificationClicked { payload, .. } => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(payloads, vec!["older".to_owned()]);
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
    screen.open_notification_browser(client_id);
    assert_eq!(
        listed(&screen, client_id),
        (vec![NotificationId::new(1), NotificationId::new(0)], 0)
    );
}

#[test]
fn clicking_the_shifted_edge_activates_the_selected_entry() {
    for corner in CORNERS {
        let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        let (mut screen, client_id) =
            browsing(vec![record(0, "edge", None, Some(click_target("edge")))]);
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        screen.notification_position = corner;
        let viewport = screen.notification_browser_viewport(client_id).unwrap();
        let rect = screen.notification_browser_rects(client_id, viewport)[0];
        let x = if corner.is_left() {
            rect.x + rect.cols - 1
        } else {
            rect.x
        };

        screen.handle_mouse_event(
            MouseEvent::new_left_press_event(Position::new((rect.y + 1) as i32, x as u16)),
            client_id,
        );

        assert!(
            !screen.notification_browsers.contains_key(&client_id),
            "{corner:?}"
        );
        assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
            instruction, PluginInstruction::NotificationClicked { payload, .. } if payload == "edge"
        )));
    }
}

#[test]
fn selection_rendering_occlusion_and_cleanup_follow_the_shifted_boxes() {
    for corner in CORNERS {
        let (mut screen, client_id) = browsing(vec![
            record(0, "older", None, None),
            record(1, "newer", None, None),
        ]);
        screen.notification_position = corner;
        screen.style.colors.frame_selected.base = PaletteColor::Rgb((12, 34, 56));
        let receiver = screen_wired_to_server(&mut screen);
        let viewport = screen.notification_browser_viewport(client_id).unwrap();
        let rect = screen.notification_browser_rects(client_id, viewport)[0];
        let x = if corner.is_left() {
            rect.x + rect.cols - 1
        } else {
            rect.x
        };
        let target = (x, rect.y + 1);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(1),
                format!("\x1b[{};{}H", target.1 + 1, target.0 + 1).into_bytes(),
            )
            .unwrap();
        assert_eq!(pane_cursor(&mut screen, client_id), target);

        let mut grid = grid_for_render_snapshot(screen.surface_size());
        screen.close_notification_browser(client_id);
        let (_, baseline) =
            render_frame_and_cursor_into(&mut screen, &receiver, client_id, &mut grid);
        let uncovered_cell = grid.get_character_at(target.0, target.1).unwrap();
        screen.open_notification_browser(client_id);

        let (cursor, _) =
            render_frame_and_cursor_into(&mut screen, &receiver, client_id, &mut grid);
        assert_eq!(
            cursor, None,
            "the shifted edge hides the cursor at {corner:?}"
        );
        let frame = screen
            .notification_browser_frame(client_id, viewport)
            .unwrap();
        for entry in frame.layout.entries {
            let selected = entry.index == frame.selected;
            let declaration = if selected {
                screen.style.colors.text_selected
            } else {
                screen.style.colors.text_unselected
            };
            let border_color = if selected {
                screen.style.colors.frame_selected.base
            } else {
                declaration.emphasis_0
            };
            for (x, y) in [
                (entry.x, entry.y),
                (entry.x, entry.y + 1),
                (entry.x + frame.layout.width - 1, entry.y + 2),
            ] {
                let border = grid.get_character_at(x, y).unwrap();
                assert_eq!(border.styles.foreground, Some(border_color.into()));
                assert_eq!(
                    border.styles.background,
                    Some(declaration.background.into())
                );
            }
            let text = grid.get_character_at(entry.x + 2, entry.y + 1).unwrap();
            assert_eq!(text.character, if selected { 'n' } else { 'o' });
            assert_eq!(text.styles.foreground, Some(declaration.base.into()));
            assert_eq!(text.styles.background, Some(declaration.background.into()));
        }

        screen.move_notification_selection(
            if corner.is_top() {
                Direction::Down
            } else {
                Direction::Up
            },
            client_id,
        );
        let (cursor, _) =
            render_frame_and_cursor_into(&mut screen, &receiver, client_id, &mut grid);
        assert_eq!(cursor, Some(target));
        assert_eq!(
            grid.get_character_at(target.0, target.1).unwrap(),
            uncovered_cell
        );
        screen.close_notification_browser(client_id);
        let (cursor, restored) =
            render_frame_and_cursor_into(&mut screen, &receiver, client_id, &mut grid);
        assert_eq!(cursor, Some(target));
        assert_eq!(
            restored, baseline,
            "closing the browser restores every cell at {corner:?}"
        );
    }
}

#[test]
fn a_click_outside_the_list_keeps_the_mode() {
    let (mut screen, client_id) = browsing(vec![record(0, "still here", None, None)]);

    let pane_position = Position::new(20, 5);
    screen.handle_mouse_event(MouseEvent::new_left_press_event(pane_position), client_id);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(pane_position), client_id);

    assert!(screen.notification_browsers.contains_key(&client_id));
    assert_eq!(selected_id(&screen, client_id), NotificationId::new(0));
}

#[test]
fn a_browsing_client_is_shown_the_list_instead_of_its_toasts() {
    let (mut screen, client_id) = browsing(vec![
        record(0, "first", None, None),
        record(1, "second", None, None),
    ]);
    screen.notifications.push(settled_toast(1, "second"));
    let viewport = screen.notification_browser_viewport(client_id).unwrap();
    let now = Instant::now();

    let browser_rects = screen.notification_browser_rects(client_id, viewport);
    assert_eq!(browser_rects.len(), 2);
    // the cursor is kept off both the toast a non-browsing client would see and the list
    let occluded = screen.notification_occlusion_rects(viewport, now);
    for rect in &browser_rects {
        assert!(occluded.contains(rect), "{rect:?} not in {occluded:?}");
    }

    // leaving the mode repaints the list's cells that the single toast no longer covers
    let surface = Size { cols: 80, rows: 30 };
    screen.prepare_notification_damage(now, surface);
    screen
        .change_mode(InputMode::Normal, Some(InputMode::Normal), client_id)
        .expect("TEST");
    let damage: Vec<CellRect> = screen
        .prepare_notification_damage(now, surface)
        .into_iter()
        .flat_map(|(_, rects)| rects)
        .collect();
    assert!(
        !damage.is_empty(),
        "closing the browser left its cells painted"
    );
}

#[test]
fn palette_suspends_history_and_restores_its_selection_and_cells() {
    let (mut screen, client) = browsing(vec![
        record(0, "old history entry", None, None),
        record(1, "new history entry", None, None),
    ]);
    screen.move_notification_selection(Direction::Down, client);
    screen.toggle_notification_detail(client);
    screen.notification_position = NotificationPosition::TopLeft;
    let saved = format!("{:?}", screen.notification_browsers[&client]);
    let viewport = screen.notification_browser_viewport(client).unwrap();
    let hidden_rect = screen.notification_browser_rects(client, viewport)[0];
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(screen.surface_size());
    let (before_cursor, before) =
        render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
    let before_cells = grid.as_character_lines();
    screen.toggle_command_palette(client);
    screen
        .change_mode(InputMode::Locked, Some(InputMode::Normal), client)
        .unwrap();
    let (cursor, opened) = render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
    assert!(opened.contains("Do · Project:"), "{opened}");
    assert!(!opened.contains("history entry"), "{opened}");
    assert!(cursor.is_some());
    assert!(screen.command_palette_handles_input(&None, b"Rename tab", false, client));
    let filtered = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(filtered.contains("> Rename tab"), "{filtered}");
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let mut notification = settled_toast(2, "live toast");
    notification.on_click = Some(click_target("live"));
    screen.notifications.push(notification);
    let toast = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(toast.contains("live toast"), "{toast}");
    assert!(!toast.contains("history entry"), "{toast}");
    let toast_rect = screen.notification_cell_rects(viewport, Instant::now())[0];
    let toast_position = Position::new((toast_rect.y + 1) as i32, (toast_rect.x + 2) as u16);
    screen.handle_mouse_event(MouseEvent::new_left_press_event(toast_position), client);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(toast_position), client);
    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction,
        PluginInstruction::NotificationClicked { payload, .. } if payload == "live")));
    assert!(screen.command_palettes.contains_key(&client));
    screen.notifications.clear();
    // A click over the suspended browser must reach the palette's outside-click dismissal.
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(
            (hidden_rect.y + 1) as i32,
            (hidden_rect.x + 2) as u16,
        )),
        client,
    );
    assert!(!screen.command_palettes.contains_key(&client));
    assert_eq!(
        format!("{:?}", screen.notification_browsers[&client]),
        saved
    );
    screen
        .change_mode(InputMode::Notifications, Some(InputMode::Normal), client)
        .unwrap();
    let (restored_cursor, restored) =
        render_frame_and_cursor_into(&mut screen, &receiver, client, &mut grid);
    assert_eq!(restored, before);
    assert_eq!(grid.as_character_lines(), before_cells);
    assert_eq!(restored_cursor, before_cursor);
}

#[test]
fn palette_history_damage_and_cursor_occlusion_follow_each_clients_visibility() {
    let (mut screen, client) = browsing(vec![record(0, "history", None, None)]);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.open_notification_browser(other);
    let viewport = screen.notification_browser_viewport(client).unwrap();
    let rects = screen.notification_browser_rects(client, viewport);
    let now = Instant::now();
    let size = screen.surface_size();
    screen.prepare_notification_damage(now, size);
    screen.toggle_command_palette(client);
    assert!(!screen.notification_browser_is_visible(client));
    assert!(screen.notification_browser_is_visible(other));
    assert!(!screen.prepare_notification_damage(now, size).is_empty());
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    let other_frame = render_frame_into(&mut screen, &receiver, other, &mut grid);
    assert!(other_frame.contains("history"), "{other_frame}");
    assert!(!other_frame.contains("Do · Project:"), "{other_frame}");
    assert!(!screen.last_notification_fill.contains_key(&client));
    assert_eq!(screen.last_notification_fill[&other], rects);
    assert_eq!(screen.notification_occlusion_rects(viewport, now), rects);
    screen.toggle_command_palette(other);
    assert!(screen
        .notification_occlusion_rects(viewport, now)
        .is_empty());
    screen.close_command_palette(client);
    screen.prepare_notification_damage(now, size);
    assert_eq!(screen.last_notification_fill[&client], rects);
    assert_eq!(screen.notification_occlusion_rects(viewport, now), rects);
}
