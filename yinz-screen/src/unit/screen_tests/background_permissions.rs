use super::*;
use crate::PluginId;
use yinz_types::data::{PaneInfo, PermissionStatus, PermissionType, PluginPermission};

const PLUGIN: PluginId = PluginId::new(42);
const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size {
    cols: 100,
    rows: 30,
};

fn request(mock: &MockScreen) {
    mock.to_screen
        .send(ScreenInstruction::RequestPluginPermissions(
            PLUGIN,
            PluginPermission::new(
                "background-permission-fixture".into(),
                vec![PermissionType::ReadApplicationState],
            ),
        ))
        .unwrap();
}

fn attach(mock: &MockScreen, tab: TabId) {
    mock.to_screen
        .send(ScreenInstruction::AddPlugin(
            None,
            false,
            yinz_bus::pty::ReplacedPane::Suppress,
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/background-permission-fixture",
                &None,
                None,
                None,
            )
            .unwrap(),
            None,
            Some(tab),
            PLUGIN,
            None,
            None,
            true,
            None,
            Some(false),
            Some(CLIENT),
            None,
            None,
        ))
        .unwrap();
}

// Also a dispatcher barrier: every instruction sent before this query has been handled.
fn pane_info(mock: &MockScreen, pane_id: PaneId) -> Option<PaneInfo> {
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetPaneInfo {
            pane_id,
            response_channel: tx,
        })
        .unwrap();
    rx.recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
}

fn start() -> (
    MockScreen,
    Watched<Vec<ServerInstruction>>,
    Vec<std::thread::JoinHandle<()>>,
) {
    let mut mock = MockScreen::new(SIZE);
    let renders = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let logger = log_actions_in_thread!(renders, ServerInstruction::KillSession, receiver);
    let screen = mock.run(None, vec![]);
    (mock, renders, vec![screen, logger])
}

fn assert_prompt(renders: &Watched<Vec<ServerInstruction>>) {
    wait_for_settled_render(
        renders,
        SIZE,
        "background permission prompt without input",
        |frame| frame.contains("Plugin permissions"),
    );
}

fn focused_pane(mock: &MockScreen, client_id: ClientId) -> (TabId, PaneId) {
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetFocusedPaneInfo {
            client_id,
            response_channel: tx,
        })
        .unwrap();
    match rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
    {
        yinz_types::data::GetFocusedPaneInfoResponse::Ok { tab_id, pane_id } => (tab_id, pane_id),
        response => panic!("unexpected focus response: {response:?}"),
    }
}

#[test]
fn background_permission_clients_and_inactive_tab() {
    let (mut mock, renders, threads) = start();
    mock.to_screen
        .send(ScreenInstruction::NewTab(
            None,
            None,
            Some(TiledPaneLayout::default()),
            vec![],
            None,
            (Some(vec![]), Some(vec![])),
            None,
            false,
            true,
            CLIENT,
            None,
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(1), None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(1),
            true,
            CLIENT,
            None,
            None,
        ))
        .unwrap();
    attach(&mock, TabId::new(1));
    mock.to_screen
        .send(ScreenInstruction::GoToTabWithId(
            TabId::new(0),
            Some(CLIENT),
            None,
        ))
        .unwrap();
    let other = ClientId::new(2);
    mock.to_screen
        .send(ScreenInstruction::AddClient(
            other,
            SIZE,
            Some(yinz_types::internal_ids::TabNumber::new(2)),
            None,
        ))
        .unwrap();
    request(&mock);
    assert_eq!(
        focused_pane(&mock, CLIENT),
        (TabId::new(0), PaneId::Terminal(TerminalId::new(0))),
        "a background request must not switch an unrelated client away from its tab"
    );
    assert_eq!(
        focused_pane(&mock, other),
        (TabId::new(1), PaneId::Terminal(TerminalId::new(1)))
    );
    mock.to_screen
        .send(ScreenInstruction::GoToTabWithId(
            TabId::new(1),
            Some(CLIENT),
            None,
        ))
        .unwrap();
    assert_prompt(&renders);
    assert_eq!(
        focused_pane(&mock, CLIENT),
        (TabId::new(1), PaneId::Terminal(TerminalId::new(1)))
    );
    let arriving = ClientId::new(3);
    mock.to_screen
        .send(ScreenInstruction::AddClient(
            arriving,
            SIZE,
            Some(yinz_types::internal_ids::TabNumber::new(2)),
            None,
        ))
        .unwrap();
    assert_eq!(
        focused_pane(&mock, arriving),
        (TabId::new(1), PaneId::Terminal(TerminalId::new(1)))
    );
    mock.teardown(threads);
}

#[test]
fn background_permission_survives_initial_layout() {
    for hide_floating_panes in [false, true] {
        let (mut mock, renders, threads) = start();
        let layout = TiledPaneLayout {
            hide_floating_panes,
            ..Default::default()
        };
        mock.to_screen
            .send(ScreenInstruction::NewTab(
                None,
                None,
                Some(layout.clone()),
                vec![],
                None,
                (Some(vec![]), Some(vec![])),
                None,
                false,
                true,
                CLIENT,
                None,
            ))
            .unwrap();
        attach(&mock, TabId::new(1));
        request(&mock);
        assert!(
            pane_info(&mock, PaneId::Plugin(PLUGIN))
                .unwrap()
                .is_suppressed,
            "permission presentation keeps the pane suppressed"
        );
        mock.to_screen
            .send(ScreenInstruction::ApplyLayout(
                layout,
                vec![],
                vec![(TerminalId::new(1), None)],
                vec![],
                vec![],
                HashMap::new(),
                TabId::new(1),
                true,
                CLIENT,
                None,
                None,
            ))
            .unwrap();
        assert_prompt(&renders);
        assert_ne!(focused_pane(&mock, CLIENT).1, PaneId::Plugin(PLUGIN));
        mock.teardown(threads);
    }
}

#[test]
fn background_permission_replays_on_pane_attachment() {
    let (mut mock, renders, threads) = start();
    request(&mock);
    assert!(pane_info(&mock, PaneId::Plugin(PLUGIN)).is_none());
    attach(&mock, TabId::new(0));
    assert_prompt(&renders);
    assert_ne!(focused_pane(&mock, CLIENT).1, PaneId::Plugin(PLUGIN));
    mock.teardown(threads);
}

#[test]
fn background_permission_schedules_render_when_idle() {
    let (mut mock, renders, threads) = start();
    attach(&mock, TabId::new(0));
    assert!(
        pane_info(&mock, PaneId::Plugin(PLUGIN))
            .unwrap()
            .is_suppressed
    );
    // Fence the background queue, then the screen queue. All preceding render jobs have
    // returned to the dispatcher before the permission request; there is no animation clock
    // in this mock to hide a missing render request.
    mock.to_background_jobs
        .send(BackgroundJob::WatchCommandPaletteSessions(true))
        .unwrap();
    mock.received_background_jobs.wait_until(
        "background queue drained",
        yinz_test_support::default_timeout(),
        |jobs| {
            jobs.iter()
                .any(|job| matches!(job, BackgroundJob::WatchCommandPaletteSessions(true)))
                .then_some(())
        },
    );
    pane_info(&mock, PaneId::Plugin(PLUGIN));
    let checkpoint = mock.received_background_jobs.lock().len();
    request(&mock);
    mock.received_background_jobs.wait_until(
        "permission schedules a render",
        yinz_test_support::default_timeout(),
        |jobs| {
            jobs[checkpoint..]
                .iter()
                .any(|job| matches!(job, BackgroundJob::RenderToClients))
                .then_some(())
        },
    );
    assert_prompt(&renders);
    mock.teardown(threads);
}

#[test]
fn background_permission_restores_placement() {
    let (mut mock, renders, mut threads) = start();
    let answers = Watched::new(vec![]);
    let receiver = mock.plugin_receiver.take().unwrap();
    threads.push(log_actions_in_thread!(
        answers,
        PluginInstruction::Exit,
        receiver
    ));
    attach(&mock, TabId::new(0));
    for (key, expected) in [
        (yinz_types::keys::BareKey::Enter, PermissionStatus::Granted),
        (yinz_types::keys::BareKey::Esc, PermissionStatus::Denied),
    ] {
        let checkpoint = renders.lock().len();
        request(&mock);
        wait_for_render_after(
            &renders,
            SIZE,
            checkpoint,
            "fresh permission prompt",
            |_, frame| frame.contains("Plugin permissions"),
        );
        if expected == PermissionStatus::Granted {
            mock.to_screen
                .send(ScreenInstruction::WriteCharacter(
                    Some(yinz_types::keys::KeyWithModifier::new(
                        yinz_types::keys::BareKey::Tab,
                    )),
                    vec![],
                    false,
                    CLIENT,
                    None,
                ))
                .unwrap();
        }
        mock.to_screen
            .send(ScreenInstruction::WriteCharacter(
                Some(yinz_types::keys::KeyWithModifier::new(key)),
                vec![],
                false,
                CLIENT,
                None,
            ))
            .unwrap();
        answers.wait_until(
            "permission answer reaches host",
            yinz_test_support::default_timeout(),
            |events| {
                events
                    .iter()
                    .any(|event| {
                        matches!(event,
                            PluginInstruction::PermissionAnswer { request_id: 0, status, .. }
                            if *status == expected
                        )
                    })
                    .then_some(())
            },
        );
        assert!(
            pane_info(&mock, PaneId::Plugin(PLUGIN))
                .unwrap()
                .is_suppressed
        );
        assert_eq!(
            focused_pane(&mock, CLIENT).1,
            PaneId::Terminal(TerminalId::new(0))
        );
        wait_for_render_after(
            &renders,
            SIZE,
            checkpoint,
            "answered prompt clears",
            |_, frame| !frame.contains("Plugin permissions"),
        );
    }
    mock.teardown(threads);
}

#[test]
fn permission_modals_queue_reassign_and_consume_paste_without_changing_mode() {
    use yinz_types::keys::{BareKey, KeyWithModifier};
    let mut screen = create_non_mirrored_screen(SIZE);
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let mut first = PluginPermission::new(
        "first.scm".into(),
        vec![PermissionType::ReadApplicationState],
    );
    first.request_id = 1;
    screen.open_permission_modal(PLUGIN, first.clone());
    assert!(
        !screen.permission_modal_open(CLIENT),
        "startup waits for an interactive client"
    );
    new_tab(&mut screen, 1, TabId::new(0));
    screen.assign_permission_modals();
    assert!(screen.permission_modal_open(CLIENT));
    let focus = screen.get_active_pane_id(&CLIENT);
    let mode = screen.get_client_input_mode(CLIENT);
    let mut second = first.clone();
    second.request_id = 2;
    screen.open_permission_modal(PLUGIN, second);
    screen.open_permission_modal(PLUGIN, first); // duplicate delivery is harmless
    assert_eq!(screen.permission_modals.len(), 2);
    assert!(screen.command_palette_handles_input(&None, b"y\nAllow\r", false, CLIENT));
    assert_eq!(screen.permission_modals.len(), 2, "paste is not approval");
    screen.permission_modal_input(&Some(KeyWithModifier::new(BareKey::Enter)), CLIENT);
    assert!(rx.try_iter().any(|(event, _)| matches!(
        event,
        PluginInstruction::PermissionAnswer {
            request_id: 1,
            status: PermissionStatus::Denied,
            ..
        }
    )));
    assert!(screen.permission_modal_open(CLIENT));
    assert_eq!(screen.get_active_pane_id(&CLIENT), focus);
    assert_eq!(screen.get_client_input_mode(CLIENT), mode);
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    screen.remove_client(CLIENT).unwrap();
    screen.assign_permission_modals();
    assert!(screen.permission_modal_open(other));
    assert!(!screen.permission_modal_open(CLIENT));
    screen.permission_modal_input(&Some(KeyWithModifier::new(BareKey::Tab)), other);
    screen.permission_modal_input(&Some(KeyWithModifier::new(BareKey::Enter)), other);
    assert!(rx.try_iter().any(|(event, _)| matches!(
        event,
        PluginInstruction::PermissionAnswer {
            request_id: 2,
            status: PermissionStatus::Granted,
            ..
        }
    )));
    assert!(screen.permission_modals.is_empty());
}

#[test]
fn permission_modal_mouse_tracks_resizing_and_absorbs_release_after_cancellation() {
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;
    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let focus = screen.get_active_pane_id(&CLIENT);
    for (token, size) in [(1, SIZE), (2, Size { cols: 60, rows: 20 })] {
        screen.resize_to_screen(size).unwrap();
        let mut request = PluginPermission::new(
            "mouse.scm".into(),
            vec![PermissionType::ReadApplicationState],
        );
        request.request_id = token;
        screen.open_permission_modal(PLUGIN, request);
        screen.prepare_permission_modal_render();
        // Two identity rows, one permission, and two controls, inside a one-cell border.
        let allow = Position::new(((size.rows - 7) / 2 + 5) as i32, (size.cols / 2) as u16);
        assert!(screen.permission_modal_mouse(&MouseEvent::new_left_press_event(allow), CLIENT));
        if token == 1 {
            screen.cancel_permission_modal(token);
            assert!(
                screen.permission_modal_mouse(&MouseEvent::new_left_release_event(allow), CLIENT)
            );
            assert!(
                !screen.permission_modal_mouse(&MouseEvent::new_left_release_event(allow), CLIENT)
            );
        } else {
            assert!(
                screen.permission_modal_mouse(&MouseEvent::new_left_release_event(allow), CLIENT)
            );
        }
        assert_eq!(screen.get_active_pane_id(&CLIENT), focus);
    }
    let answers: Vec<_> = rx
        .try_iter()
        .filter_map(|(event, _)| match event {
            PluginInstruction::PermissionAnswer {
                request_id, status, ..
            } => Some((request_id, status)),
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![(2, PermissionStatus::Granted)]);
}

#[test]
fn permission_modal_navigation_hints_and_answers_match_selection() {
    use yinz_types::keys::{BareKey, KeyWithModifier};

    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let assert_hints = |screen: &Screen, allow: bool| {
        let layout =
            screen.permission_modals[0].layout(screen.command_palette_surface(CLIENT).unwrap());
        let deny = layout.rows.iter().find(|row| row.label == "Deny").unwrap();
        let grant = layout.rows.iter().find(|row| row.label == "Allow").unwrap();
        assert_eq!(
            deny.shortcut.as_deref(),
            Some(if allow { "Esc" } else { "Esc / Enter" })
        );
        assert_eq!(grant.shortcut.as_deref(), allow.then_some("Enter"));
    };
    for (navigate, answer, expected) in [
        (false, BareKey::Enter, PermissionStatus::Denied),
        (true, BareKey::Enter, PermissionStatus::Granted),
        (true, BareKey::Esc, PermissionStatus::Denied),
    ] {
        screen.open_permission_modal(
            PLUGIN,
            PluginPermission::new("identity.scm".into(), vec![PermissionType::RunCommands]),
        );
        assert_hints(&screen, false);
        if navigate {
            for (key, allow) in [
                (KeyWithModifier::new(BareKey::Up), false),
                (KeyWithModifier::new(BareKey::Right), false),
                (KeyWithModifier::new(BareKey::Down), true),
                (KeyWithModifier::new(BareKey::Down), true),
                (KeyWithModifier::new(BareKey::Left), true),
                (KeyWithModifier::new(BareKey::Right), true),
                (KeyWithModifier::new(BareKey::Up), false),
                (KeyWithModifier::new(BareKey::Left), false),
                (KeyWithModifier::new(BareKey::Tab), true),
                (KeyWithModifier::new(BareKey::Tab), false),
                (
                    KeyWithModifier::new(BareKey::Tab).with_shift_modifier(),
                    true,
                ),
                (
                    KeyWithModifier::new(BareKey::Tab).with_shift_modifier(),
                    false,
                ),
                (KeyWithModifier::new(BareKey::Down), true),
            ] {
                assert!(screen.permission_modal_input(&Some(key), CLIENT));
                assert_hints(&screen, allow);
            }
        }
        assert!(screen.permission_modal_input(&Some(KeyWithModifier::new(answer)), CLIENT));
        assert!(!screen.permission_modal_open(CLIENT));
        let answers: Vec<_> = rx
            .try_iter()
            .filter_map(|(event, _)| match event {
                PluginInstruction::PermissionAnswer { status, .. } => Some(status),
                _ => None,
            })
            .collect();
        assert_eq!(answers, vec![expected]);
    }
}

#[test]
fn permission_modal_scrolls_all_rows_and_only_activates_visible_controls() {
    use yinz_types::keys::{BareKey, KeyWithModifier};
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;
    let size = Size {
        rows: 10,
        cols: 100,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen.open_permission_modal(
        PLUGIN,
        PluginPermission::new(
            "identity.scm".into(),
            vec![
                PermissionType::ReadApplicationState,
                PermissionType::ChangeApplicationState,
                PermissionType::OpenFiles,
                PermissionType::RunCommands,
                PermissionType::OpenTerminalsOrPlugins,
                PermissionType::WriteToStdin,
                PermissionType::WebAccess,
                PermissionType::ReadCliPipes,
                PermissionType::MessageAndLaunchOtherPlugins,
                PermissionType::Reconfigure,
            ],
        ),
    );
    let layout = |screen: &Screen| {
        screen.permission_modals[0].layout(screen.command_palette_surface(CLIENT).unwrap())
    };
    let key = |screen: &mut Screen, key| {
        assert!(screen.permission_modal_input(&Some(KeyWithModifier::new(key)), CLIENT));
    };
    let initial = layout(&screen);
    assert_eq!(initial.window_start, 0);
    assert!(initial.rows.iter().any(|row| row.label == "identity.scm"));
    assert!(initial.more_below);
    key(&mut screen, BareKey::Enter);
    assert!(
        screen.permission_modal_open(CLIENT),
        "hidden Deny is not activated"
    );
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..14 {
        seen.extend(layout(&screen).rows.iter().map(|row| row.item_index));
        key(&mut screen, BareKey::PageDown);
    }
    assert_eq!(seen, (0..14).collect());
    assert!(layout(&screen).more_above);
    assert!(!layout(&screen).more_below);
    key(&mut screen, BareKey::PageUp);
    assert!(layout(&screen).more_below);
    key(&mut screen, BareKey::Home);
    assert_eq!(layout(&screen).window_start, 0);
    key(&mut screen, BareKey::PageDown);
    assert!(layout(&screen).row_of_item(13).is_some());
    key(&mut screen, BareKey::PageUp);
    assert_eq!(layout(&screen).window_start, 0);
    key(&mut screen, BareKey::End);
    let position = Position::new(layout(&screen).row_of_item(13).unwrap() as i32, 50);
    screen.permission_modal_mouse(&MouseEvent::new_left_press_event(position), CLIENT);
    screen.permission_modal_mouse(&MouseEvent::new_scroll_up_event(position), CLIENT);
    screen.permission_modal_mouse(&MouseEvent::new_scroll_down_event(position), CLIENT);
    screen.permission_modal_mouse(&MouseEvent::new_left_release_event(position), CLIENT);
    assert!(
        screen.permission_modal_open(CLIENT),
        "scrolling invalidates the press even after returning"
    );
    seen.clear();
    for _ in 0..14 {
        seen.extend(layout(&screen).rows.iter().map(|row| row.item_index));
        screen.permission_modal_mouse(&MouseEvent::new_scroll_up_event(position), CLIENT);
    }
    assert_eq!(seen, (0..14).collect());
    key(&mut screen, BareKey::Tab);
    assert!(
        layout(&screen).row_of_item(13).is_some(),
        "selecting Allow reveals it"
    );
    key(&mut screen, BareKey::Home);
    key(&mut screen, BareKey::Enter);
    assert!(
        screen.permission_modal_open(CLIENT),
        "hidden Allow is not activated"
    );
    key(&mut screen, BareKey::Down);
    screen.resize_to_screen(SIZE).unwrap();
    screen.prepare_permission_modal_render();
    assert_eq!(
        layout(&screen).window_start,
        0,
        "resize clamps the viewport"
    );
    let position = Position::new(layout(&screen).row_of_item(13).unwrap() as i32, 50);
    screen.permission_modal_mouse(&MouseEvent::new_left_press_event(position), CLIENT);
    screen.resize_to_screen(size).unwrap();
    screen.prepare_permission_modal_render();
    screen.permission_modal_mouse(&MouseEvent::new_left_release_event(position), CLIENT);
    assert!(
        screen.permission_modal_open(CLIENT),
        "resize invalidates the press"
    );
    key(&mut screen, BareKey::Up);
    assert!(layout(&screen).row_of_item(12).is_some());
    key(&mut screen, BareKey::Enter);
    let answers: Vec<_> = rx
        .try_iter()
        .filter_map(|(event, _)| match event {
            PluginInstruction::PermissionAnswer { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![PermissionStatus::Denied]);

    screen.open_permission_modal(
        PLUGIN,
        PluginPermission::new("identity.scm".into(), vec![PermissionType::RunCommands; 10]),
    );
    key(&mut screen, BareKey::End);
    let position = Position::new(layout(&screen).row_of_item(13).unwrap() as i32, 50);
    screen.permission_modal_mouse(&MouseEvent::new_left_press_event(position), CLIENT);
    screen.permission_modal_mouse(&MouseEvent::new_scroll_up_event(position), CLIENT);
    key(&mut screen, BareKey::Esc);
    assert!(
        screen.permission_modal_mouse(&MouseEvent::new_left_release_event(position), CLIENT),
        "a release after scrolling and denial cannot leak to the pane"
    );
}

#[test]
fn permission_identities_wrap_without_losing_suffixes_and_controls_follow_resize() {
    use unicode_width::UnicodeWidthStr;
    use yinz_types::keys::{BareKey, KeyWithModifier};
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;

    let prefix = "projects with spaces/界面/cafe\u{301}/".repeat(8);
    let identities = [
        format!("/plugins/{prefix}trusted.scm"),
        format!("/plugins/{prefix}untrusted.scm"),
        format!("git://plugin?repo=/repos/{prefix}&rev=main&path=trusted.scm"),
        format!("git://plugin?repo=/repos/{prefix}&rev=main&path=untrusted.scm"),
    ];
    let narrow = Size { rows: 8, cols: 32 };
    let wide = Size { rows: 8, cols: 100 };
    let mut screen = create_non_mirrored_screen(narrow);
    new_tab(&mut screen, 1, TabId::new(0));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let layout = |screen: &Screen| {
        screen.permission_modals[0].layout(screen.command_palette_surface(CLIENT).unwrap())
    };
    let key = |screen: &mut Screen, key| {
        assert!(screen.permission_modal_input(&Some(KeyWithModifier::new(key)), CLIENT));
    };
    for (token, identity) in identities.into_iter().enumerate() {
        let mut request =
            PluginPermission::new(identity.clone(), vec![PermissionType::RunCommands]);
        request.request_id = token as u64;
        screen.open_permission_modal(PLUGIN, request);
        let mut control_indices = vec![];
        for size in [narrow, wide, narrow] {
            screen.resize_to_screen(size).unwrap();
            screen.prepare_permission_modal_render();
            key(&mut screen, BareKey::Home);
            key(&mut screen, BareKey::Enter);
            assert!(
                screen.permission_modal_open(CLIENT),
                "hidden control cannot answer"
            );
            let mut seen = BTreeMap::new();
            loop {
                let current = layout(&screen);
                for row in &current.rows {
                    assert!(row.label.width() <= current.width - 4);
                    seen.insert(row.item_index, row.clone());
                }
                if !current.more_below {
                    break;
                }
                key(&mut screen, BareKey::PageDown);
            }
            let deny = seen.len() - 2;
            let allow = seen.len() - 1;
            control_indices.push(allow);
            assert_eq!(seen[&deny].label, "Deny");
            assert_eq!(seen[&allow].label, "Allow");
            assert!(seen.values().take(deny).all(|row| row.disabled));
            let recovered: String = seen
                .range(1..deny - 1)
                .map(|(_, row)| row.label.as_str())
                .collect();
            assert_eq!(recovered, identity, "complete identity at {size:?}");
            key(&mut screen, BareKey::Tab);
            key(&mut screen, BareKey::Down);
            assert!(layout(&screen).row_of_item(allow).is_some());
        }
        assert!(
            control_indices[0] > control_indices[1],
            "wider rows rewrap the identity"
        );
        assert_eq!(control_indices[0], control_indices[2]);

        let current = layout(&screen);
        let allow_position = Position::new(
            current.row_of_item(control_indices[2]).unwrap() as i32,
            (current.x + 2) as u16,
        );
        screen.permission_modal_mouse(&MouseEvent::new_left_press_event(allow_position), CLIENT);
        screen.resize_to_screen(wide).unwrap();
        screen.prepare_permission_modal_render();
        screen.resize_to_screen(narrow).unwrap();
        screen.prepare_permission_modal_render();
        key(&mut screen, BareKey::Down);
        screen.permission_modal_mouse(&MouseEvent::new_left_release_event(allow_position), CLIENT);
        assert!(
            screen.permission_modal_open(CLIENT),
            "resize invalidates a stale press"
        );
        if token % 2 == 0 {
            key(&mut screen, BareKey::Enter);
        } else {
            let current = layout(&screen);
            let position = Position::new(
                current.row_of_item(control_indices[2]).unwrap() as i32,
                (current.x + 2) as u16,
            );
            screen.permission_modal_mouse(&MouseEvent::new_left_press_event(position), CLIENT);
            screen.permission_modal_mouse(&MouseEvent::new_left_release_event(position), CLIENT);
        }
        assert!(!screen.permission_modal_open(CLIENT));
        let answers: Vec<_> = rx
            .try_iter()
            .filter_map(|(event, _)| match event {
                PluginInstruction::PermissionAnswer {
                    request_id, status, ..
                } => Some((request_id, status)),
                _ => None,
            })
            .collect();
        assert_eq!(answers, vec![(token as u64, PermissionStatus::Granted)]);
    }
}

#[test]
fn background_permission_waits_for_client_and_pane_in_either_order() {
    for pane_first in [false, true] {
        let (mut mock, renders, mut threads) = start();
        let answers = Watched::new(vec![]);
        let receiver = mock.plugin_receiver.take().unwrap();
        threads.push(log_actions_in_thread!(
            answers,
            PluginInstruction::Exit,
            receiver
        ));
        mock.to_screen
            .send(ScreenInstruction::RemoveClient(CLIENT))
            .unwrap();
        request(&mock);
        assert!(pane_info(&mock, PaneId::Plugin(PLUGIN)).is_none());
        if pane_first {
            attach(&mock, TabId::new(0));
            assert!(pane_info(&mock, PaneId::Plugin(PLUGIN)).is_some());
        }
        mock.to_screen
            .send(ScreenInstruction::AddClient(CLIENT, SIZE, None, None))
            .unwrap();
        assert_prompt(&renders);
        if !pane_first {
            attach(&mock, TabId::new(0));
            assert!(pane_info(&mock, PaneId::Plugin(PLUGIN)).is_some());
        }
        for key in [
            yinz_types::keys::BareKey::Tab,
            yinz_types::keys::BareKey::Enter,
        ] {
            mock.to_screen
                .send(ScreenInstruction::WriteCharacter(
                    Some(yinz_types::keys::KeyWithModifier::new(key)),
                    vec![],
                    false,
                    CLIENT,
                    None,
                ))
                .unwrap();
        }
        answers.wait_until(
            "early permission grant reaches host",
            yinz_test_support::default_timeout(),
            |events| {
                events
                    .iter()
                    .any(|event| {
                        matches!(
                            event,
                            PluginInstruction::PermissionAnswer {
                                request_id: 0,
                                status: PermissionStatus::Granted,
                                ..
                            }
                        )
                    })
                    .then_some(())
            },
        );
        mock.teardown(threads);
    }
}
