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
fn activation_footer_stays_with_history_target_after_expiry() {
    let (mut screen, client, viewport, now) = setup();
    screen
        .receive_toast("newest".into(), None, None, None, None)
        .unwrap();
    let layouts = screen.notification_layout(viewport, now, client);
    assert!(layouts[0].footer.is_some());
    assert!(layouts[1].footer.is_none());
    let target = layouts[0].notification.id;
    screen.dismiss_notification(target, now).unwrap();
    screen.reap_settled_notifications(now + Duration::from_secs(1));
    let layouts = screen.notification_layout(viewport, now + Duration::from_secs(2), client);
    assert_eq!(layouts.len(), 1);
    assert!(layouts[0].footer.is_none());
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
