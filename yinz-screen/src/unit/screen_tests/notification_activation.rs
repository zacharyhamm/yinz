use super::*;
use crate::output::Output;
use crate::ui::notification::{notification_chunks, MAX_NOTIFICATION_TEXT_ROWS};
use std::time::{Duration, Instant};
use yinz_types::animation::Tween;
use yinz_types::input::options::NotificationPosition;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_size::Viewport;

fn setup() -> (Screen, ClientId, Viewport, Instant) {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 30 });
    let client = ClientId::new(1);
    new_tab(&mut screen, 1, TabId::new(0));
    let mut mode = screen.default_mode_info.clone();
    mode.keybinds = Config::from_default_assets()
        .unwrap()
        .keybinds
        .to_keybinds_vec();
    screen.mode_info.insert(client, mode);
    screen
        .receive_toast(
            "done".into(),
            Some(PaneId::Terminal(TerminalId::new(1))),
            None,
            None,
            None,
        )
        .unwrap();
    let now = Instant::now() + Duration::from_secs(1);
    screen.pinned_now = Some(now);
    let viewport = Viewport {
        x: 0,
        y: 0,
        cols: 80,
        rows: 30,
    };
    (screen, client, viewport, now)
}

fn bind(screen: &mut Screen, client: ClientId, keys: &[KeyWithModifier]) {
    screen
        .mode_info
        .entry(client)
        .or_insert_with(|| screen.default_mode_info.clone())
        .keybinds = vec![(
        InputMode::Normal,
        keys.iter()
            .map(|key| (key.clone(), vec![Action::ActivateLastNotification]))
            .collect(),
    )];
}

#[test]
fn activation_hint_uses_current_clients_direct_binding_in_stable_order() {
    let (mut screen, client, viewport, now) = setup();
    assert_eq!(
        screen.notification_layout(viewport, now, client)[0]
            .footer
            .as_deref(),
        Some("Alt+a to activate")
    );
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    bind(
        &mut screen,
        other,
        &[KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier()],
    );
    assert_eq!(
        screen.notification_layout(viewport, now, other)[0]
            .footer
            .as_deref(),
        Some("Ctrl+ENTER to activate")
    );
    assert_eq!(
        screen.notification_layout(viewport, now, client)[0]
            .footer
            .as_deref(),
        Some("Alt+a to activate")
    );
    let a = KeyWithModifier::new(BareKey::Char('a')).with_alt_modifier();
    let z = KeyWithModifier::new(BareKey::Char('z')).with_ctrl_modifier();
    for keys in [vec![z.clone(), a.clone()], vec![a.clone(), z]] {
        bind(&mut screen, client, &keys);
        assert_eq!(
            screen.notification_layout(viewport, now, client)[0]
                .footer
                .as_deref(),
            Some("Alt+a to activate")
        );
    }
    bind(
        &mut screen,
        client,
        &[KeyWithModifier::new(BareKey::Char(' '))
            .with_ctrl_modifier()
            .with_alt_modifier()],
    );
    assert_eq!(
        screen.notification_layout(viewport, now, client)[0]
            .footer
            .as_deref(),
        Some("Ctrl+Alt+SPACE to activate")
    );
    screen
        .change_mode(InputMode::Locked, Some(InputMode::Normal), client)
        .unwrap();
    assert!(screen.notification_layout(viewport, now, client)[0]
        .footer
        .is_none());
    screen
        .change_mode(InputMode::Normal, Some(InputMode::Normal), client)
        .unwrap();
    assert!(screen.notification_layout(viewport, now, client)[0]
        .footer
        .is_some());
    // A binding reachable only through a mode switch is not a direct activation shortcut.
    screen.mode_info.get_mut(&client).unwrap().keybinds.push((
        InputMode::Locked,
        vec![(
            a,
            vec![Action::SwitchToMode {
                input_mode: InputMode::Normal,
            }],
        )],
    ));
    screen
        .change_mode(InputMode::Locked, Some(InputMode::Normal), client)
        .unwrap();
    assert!(screen.notification_layout(viewport, now, client)[0]
        .footer
        .is_none());
    bind(&mut screen, other, &[]);
    assert!(screen.notification_layout(viewport, now, other)[0]
        .footer
        .is_none());
}

#[test]
fn informational_footer_transfers_when_dismissal_begins() {
    let (mut screen, client, viewport, now) = setup();
    screen
        .receive_toast("newest".into(), None, None, None, None)
        .unwrap();
    let layouts = screen.notification_layout(viewport, now, client);
    assert_eq!(layouts[0].footer.as_deref(), Some("Alt+a to dismiss"));
    assert!(layouts[1].footer.is_none());
    let target = layouts[0].notification.id;
    screen.dismiss_notification(target, now).unwrap();
    let layouts = screen.notification_layout(viewport, now, client);
    assert!(layouts[0].footer.is_none());
    assert_eq!(layouts[1].footer.as_deref(), Some("Alt+a to activate"));
    screen.reap_settled_notifications(now + Duration::from_secs(1));
    let layouts = screen.notification_layout(viewport, now + Duration::from_secs(2), client);
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].footer.as_deref(), Some("Alt+a to activate"));
}

#[test]
fn expired_actionable_target_keeps_the_hint_off_older_toasts_until_consumed() {
    let (mut screen, client, viewport, now) = setup();
    screen
        .receive_toast(
            "newer".into(),
            Some(PaneId::Terminal(TerminalId::new(1))),
            None,
            None,
            None,
        )
        .unwrap();
    screen
        .dismiss_notification(NotificationId::new(1), now)
        .unwrap();
    screen.reap_settled_notifications(now + Duration::from_secs(1));
    assert!(screen.notification_layout(viewport, now, client)[0]
        .footer
        .is_none());
    screen.activate_last_notification(client, now);
    assert_eq!(
        screen.notification_layout(viewport, now, client)[0]
            .footer
            .as_deref(),
        Some("Alt+a to activate")
    );
}

#[test]
fn pending_activation_consumes_newest_first_across_clients_during_exit() {
    let (mut screen, client, viewport, now) = setup();
    new_tab(&mut screen, 2, TabId::new(1));
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen
        .receive_toast(
            "second".into(),
            Some(PaneId::Terminal(TerminalId::new(2))),
            None,
            None,
            None,
        )
        .unwrap();
    screen
        .receive_toast("information".into(), None, None, None, None)
        .unwrap();

    for (id, clicker) in [(2, client), (1, other), (0, client)] {
        assert_eq!(
            screen.pending_notification().unwrap().info.id,
            NotificationId::new(id)
        );
        screen.activate_last_notification(clicker, now);
        assert!(!screen.notification_history[id as usize].pending);
        assert!(screen
            .notification_layout(viewport, now, client)
            .iter()
            .find(|layout| layout.notification.id == NotificationId::new(id))
            .unwrap()
            .footer
            .is_none());
    }
    assert_eq!(
        screen.get_active_pane_id(&other),
        Some(PaneId::Terminal(TerminalId::new(2)))
    );
    assert_eq!(
        screen.get_active_pane_id(&client),
        Some(PaneId::Terminal(TerminalId::new(1)))
    );
    assert!(screen.pending_notification().is_none());
    assert!(screen.notifications.iter().all(|n| n.is_dismissing()));
    screen.activate_last_notification(other, now);
    assert_eq!(
        screen.get_active_pane_id(&other),
        Some(PaneId::Terminal(TerminalId::new(2)))
    );
    assert_eq!(screen.notification_history.len(), 3);
}

#[test]
fn expired_callbacks_and_pane_targets_remain_pending_but_information_does_not() {
    let (mut screen, client, _, now) = setup();
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    for (text, pane) in [
        ("callback only", None),
        (
            "closed pane callback",
            Some(PaneId::Terminal(TerminalId::new(99))),
        ),
    ] {
        screen
            .receive_toast(
                text.into(),
                pane,
                Some(crate::screen::NotificationClick {
                    plugin_id: crate::PluginId::new(7),
                    client_id: client,
                    message: "clicked".into(),
                    payload: text.into(),
                }),
                None,
                None,
            )
            .unwrap();
    }
    screen
        .receive_toast("information".into(), None, None, None, None)
        .unwrap();
    for id in 0..4 {
        screen
            .dismiss_notification(NotificationId::new(id), now)
            .unwrap();
    }
    let expired = now + Duration::from_secs(3600);
    screen.pinned_now = Some(expired);
    screen.reap_settled_notifications(expired);
    assert!(screen.notifications.is_empty());
    for id in (0..3).rev() {
        assert_eq!(
            screen.pending_notification().unwrap().info.id,
            NotificationId::new(id)
        );
        screen.activate_last_notification(client, expired);
    }
    screen.activate_last_notification(client, expired);
    let payloads: Vec<_> = rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::NotificationClicked { payload, .. } => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(payloads, ["closed pane callback", "callback only"]);
    assert!(screen.pending_notification().is_none());
    assert_eq!(screen.notification_history.len(), 4);
}

#[test]
fn closing_a_source_pane_makes_its_notification_informational() {
    for expired in [false, true] {
        let (mut screen, client, viewport, now) = setup();
        new_tab(&mut screen, 2, TabId::new(1));
        let pane = PaneId::Terminal(TerminalId::new(2));
        screen
            .receive_toast("closing".into(), Some(pane), None, None, None)
            .unwrap();
        if expired {
            screen
                .dismiss_notification(NotificationId::new(1), now)
                .unwrap();
            screen.reap_settled_notifications(now + Duration::from_secs(1));
        }
        screen.close_pane_explicitly(pane, None);
        if !expired {
            assert_eq!(
                screen.notification_layout(viewport, now, client)[0]
                    .footer
                    .as_deref(),
                Some("Alt+a to dismiss")
            );
            screen.activate_last_notification(client, now);
        }
        assert_eq!(
            screen.pending_notification().unwrap().info.id,
            NotificationId::new(0)
        );
        screen.activate_last_notification(client, now);
        assert!(screen.pending_notification().is_none());
    }
}

#[test]
fn clicking_or_browsing_an_older_notification_consumes_only_that_entry() {
    for mouse in [false, true] {
        let (mut screen, client, viewport, now) = setup();
        new_tab(&mut screen, 2, TabId::new(1));
        screen
            .receive_toast(
                "newer".into(),
                Some(PaneId::Terminal(TerminalId::new(2))),
                None,
                None,
                None,
            )
            .unwrap();
        if mouse {
            let layout = screen.notification_layout(viewport, now, client);
            let older = layout
                .iter()
                .find(|n| n.notification.id == NotificationId::new(0))
                .unwrap();
            let click = MouseEvent::new_left_press_event(Position::new(
                (older.y + 1) as i32,
                (older.x + 1) as u16,
            ));
            assert!(screen.handle_pane_notification_click(&click, viewport, client, now));
        } else {
            screen.open_notification_browser(client);
            screen.move_notification_selection(Direction::Down, client);
            screen.activate_selected_notification(client, now);
        }
        assert!(!screen.notification_history[0].pending);
        assert_eq!(
            screen.pending_notification().unwrap().info.id,
            NotificationId::new(1)
        );
        screen.activate_last_notification(client, now);
        assert!(screen.pending_notification().is_none());
        screen.open_notification_browser(client);
        screen.move_notification_selection(Direction::Down, client);
        screen.activate_selected_notification(client, now);
        assert_eq!(
            screen.get_active_pane_id(&client),
            Some(PaneId::Terminal(TerminalId::new(1)))
        );
        assert_eq!(screen.notification_history.len(), 2);
    }
}

#[test]
fn footer_geometry_agrees_for_render_damage_cursor_and_click_for_each_client() {
    let (mut screen, client, viewport, now) = setup();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    bind(&mut screen, other, &[]);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(screen.surface_size());
    let painted = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(painted.contains("Alt+a to activate"), "{painted}");
    let mut other_grid = grid_for_render_snapshot(screen.surface_size());
    let other_painted = render_frame_into(&mut screen, &receiver, other, &mut other_grid);
    assert!(!other_painted.contains("to activate"), "{other_painted}");
    let rect = screen.notification_cell_rects(viewport, now, client)[0];
    let other_rect = screen.notification_cell_rects(viewport, now, other)[0];
    assert_eq!(rect.rows, other_rect.rows + 1);
    assert!(rect.cols > other_rect.cols);
    assert_eq!(screen.last_notification_fill[&client], vec![rect]);
    assert_eq!(screen.last_notification_fill[&other], vec![other_rect]);
    let mut output = Output::default();
    output.set_notification_rects(
        [client, other]
            .into_iter()
            .map(|id| (id, screen.notification_occlusion_rects(viewport, now, id)))
            .collect(),
    );
    let (x, y) = (rect.x + 2, rect.y + rect.rows - 1);
    assert!(!output.cursor_is_visible(x, y, None, client));
    assert!(output.cursor_is_visible(x, y, None, other));
    let click = MouseEvent::new_left_press_event(Position::new(y as i32, x as u16));
    assert!(!screen.handle_pane_notification_click(&click, viewport, other, now));
    assert!(screen.handle_pane_notification_click(&click, viewport, client, now));
    assert!(screen.notifications[0].is_dismissing());
}

#[test]
fn mode_change_repairs_cells_uncovered_by_footer() {
    let (mut screen, client, _, _) = setup();
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(screen.surface_size());
    assert!(render_frame_into(&mut screen, &receiver, client, &mut grid).contains("to activate"));
    screen
        .change_mode(InputMode::Locked, Some(InputMode::Normal), client)
        .unwrap();
    let changed = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(!changed.contains("to activate"));
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_should_clear_display_before_rendering();
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_force_render();
    let fresh = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert_eq!(changed, fresh);
}

#[test]
fn consuming_a_toast_transfers_hint_without_jumping_and_repairs_exposed_cells() {
    for corner in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
        NotificationPosition::BottomLeft,
        NotificationPosition::BottomRight,
    ] {
        let (mut screen, client, viewport, now) = setup();
        screen.notification_position = corner;
        screen
            .receive_toast("newer".into(), None, None, None, None)
            .unwrap();
        for notification in &mut screen.notifications {
            notification.y_offset = Tween::settled(0.0);
        }
        let receiver = screen_wired_to_server(&mut screen);
        let mut grid = grid_for_render_snapshot(screen.surface_size());
        assert!(render_frame_into(&mut screen, &receiver, client, &mut grid).contains("to dismiss"));
        let before: Vec<_> = screen
            .notification_layout(viewport, now, client)
            .iter()
            .map(|layout| layout.y)
            .collect();
        screen.activate_last_notification(client, now);
        let layouts = screen.notification_layout(viewport, now, client);
        assert_eq!(
            layouts.iter().map(|layout| layout.y).collect::<Vec<_>>(),
            before
        );
        assert!(layouts[0].footer.is_none());
        assert_eq!(layouts[1].footer.as_deref(), Some("Alt+a to activate"));
        let changed = render_frame_into(&mut screen, &receiver, client, &mut grid);
        assert!(!changed.contains("to dismiss"));
        screen
            .get_active_tab_mut(client)
            .unwrap()
            .set_should_clear_display_before_rendering();
        screen
            .get_active_tab_mut(client)
            .unwrap()
            .set_force_render();
        let fresh = render_frame_into(&mut screen, &receiver, client, &mut grid);
        assert_eq!(changed, fresh, "{corner:?}");
    }
}

#[test]
fn footer_keeps_message_limit_and_stack_displacement_in_all_corners() {
    for corner in [
        NotificationPosition::TopLeft,
        NotificationPosition::TopRight,
        NotificationPosition::BottomLeft,
        NotificationPosition::BottomRight,
    ] {
        let (mut screen, client, viewport, now) = setup();
        screen.notification_position = corner;
        screen.notifications[0].text = "row\n".repeat(20);
        let layout = &screen.notification_layout(viewport, now, client)[0];
        assert_eq!(layout.lines.len(), MAX_NOTIFICATION_TEXT_ROWS);
        assert_eq!(layout.height(), MAX_NOTIFICATION_TEXT_ROWS + 3);
        let chunks = notification_chunks(
            &layout.lines,
            layout.footer.as_deref(),
            layout.x,
            layout.y,
            layout.width,
            &screen.style,
            1.0,
        );
        assert_eq!(chunks.len(), layout.height());
        // Pin a surviving toast at rest, then transfer the footer as a new toast arrives.
        screen.notifications[0].y_offset = Tween::settled(0.0);
        let old_id = screen.notifications[0].id;
        let before = screen.notification_layout(viewport, now, client)[0].y;
        let arrival = now + Duration::from_millis(16);
        screen.with_notification_stack_change(arrival, |screen| {
            let mut record = screen.notification_history.last().unwrap().clone();
            record.info.id = NotificationId::new(99);
            screen.notification_history.push(record);
            screen.notifications.insert(
                0,
                crate::screen::Notification::new(
                    NotificationId::new(99),
                    "next".into(),
                    None,
                    None,
                    arrival,
                ),
            );
        });
        let layouts = screen.notification_layout(viewport, arrival, client);
        assert_eq!(
            layouts
                .iter()
                .find(|layout| layout.notification.id == old_id)
                .unwrap()
                .y,
            before
        );
    }
}
