use super::*;
use crate::output::Output;
use crate::ui::notification::{notification_chunks, MAX_NOTIFICATION_TEXT_ROWS};
use std::time::{Duration, Instant};
use yinz_types::animation::Tween;
use yinz_types::data::{GetFocusedPaneInfoResponse, OverlayPlacement};
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
fn receiving_a_targeted_notification_keeps_focus_until_activation() {
    let (mut screen, client, _viewport, now) = setup();
    new_tab(&mut screen, 2, TabId::new(1));
    let control = PaneId::Terminal(TerminalId::new(1));
    let current = PaneId::Terminal(TerminalId::new(2));
    screen
        .focus_pane_with_id(current, true, false, client)
        .unwrap();
    screen
        .receive_toast(
            "Plan approval needed".into(),
            Some(control),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(screen.get_active_pane_id(&client), Some(current));
    screen.activate_last_notification(client, now);
    assert_eq!(screen.get_active_pane_id(&client), Some(control));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.get_active_pane_id(&client), Some(current));
}

fn activate_target(screen: &mut Screen, client: ClientId, target: PaneId, now: Instant) {
    screen
        .receive_toast("targeted".into(), Some(target), None, None, None)
        .unwrap();
    screen.activate_last_notification(client, now);
}

#[test]
fn notification_return_cross_tab_publishes_final_scroll_mode() {
    for origin_scrolled in [false, true] {
        for initial_mode in [InputMode::Normal, InputMode::Scroll, InputMode::Tab] {
            let (mut screen, client, _, now) = setup();
            let origin_tab = TabId::new(0);
            let origin = PaneId::Terminal(TerminalId::new(1));
            let remembered = PaneId::Terminal(TerminalId::new(2));
            let target = PaneId::Terminal(TerminalId::new(3));
            add_pane_to_tab(&mut screen, origin_tab, remembered, false);
            new_tab(&mut screen, 3, TabId::new(1));
            screen
                .focus_pane_with_id(origin, false, false, client)
                .unwrap();
            activate_target(&mut screen, client, target, now);
            screen
                .focus_pane_with_id(remembered, false, false, client)
                .unwrap();
            screen
                .focus_pane_with_id(target, false, false, client)
                .unwrap();

            for (pane_id, scrolled) in [(origin, origin_scrolled), (remembered, !origin_scrolled)] {
                let pane = screen
                    .tabs
                    .get_mut(&origin_tab)
                    .unwrap()
                    .get_pane_with_id_mut(pane_id)
                    .unwrap();
                pane.handle_pty_bytes(b"scrollback\r\n".repeat(100));
                if scrolled {
                    pane.scroll_up(5, Some(client));
                }
            }
            screen.change_mode(initial_mode, None, client).unwrap();
            let server = screen_wired_to_server(&mut screen);

            screen.return_from_notification(client).unwrap();

            assert_eq!(screen.active_pane_id(client), Some(origin));
            let expected_mode = match (initial_mode, origin_scrolled) {
                (InputMode::Tab, _) => InputMode::Tab,
                (_, true) => InputMode::Scroll,
                (_, false) => InputMode::Normal,
            };
            let mode_requests: Vec<_> = server
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    ServerInstruction::ChangeMode(client, mode, _) => Some((client, mode)),
                    _ => None,
                })
                .collect();
            // Inspect the requests before simulating any server reply.
            assert_eq!(
                mode_requests,
                if expected_mode != initial_mode {
                    vec![(client, expected_mode)]
                } else {
                    vec![]
                },
                "origin scrolled: {origin_scrolled}, initial mode: {initial_mode:?}"
            );
        }
    }
}

#[test]
fn cross_tab_notification_focus_publishes_shared_layer_changes_for_a_destination_viewer() {
    let (mut screen, client, _, now) = setup();
    let other = ClientId::new(2);
    let origin = PaneId::Terminal(TerminalId::new(1));
    let target = PaneId::Terminal(TerminalId::new(2));
    let floating = PaneId::Terminal(TerminalId::new(3));
    let target_tab = TabId::new(1);
    new_tab(&mut screen, 2, target_tab);
    screen.add_client(other).unwrap();
    screen
        .focus_pane_with_id(origin, false, false, client)
        .unwrap();
    add_pane_to_tab(&mut screen, target_tab, floating, true);
    screen
        .focus_pane_with_id(floating, false, false, other)
        .unwrap();
    assert_eq!(screen.active_pane_id(other), Some(floating));
    let server = screen_wired_to_server(&mut screen);

    activate_target(&mut screen, client, target, now);

    assert_eq!(screen.active_pane_id(client), Some(target));
    assert_eq!(screen.active_pane_id(other), Some(target));
    let peer_updates: Vec<_> = server
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::KeyPassthroughChanged(id, old, new, route, _, _) if id == other => {
                Some((old, new, route))
            },
            _ => None,
        })
        .collect();
    assert_eq!(peer_updates, [(floating, target, false)]);
    assert_eq!(screen.notification_return_panes.get(&client), Some(&origin));
    assert!(!screen.notification_return_panes.contains_key(&other));
}

#[test]
fn notification_return_uses_actual_focus_when_the_target_cannot_take_it() {
    let (mut screen, client, _, now) = setup();
    let first = PaneId::Terminal(TerminalId::new(1));
    let second = PaneId::Terminal(TerminalId::new(2));
    let modal = PaneId::Terminal(TerminalId::new(3));
    let refused = PaneId::Terminal(TerminalId::new(4));
    let second_tab = TabId::new(1);
    new_tab(&mut screen, 2, second_tab);
    screen
        .focus_pane_with_id(first, false, false, client)
        .unwrap();

    activate_target(&mut screen, client, second, now);
    assert_eq!(screen.active_pane_id(client), Some(second));
    screen
        .tabs
        .get_mut(&second_tab)
        .unwrap()
        .new_pane(
            modal,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
            Some(client),
            None,
        )
        .unwrap();
    assert_eq!(screen.active_pane_id(client), Some(modal));
    activate_target(&mut screen, client, second, now);
    assert_eq!(screen.active_pane_id(client), Some(modal));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(first));

    screen.close_pane_explicitly(modal, None);
    add_pane_to_tab(&mut screen, second_tab, refused, false);
    assert!(screen.set_pane_selectable(refused, false));
    screen
        .focus_pane_with_id(second, false, false, client)
        .unwrap();
    screen
        .focus_pane_with_id(first, false, false, client)
        .unwrap();
    // First establish a different saved origin so that recording the switch is observable.
    activate_target(&mut screen, client, second, now);
    activate_target(&mut screen, client, first, now);
    assert_eq!(screen.notification_return_panes.get(&client), Some(&second));
    activate_target(&mut screen, client, refused, now);
    assert_eq!(screen.active_pane_id(client), Some(second));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(first));
}

#[test]
fn notification_return_slots_are_isolated_and_cleared_when_leaving_interactive_use() {
    let (mut screen, client, _, now) = setup();
    let other = ClientId::new(2);
    let first = PaneId::Terminal(TerminalId::new(1));
    let second = PaneId::Terminal(TerminalId::new(2));
    let third = PaneId::Terminal(TerminalId::new(3));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    screen.add_client(other).unwrap();
    screen
        .focus_pane_with_id(first, false, false, client)
        .unwrap();
    screen
        .focus_pane_with_id(second, false, false, other)
        .unwrap();
    activate_target(&mut screen, client, third, now);
    activate_target(&mut screen, other, third, now);
    assert_eq!(screen.notification_return_panes.get(&client), Some(&first));
    assert_eq!(screen.notification_return_panes.get(&other), Some(&second));

    activate_target(&mut screen, other, third, now);
    screen
        .receive_toast("informational".into(), None, None, None, None)
        .unwrap();
    screen.activate_last_notification(other, now);
    activate_target(
        &mut screen,
        other,
        PaneId::Terminal(TerminalId::new(99)),
        now,
    );
    screen
        .receive_toast(
            "callback only".into(),
            None,
            Some(crate::screen::NotificationClick {
                plugin_id: crate::PluginId::new(7),
                client_id: other,
                message: "clicked".into(),
                payload: String::new(),
            }),
            None,
            None,
        )
        .unwrap();
    screen.activate_last_notification(other, now);
    assert_eq!(screen.notification_return_panes.get(&other), Some(&second));

    screen.add_watcher_client(client).unwrap();
    assert!(!screen.notification_return_panes.contains_key(&client));
    activate_target(&mut screen, client, first, now);
    assert!(!screen.notification_return_panes.contains_key(&client));
    screen.remove_watcher_client(client);
    screen.return_from_notification(other).unwrap();
    assert_eq!(screen.active_pane_id(other), Some(second));
    screen.remove_client(other).unwrap();
    assert!(screen.notification_return_panes.is_empty());
}

#[test]
fn notification_return_follows_an_origin_moved_to_another_group_and_can_repeat() {
    let (mut screen, client, _, now) = setup();
    let origin = PaneId::Terminal(TerminalId::new(4));
    let target = PaneId::Terminal(TerminalId::new(2));
    let new_home = TabId::new(2);
    add_pane_to_tab(&mut screen, TabId::new(0), origin, false);
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, new_home);
    screen.restore_tab_group(
        new_home,
        &TiledPaneLayout {
            tab_group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 1,
                name: "New home".into(),
            }),
            ..Default::default()
        },
    );
    screen
        .focus_pane_with_id(origin, false, false, client)
        .unwrap();
    activate_target(&mut screen, client, target, now);
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![origin], new_home, false, client)
        .unwrap();
    assert_eq!(outcome.moved, vec![origin]);
    for _ in 0..2 {
        screen
            .focus_pane_with_id(target, false, false, client)
            .unwrap();
        screen.return_from_notification(client).unwrap();
        assert_eq!(screen.active_pane_id(client), Some(origin));
        assert_eq!(screen.get_active_tab(client).unwrap().id, new_home);
        assert_eq!(screen.group_for_client(Some(client)), 1);
    }
}

#[test]
fn notification_return_leaves_unavailable_panes_in_place() {
    let (mut screen, client, _, now) = setup();
    let origin = PaneId::Terminal(TerminalId::new(3));
    let target = PaneId::Terminal(TerminalId::new(2));
    let origin_tab = TabId::new(0);
    add_pane_to_tab(&mut screen, origin_tab, origin, false);
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .focus_pane_with_id(origin, false, false, client)
        .unwrap();
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(origin));
    activate_target(&mut screen, client, target, now);
    screen
        .tabs
        .get_mut(&origin_tab)
        .unwrap()
        .suppress_pane(origin, Some(client));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(target));
    assert!(screen.tabs[&origin_tab].has_pane_with_pid(&origin));
    assert!(screen.tabs[&origin_tab].rendered_pane(origin).is_none());

    screen
        .tabs
        .get_mut(&origin_tab)
        .unwrap()
        .unsuppress_pane(origin, false);
    assert!(screen.set_pane_selectable(origin, false));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(target));
    assert!(screen.set_pane_selectable(origin, true));
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(origin));

    screen
        .focus_pane_with_id(target, false, false, client)
        .unwrap();
    screen.close_pane_explicitly(origin, None);
    screen.return_from_notification(client).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(target));
    assert!(!screen.notification_return_panes.contains_key(&client));
}

fn assert_mock_focus(mock: &MockScreen, tab_id: TabId, pane_id: PaneId) {
    let (response_channel, response) = channels::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetFocusedPaneInfo {
            client_id: mock.main_client_id,
            response_channel,
        })
        .unwrap();
    assert_eq!(
        response.recv_timeout(Duration::from_secs(10)).unwrap(),
        GetFocusedPaneInfoResponse::Ok { tab_id, pane_id }
    );
}

#[test]
fn notification_return_renders_quiet_same_tab_origin() {
    let size = Size { cols: 80, rows: 24 };
    let mut mock = MockScreen::new(size);
    let (jobs_tx, jobs) = channels::unbounded();
    mock.to_background_jobs = SenderWithContext::new(jobs_tx);
    let server = mock.server_receiver.take().unwrap();
    let screen_thread = mock.run(None, vec![]);
    let client = mock.main_client_id;
    let tab = TabId::new(0);
    let target_terminal = TerminalId::new(0);
    let origin_terminal = TerminalId::new(1);
    let target = PaneId::Terminal(target_terminal);
    let origin = PaneId::Terminal(origin_terminal);
    let send = |instruction| mock.to_screen.send(instruction).unwrap();

    send(ScreenInstruction::NewPane(
        origin,
        None,
        None,
        None,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(client),
        None,
        false,
    ));
    send(ScreenInstruction::PtyBytes(
        origin_terminal,
        b"origin prompt".to_vec(),
    ));
    send(ScreenInstruction::PtyBytes(
        target_terminal,
        b"target prompt".to_vec(),
    ));
    send(ScreenInstruction::RenderToClients);
    assert_mock_focus(&mock, tab, origin);
    let mut render_events: Vec<_> = server
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    let origin_cursor =
        take_snapshots_and_cursor_coordinates_from_render_events(render_events.iter(), size)
            .into_iter()
            .find(|(cursor, frame)| {
                cursor.is_some()
                    && frame.contains("origin prompt")
                    && frame.contains("target prompt")
            })
            .expect("origin baseline renders both quiet panes")
            .0;

    send(ScreenInstruction::Notify(
        "targeted".into(),
        Some(target),
        None,
        None,
    ));
    send(ScreenInstruction::ActivateLastNotification(client, None));
    send(ScreenInstruction::RenderToClients);
    assert_mock_focus(&mock, tab, target);
    render_events.extend(server.try_iter().map(|(instruction, _)| instruction));
    let baseline =
        take_snapshots_and_cursor_coordinates_from_render_events(render_events.iter(), size);
    assert!(baseline.iter().any(|(cursor, frame)| {
        cursor.is_some() && *cursor != origin_cursor && frame.contains("target prompt")
    }));
    let _ = jobs.try_iter().count();

    send(ScreenInstruction::ReturnFromNotification(client, None));
    assert_mock_focus(&mock, tab, origin);
    // Only the return's render request is serviced. No animation ticks or PTY bytes can help.
    for (job, _) in jobs.try_iter() {
        if matches!(job, BackgroundJob::RenderToClients) {
            send(ScreenInstruction::RenderToClients);
        }
    }
    assert_mock_focus(&mock, tab, origin);
    render_events.extend(server.try_iter().map(|(instruction, _)| instruction));
    let snapshots =
        take_snapshots_and_cursor_coordinates_from_render_events(render_events.iter(), size);
    assert!(
        snapshots
            .iter()
            .skip(baseline.len())
            .any(|(cursor, frame)| { *cursor == origin_cursor && frame.contains("origin prompt") }),
        "the return must schedule a fresh frame with the origin cursor"
    );
    mock.teardown(vec![screen_thread]);
}

#[test]
fn forwarded_desktop_clicks_save_focus_with_and_without_a_report() {
    for (response, expected_report) in [
        (
            b"i=p0r.click;".as_slice(),
            Some(b"\x1b]99;i=click;\x1b\\".as_slice()),
        ),
        (b"i=p0.click;".as_slice(), None),
    ] {
        assert_forwarded_desktop_return(response, expected_report);
    }
}

fn assert_forwarded_desktop_return(response: &[u8], expected_report: Option<&[u8]>) {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock.run(None, vec![]);
    let client = mock.main_client_id;
    let tab = TabId::new(0);
    let destination = TerminalId::new(0);
    let target = PaneId::Terminal(destination);
    let origin = PaneId::Terminal(TerminalId::new(1));
    let pty_writer = mock.pty_writer_receiver.take().unwrap();
    mock.to_screen
        .send(ScreenInstruction::NewPane(
            origin,
            None,
            None,
            None,
            NewPanePlacement::default(),
            false,
            ClientTabIndexOrPaneId::ClientId(client),
            None,
            false,
        ))
        .unwrap();
    assert_mock_focus(&mock, tab, origin);
    mock.to_screen
        .send(ScreenInstruction::ReturnFromNotification(client, None))
        .unwrap();
    assert_mock_focus(&mock, tab, origin);

    let respond = |response: &[u8]| {
        mock.to_screen
            .send(ScreenInstruction::DesktopNotificationResponse(
                response.to_vec(),
                client,
            ))
            .unwrap();
    };
    let reports = || {
        pty_writer
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyWriteInstruction::Write(bytes, id, _) if bytes.starts_with(b"\x1b]99;") => {
                    Some((bytes, id))
                },
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    respond(response);
    assert_mock_focus(&mock, tab, target);
    assert_eq!(
        reports(),
        expected_report
            .map(|bytes| (bytes.to_vec(), destination))
            .into_iter()
            .collect::<Vec<_>>()
    );
    // The query is written to the other pane without moving focus or replacing the origin.
    respond(b"i=p1.query:p=?;reply");
    assert_mock_focus(&mock, tab, target);
    assert_eq!(
        reports(),
        vec![(
            b"\x1b]99;i=query:p=?;reply\x1b\\".to_vec(),
            TerminalId::new(1)
        )]
    );
    mock.to_screen
        .send(ScreenInstruction::ReturnFromNotification(client, None))
        .unwrap();
    assert_mock_focus(&mock, tab, origin);
    mock.teardown(vec![screen_thread]);
}

#[test]
fn notification_return_waits_for_a_fresh_press_after_a_pending_layout() {
    for cancel_layout in [false, true] {
        assert_return_after_pending_layout(cancel_layout);
    }
}

fn assert_return_after_pending_layout(cancel_layout: bool) {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    mock.config.options.pane_frames = Some(false);
    let server = mock.server_receiver.take().unwrap();
    let screen_thread = mock.run(None, vec![]);
    let client = mock.main_client_id;
    let other = ClientId::new(2);
    let active_tab = TabId::new(0);
    let pending_tab = TabId::new(1);
    let target = PaneId::Terminal(TerminalId::new(0));
    let origin = PaneId::Terminal(TerminalId::new(1));
    let send = |instruction| mock.to_screen.send(instruction).unwrap();
    send(ScreenInstruction::NewPane(
        origin,
        None,
        None,
        None,
        NewPanePlacement::default(),
        false,
        ClientTabIndexOrPaneId::ClientId(client),
        None,
        false,
    ));
    assert_mock_focus(&mock, active_tab, origin);
    send(ScreenInstruction::AddClient(
        other,
        Size { cols: 80, rows: 24 },
        None,
        None,
    ));
    send(ScreenInstruction::Notify(
        "targeted".into(),
        Some(target),
        None,
        None,
    ));
    send(ScreenInstruction::ActivateLastNotification(client, None));
    assert_mock_focus(&mock, active_tab, target);

    send(ScreenInstruction::NewTab(
        None,
        None,
        Some(TiledPaneLayout::default()),
        vec![],
        None,
        (Some(vec![]), Some(vec![])),
        None,
        false,
        false,
        client,
        None,
    ));
    send(ScreenInstruction::ReturnFromNotification(client, None));
    send(ScreenInstruction::RenderToClients);
    assert_mock_focus(&mock, active_tab, target);
    let cue = "Layout loading. Try the return shortcut again.";
    let mut seen_by_client = false;
    for (instruction, _) in server.try_iter() {
        if let ServerInstruction::Render(Some(clients)) = instruction {
            seen_by_client |= clients
                .get(&client)
                .is_some_and(|output| output.contains(cue));
            assert!(!clients
                .get(&other)
                .is_some_and(|output| output.contains(cue)));
        }
    }
    assert!(
        seen_by_client,
        "the invoking client sees the cue without pane frames"
    );
    let (response_channel, response) = channels::bounded(1);
    send(ScreenInstruction::ListNotifications { response_channel });
    let history = response.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(history.len(), 1);
    if cancel_layout {
        send(ScreenInstruction::CloseTabWithId(pending_tab, None));
    } else {
        send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(2), None)],
            vec![],
            vec![],
            HashMap::new(),
            pending_tab,
            false,
            client,
            None,
            None,
        ));
    }
    assert_mock_focus(&mock, active_tab, target);
    send(ScreenInstruction::ReturnFromNotification(client, None));
    assert_mock_focus(&mock, active_tab, origin);
    mock.teardown(vec![screen_thread]);
}

#[test]
fn notification_return_retry_cue_refreshes_expires_and_clears_after_success() {
    let (mut screen, client, _, now) = setup();
    let tab = TabId::new(0);
    let pending = TabId::new(1);
    let origin = PaneId::Terminal(TerminalId::new(1));
    let target = PaneId::Terminal(TerminalId::new(2));
    add_pane_to_tab(&mut screen, tab, target, false);
    activate_target(&mut screen, client, target, now);
    screen
        .new_tab(pending, (vec![], vec![]), None, None)
        .unwrap();
    screen
        .change_mode(InputMode::Notifications, None, client)
        .unwrap();
    assert!(screen.notification_browser_is_visible(client));

    let server = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(Size { cols: 80, rows: 30 });
    let cue = "Layout loading. Try the return shortcut again.";
    screen.return_from_notification(client).unwrap();
    let first_deadline = screen.notification_return_retry_until[&client];
    assert!(render_frame_into(&mut screen, &server, client, &mut grid).contains(cue));

    screen.pinned_now = Some(now + Duration::from_secs(1));
    screen.return_from_notification(client).unwrap();
    let refreshed_deadline = screen.notification_return_retry_until[&client];
    assert!(refreshed_deadline > first_deadline);
    screen.pinned_now = Some(first_deadline);
    assert!(render_frame_into(&mut screen, &server, client, &mut grid).contains(cue));
    screen.pinned_now = Some(refreshed_deadline);
    screen.on_animation_tick(refreshed_deadline).unwrap();
    let expired = render_frame_into(&mut screen, &server, client, &mut grid);
    assert!(
        !expired.is_empty() && !expired.contains(cue),
        "cue cells must repaint on expiry"
    );
    assert!(!screen.notification_return_retry_until.contains_key(&client));
    assert_eq!(screen.active_pane_id(client), Some(target));

    screen.return_from_notification(client).unwrap();
    assert!(render_frame_into(&mut screen, &server, client, &mut grid).contains(cue));
    screen.close_tab_by_id(pending).unwrap();
    assert_eq!(screen.active_pane_id(client), Some(target));
    screen.return_from_notification(client).unwrap();
    let returned = render_frame_into(&mut screen, &server, client, &mut grid);
    assert!(
        !returned.is_empty() && !returned.contains(cue),
        "retry clears the cue cells"
    );
    assert_eq!(screen.active_pane_id(client), Some(origin));
    assert!(screen.notification_browser_is_visible(client));
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
        let origin = PaneId::Terminal(TerminalId::new(2));
        let target = PaneId::Terminal(TerminalId::new(1));
        screen
            .focus_pane_with_id(origin, false, false, client)
            .unwrap();
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
        assert_eq!(screen.active_pane_id(client), Some(target));
        screen.return_from_notification(client).unwrap();
        assert_eq!(screen.active_pane_id(client), Some(origin));
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
