//! `Notifications` mode: the in-place notification browser.

use super::*;
use crate::screen::{Notification, NotificationClick, NotificationRecord};
use crate::ui::damage::CellRect;
use std::time::Instant;
use yinz_types::animation::Tween;
use yinz_types::data::NotificationInfo;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::TabId;

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
        },
        pane_id,
        on_click,
        consumed: false,
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
        message: "hocus_toast_click".to_owned(),
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
fn entering_the_mode_lists_every_unconsumed_notification_newest_first() {
    let mut consumed = record(2, "already clicked", None, None);
    consumed.consumed = true;
    let (screen, client_id) = browsing(vec![
        record(0, "first", None, None),
        record(1, "second", None, None),
        consumed,
    ]);

    assert_eq!(
        listed(&screen, client_id),
        (vec![NotificationId::new(1), NotificationId::new(0)], 0)
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
            "hocus has a plan",
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
            Some(click_target("42")),
        ),
        record(1, "newer", None, None),
    ]);
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    screen
        .notifications
        .push(settled_toast(0, "hocus has a plan"));

    screen.move_notification_selection(Direction::Down, client_id);
    screen.activate_selected_notification(client_id, Instant::now());

    let payload = plugin_rx
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::NotificationClicked { payload, .. } => Some(payload),
            _ => None,
        })
        .expect("the plugin that raised the notification hears about it");
    assert_eq!(payload, "42");
    assert!(screen.notifications[0].is_dismissing());
    assert!(screen.notification_history[0].consumed);
    assert!(!screen.notification_history[1].consumed);
    assert!(!screen.notification_browsers.contains_key(&client_id));
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
}

#[test]
fn activating_a_notification_with_no_target_consumes_it_and_leaves_the_mode() {
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (mut screen, client_id) = browsing(vec![record(0, "just saying", None, None)]);
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));

    screen.activate_selected_notification(client_id, Instant::now());

    assert!(screen.notification_history[0].consumed);
    assert!(!screen.notification_browsers.contains_key(&client_id));
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
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
    assert!(screen.notification_history[0].consumed);
    assert!(!screen.notification_history[1].consumed);
    assert_eq!(mode_changes(&server_rx), vec![InputMode::Normal]);
}

#[test]
fn a_click_outside_the_list_keeps_the_mode() {
    let (mut screen, client_id) = browsing(vec![record(0, "still here", None, None)]);

    let pane_position = Position::new(20, 5);
    screen.handle_mouse_event(MouseEvent::new_left_press_event(pane_position), client_id);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(pane_position), client_id);

    assert!(screen.notification_browsers.contains_key(&client_id));
    assert!(!screen.notification_history[0].consumed);
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
