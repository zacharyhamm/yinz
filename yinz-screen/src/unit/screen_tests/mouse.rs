#[cfg(test)]
const TERMINAL_1: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(1);

use super::*;
use crate::screen::{Notification, NotificationClick};
use crate::ui::notification::{notification_height, notification_width};
use std::time::Instant;
use yinz_test_support::Watched;
use yinz_types::animation::Tween;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

/// A toast that has finished arriving and is sitting still, which is what every hit-test below
/// wants: the animation is not under test here, the geometry is.
fn settled_notification(id: u64, text: &str, pane_id: PaneId) -> Notification {
    settled_notification_with(id, text, Some(pane_id), None)
}

/// The same, for the tests that care about what a toast carries rather than where it sits.
fn settled_notification_with(
    id: u64,
    text: &str,
    pane_id: Option<PaneId>,
    on_click: Option<NotificationClick>,
) -> Notification {
    let mut notification = Notification::new(
        NotificationId::new(id),
        text.to_owned(),
        pane_id,
        on_click,
        Instant::now(),
    );
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(0.0);
    notification
}

#[test]
fn clicking_a_pane_notification_focuses_its_pane_and_dismisses_it() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(size);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.core.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .core
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            crate::TerminalId::new(1),
            b"\x1b[?1002h\x1b[?1006h".to_vec(),
        )
        .unwrap();
    screen
        .go_to_tab(TabNumber::new(2), client_id)
        .expect("TEST");

    let text = "agent idle";
    screen.notices.notifications.push(settled_notification(
        0,
        text,
        PaneId::Terminal(crate::TerminalId::new(1)),
    ));
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        TabPosition::new(0)
    );
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
    assert!(screen.notices.notifications[0].is_dismissing());
    pty_writer_rx.try_iter().for_each(drop);

    let pane_position = Position::new(5, 5);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(pane_position), client_id);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(pane_position), client_id);
    assert!(pty_writer_rx.try_recv().is_err());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(pane_position), client_id);
    assert!(matches!(
        pty_writer_rx.recv().unwrap().0,
        PtyWriteInstruction::Write(_, TERMINAL_1, None)
    ));
}

#[test]
fn clicking_a_stale_pane_notification_dismisses_it_without_moving_focus() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let focused_before = screen.get_active_pane_id(&client_id);

    let text = "gone";
    screen.notices.notifications.push(settled_notification(
        0,
        text,
        PaneId::Terminal(crate::TerminalId::new(99)),
    ));
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    assert_eq!(screen.get_active_pane_id(&client_id), focused_before);
    assert!(screen.notices.notifications[0].is_dismissing());
}

#[test]
fn clicking_overlapping_pane_notifications_uses_paint_order() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .go_to_tab(TabNumber::new(2), client_id)
        .expect("TEST");

    let text = "agent idle";
    let newer = settled_notification(1, text, PaneId::Terminal(crate::TerminalId::new(2)));
    let mut older = settled_notification(0, text, PaneId::Terminal(crate::TerminalId::new(1)));
    // mid-glide: the older toast has just been pushed away from the corner and has not yet
    // travelled, so it is still sitting on top of the newer one. Whichever paints last wins the
    // click, and the newer one paints last.
    older.y_offset = Tween::new(
        -(notification_height(1) as f32),
        0.0,
        std::time::Duration::from_millis(220),
        yinz_types::animation::Easing::EaseOut,
        Instant::now(),
    );
    screen.notices.notifications.extend([newer, older]);
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
    assert!(!screen.notices.notifications[0].is_dismissing());
    assert!(screen.notices.notifications[1].is_dismissing());
}

/// The click on a toast is the one gesture that says "the thing this was about, now". Focusing
/// the pane says where; the message says what, and only the plugin that raised the toast can act
/// on it - agent opening the conversation that asked, say.
///
/// Both clients travel with it. A toast raised by one client's instance can be clicked by
/// another, and the two ids answer different questions - see
/// `plugins::client_for_notification_click`.
#[test]
fn clicking_a_notification_tells_the_plugin_that_raised_it() {
    let size = Size { cols: 80, rows: 20 };
    let raiser = ClientId::new(2);
    let client_id = ClientId::new(1);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut screen = create_non_mirrored_screen(size);
    screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, TabId::new(0));

    let text = "agent has a plan";
    screen.notices.notifications.push(settled_notification_with(
        0,
        text,
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        Some(NotificationClick {
            plugin_id: crate::PluginId::new(7),
            client_id: raiser,
            message: "agent_toast_click".to_owned(),
            payload: "42".to_owned(),
        }),
    ));
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    let posted = plugin_rx
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::NotificationClicked {
                plugin_id,
                raiser,
                clicker,
                message,
                payload,
            } => Some((plugin_id, raiser, clicker, message, payload)),
            _ => None,
        })
        .expect("the click reaches the plugin that raised the toast");
    assert_eq!(
        posted,
        (
            crate::PluginId::new(7),
            raiser,
            client_id,
            "agent_toast_click".to_owned(),
            "42".to_owned()
        )
    );
    assert!(screen.notices.notifications[0].is_dismissing());
}

/// A toast whose plugin is already the pane in front still has something to say - the focus is
/// skipped there, and skipping the message with it would silently drop the click.
#[test]
fn a_notification_click_is_reported_even_when_its_pane_is_already_focused() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut screen = create_non_mirrored_screen(size);
    screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    let focused = screen
        .get_active_pane_id(&client_id)
        .expect("the tab has a focused pane");

    let text = "needs you";
    screen.notices.notifications.push(settled_notification_with(
        0,
        text,
        Some(focused),
        Some(NotificationClick {
            plugin_id: crate::PluginId::new(7),
            client_id,
            message: "agent_toast_click".to_owned(),
            payload: "9".to_owned(),
        }),
    ));
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::NotificationClicked { .. }
    )));
}

/// A toast with nothing to do is not a click target: swallowing the press would leave the user
/// unable to click the pane a plain `notify` happens to be sitting over.
#[test]
fn clicking_a_notification_with_no_target_falls_through_to_the_pane() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let mut screen = create_non_mirrored_screen(size);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.core.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .core
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            crate::TerminalId::new(1),
            b"\x1b[?1002h\x1b[?1006h".to_vec(),
        )
        .unwrap();
    pty_writer_rx.try_iter().for_each(drop);

    let text = "just saying";
    screen
        .notices
        .notifications
        .push(settled_notification_with(0, text, None, None));
    let width = notification_width(text, size.cols).expect("notification fits");
    let x = size.cols - width - 1;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(1, (x + 1) as u16)),
        client_id,
    );

    assert!(
        !screen.notices.notifications[0].is_dismissing(),
        "a toast that is not a click target is not dismissed by the click either"
    );
    assert!(
        pty_writer_rx.try_recv().is_ok(),
        "the press reaches the pane underneath"
    );
}

#[test]
pub fn mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
}

#[test]
pub fn disabled_mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.set_advanced_hover_effects(false);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
}

#[test]
fn group_panes_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(crate::TerminalId::new(2))]),
        "Pane Id added to client's pane group"
    );

    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

fn screen_with_tab_bar_for_grouping() -> (Screen, Receiver<(PluginInstruction, ErrorContext)>) {
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    let (plugin_tx, plugin_rx) = channels::unbounded();
    screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    screen
        .new_tab(TabId::new(0), (vec![], vec![]), None, Some(client_id))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        run: Some(Run::Plugin(plugin.clone())),
                        split_size: Some(yinz_types::geometry::SplitSize::Fixed(2)),
                        borderless: Some(true),
                        ..Default::default()
                    },
                    TiledPaneLayout::default(),
                ],
                ..Default::default()
            },
            vec![],
            vec![(TerminalId::new(1), None)],
            vec![],
            HashMap::from([(plugin, vec![crate::PluginId::new(1)])]),
            TabId::new(0),
            true,
            client_id,
            None,
        )
        .unwrap();
    (screen, plugin_rx)
}

#[test]
fn pane_grouping_ignores_nonselectable_panes_under_alt_click_and_drag() {
    let client_id = ClientId::new(1);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    let bar = PaneId::Plugin(crate::PluginId::new(1));
    let (mut screen, _plugin_rx) = screen_with_tab_bar_for_grouping();
    screen.set_pane_selectable(bar, false);

    for event in [
        MouseEvent::new_left_press_with_alt_event(Position::new(0, 10)),
        MouseEvent::new_left_motion_with_alt_event(Position::new(0, 10)),
    ] {
        screen.handle_mouse_event(event, client_id);
        assert!(screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .is_empty());
    }

    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(5, 10)),
        client_id,
    );
    for event in [
        MouseEvent::new_left_motion_with_alt_event(Position::new(0, 10)),
        MouseEvent::new_left_press_with_alt_event(Position::new(0, 10)),
    ] {
        screen.handle_mouse_event(event, client_id);
        assert_eq!(
            screen.core.current_pane_group.borrow().clone_inner()[&client_id],
            vec![terminal]
        );
    }
}

#[test]
fn pane_grouping_filters_plugin_requests_and_preserves_selectable_plugins() {
    let client_id = ClientId::new(1);
    let other_client = ClientId::new(2);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    let plugin = PaneId::Plugin(crate::PluginId::new(1));
    let missing = PaneId::Terminal(TerminalId::new(99));

    for for_all_clients in [false, true] {
        for selectable in [false, true] {
            let (mut screen, _plugin_rx) = screen_with_tab_bar_for_grouping();
            screen.set_pane_selectable(plugin, selectable);
            screen
                .core
                .current_pane_group
                .borrow_mut()
                .override_groups_with(HashMap::from([
                    (client_id, vec![terminal]),
                    (other_client, vec![terminal]),
                ]));
            screen.group_and_ungroup_panes(
                vec![plugin, missing],
                vec![],
                for_all_clients,
                client_id,
            );
            for client in [client_id, other_client] {
                let expected = if selectable && (client == client_id || for_all_clients) {
                    vec![terminal, plugin]
                } else {
                    vec![terminal]
                };
                assert_eq!(
                    screen.core.current_pane_group.borrow().clone_inner()[&client],
                    expected
                );
            }
            screen.group_and_ungroup_panes(vec![], vec![plugin], for_all_clients, client_id);
            for panes in screen
                .core
                .current_pane_group
                .borrow()
                .clone_inner()
                .values()
            {
                assert_eq!(panes, &vec![terminal]);
            }
        }
    }
}

#[test]
fn pane_grouping_removes_panes_that_become_nonselectable_and_stale_members() {
    let client_id = ClientId::new(1);
    let terminal = PaneId::Terminal(TerminalId::new(1));
    let bar = PaneId::Plugin(crate::PluginId::new(1));
    let missing = PaneId::Terminal(TerminalId::new(99));
    let (mut screen, _plugin_rx) = screen_with_tab_bar_for_grouping();
    screen.group_and_ungroup_panes(vec![bar, terminal], vec![], false, client_id);
    assert_eq!(
        screen.core.current_pane_group.borrow().clone_inner()[&client_id],
        vec![bar, terminal]
    );

    assert!(screen.set_pane_selectable(bar, false));
    assert_eq!(
        screen.core.current_pane_group.borrow().clone_inner()[&client_id],
        vec![terminal]
    );

    // An already-invalid selection is pruned too, including a closed pane.
    screen
        .core
        .current_pane_group
        .borrow_mut()
        .override_groups_with(HashMap::from([(client_id, vec![bar, terminal, missing])]));
    screen.add_pane_id_to_group(bar, &client_id);
    assert_eq!(
        screen.core.current_pane_group.borrow().clone_inner()[&client_id],
        vec![terminal]
    );
}

/// Grouping opens the pinned multiple-select control in the tab the grouping client is in,
/// never in a fixed tab id, and the control stays there when the client moves on.
#[test]
fn pane_group_control_opens_in_the_grouping_clients_tab() {
    use crate::screen::thread_main::PendingEvents;
    use yinz_bus::pty::ReplacedPane;

    let client_id = ClientId::new(1);
    let control = crate::PluginId::new(77);
    for (target_tab, tab_number, other_tab_number) in [(TabId::new(0), 1, 3), (TabId::new(2), 3, 2)]
    {
        let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
        let (pty_tx, pty_rx): ChannelWithContext<PtyInstruction> = channels::unbounded();
        screen.core.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        // Tabs need a plugin sender to build the control's pane.
        let (plugin_tx, _plugin_rx): ChannelWithContext<PluginInstruction> = channels::unbounded();
        screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        // The group bookkeeping holds its own senders, taken when the screen was built.
        *screen.core.current_pane_group.borrow_mut() =
            crate::pane_groups::PaneGroups::new(screen.core.bus.senders.clone());
        for (pid, tab_id) in [(1, 0), (2, 1), (3, 2)] {
            new_tab(&mut screen, pid, TabId::new(tab_id));
        }
        screen
            .go_to_tab(TabNumber::new(tab_number), client_id)
            .unwrap();
        assert_eq!(
            screen.core.active_tab_ids.get(&client_id),
            Some(&target_tab)
        );

        screen.toggle_pane_in_group(client_id).unwrap();
        let launches: Vec<_> = pty_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyInstruction::FillPluginCwd(
                    _,
                    _,
                    _,
                    _,
                    run_plugin,
                    tab_id,
                    _,
                    acting_client,
                    _,
                    _,
                    _,
                    _,
                    coordinates,
                    _,
                    _,
                ) => Some((run_plugin, tab_id, acting_client, coordinates)),
                _ => None,
            })
            .collect();
        let [(run_plugin, launch_tab, acting_client, coordinates)] = launches.as_slice() else {
            panic!("grouping must launch the control exactly once, got {launches:?}");
        };
        assert!(
            format!("{run_plugin:?}").contains("multiple-select"),
            "launched {run_plugin:?}"
        );
        assert_eq!(*acting_client, client_id);
        assert_eq!(
            *launch_tab, target_tab,
            "the control must open in the grouping client's tab"
        );

        // The pty thread forwards the tab to the plugin thread, which answers with this.
        let mut pending = PendingEvents::default();
        screen
            .add_plugin(
                Some(true),
                false,
                ReplacedPane::Close,
                run_plugin.clone(),
                None,
                Some(*launch_tab),
                control,
                None,
                None,
                false,
                coordinates.clone(),
                Some(false),
                Some(client_id),
                None,
                None,
                &mut pending,
            )
            .unwrap();
        let tabs_with_control = |screen: &Screen| -> Vec<TabId> {
            screen
                .core
                .tabs
                .iter()
                .filter(|(_, tab)| tab.has_plugin(control))
                .map(|(tab_id, _)| *tab_id)
                .collect()
        };
        assert_eq!(tabs_with_control(&screen), vec![target_tab]);

        // Moving to another tab and growing the group neither moves the control nor opens a
        // second one: it stays pinned where the grouping started.
        screen
            .go_to_tab(TabNumber::new(other_tab_number), client_id)
            .unwrap();
        assert_ne!(
            screen.core.active_tab_ids.get(&client_id),
            Some(&target_tab)
        );
        screen.toggle_pane_in_group(client_id).unwrap();
        assert_eq!(
            screen.core.current_pane_group.borrow().clone_inner()[&client_id].len(),
            2
        );
        assert!(
            !pty_rx
                .try_iter()
                .any(|(instruction, _)| matches!(instruction, PtyInstruction::FillPluginCwd(..))),
            "a non-empty group must not launch another control"
        );
        assert_eq!(tabs_with_control(&screen), vec![target_tab]);
    }
}

#[test]
fn mouse_focus_clears_bell_on_focused_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));

    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        // Split horizontally: pane 1 on top, pane 2 on bottom; focus moves to pane 2
        active_tab
            .horizontal_split(new_pane_id, None, None, client_id, None, None)
            .unwrap();
        // Move focus back up to pane 1 so pane 2 is unfocused
        active_tab.move_focus_up(client_id).unwrap();
        // Plant a bell on the unfocused pane 2
        active_tab
            .handle_pty_bytes(crate::TerminalId::new(2), vec![7u8])
            .unwrap();
        active_tab.check_and_handle_bell_notifications(false);
        assert!(
            active_tab.panes_with_pending_bell.contains(&new_pane_id),
            "Pane 2 should have a pending bell before the click"
        );
        assert!(
            active_tab.tab_has_pending_bell,
            "Tab should have a pending bell before the click"
        );
    }

    // Click somewhere in pane 2 (bottom half of the screen)
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(15, 60)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(15, 60)),
        client_id,
    );

    let active_tab = screen.get_active_tab(client_id).unwrap();
    assert_eq!(
        active_tab.get_active_pane_id(client_id),
        Some(new_pane_id),
        "Mouse click should have focused pane 2"
    );
    assert!(
        !active_tab.panes_with_pending_bell.contains(&new_pane_id),
        "Bell on pane 2 should be cleared after mouse focus"
    );
    assert!(
        !active_tab.tab_has_pending_bell,
        "Tab bell should be cleared after the last pane bell is cleared"
    );
}

#[test]
fn nested_guest_fullscreen_moves_from_one_pane_to_another() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));
    let pane_one = PaneId::Terminal(crate::TerminalId::new(1));
    let pane_two = PaneId::Terminal(crate::TerminalId::new(2));
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        active_tab
            .horizontal_split(pane_two, None, None, client_id, None, None)
            .unwrap();
    }

    screen.apply_nested_guest_fullscreen(pane_one, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_one),
            "the first guest pane is fullscreen"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested.nested_fullscreen_panes,
        [pane_one].into_iter().collect(),
        "only the first guest pane is tracked as fullscreen"
    );

    screen.apply_nested_guest_fullscreen(pane_two, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_two),
            "fullscreen moved to the second guest pane"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested.nested_fullscreen_panes,
        [pane_two].into_iter().collect(),
        "the first guest pane is no longer tracked as fullscreen, only the second is"
    );

    screen.apply_nested_guest_fullscreen(pane_two, false);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            None,
            "unsetting fullscreen restores the tiled layout"
        );
    }
    assert!(
        screen.nested.nested_fullscreen_panes.is_empty(),
        "no guest panes are tracked as fullscreen after unsetting"
    );
}

#[test]
fn group_panes_with_keyboard() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(crate::TerminalId::new(2))]),
        "Pane Id added to client's pane group"
    );

    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

#[test]
fn group_panes_following_focus() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(crate::TerminalId::new(i)),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .core
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![
                PaneId::Terminal(crate::TerminalId::new(4)),
                PaneId::Terminal(crate::TerminalId::new(3))
            ]),
            "Pane Id of focused pane and newly focused pane above added to pane group"
        );
    }
    {
        let _ = screen.toggle_group_marking(client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(screen.core.current_pane_group.borrow().clone_inner().get(&client_id), Some(&vec![PaneId::Terminal(crate::TerminalId::new(4)), PaneId::Terminal(crate::TerminalId::new(3))]), "Pane Id of newly focused pane not added to group after the group marking was toggled off");
    }
}

#[test]
fn break_group_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, TabId::new(0));

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(crate::TerminalId::new(i)),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .core
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![
                PaneId::Terminal(crate::TerminalId::new(4)),
                PaneId::Terminal(crate::TerminalId::new(3)),
                PaneId::Terminal(crate::TerminalId::new(2))
            ]),
            "Group contains 3 panes"
        );
    }

    screen.handle_mouse_event(
        MouseEvent::new_right_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );
    assert_eq!(
        screen
            .core
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Group cleared by mouse event"
    );
}

/// Mouse selection and plugin mouse events are independent capabilities: enabling selection must
/// not make clickable plugin UI unreachable.
#[test]
fn a_plugin_pane_with_mouse_selection_hears_the_whole_click() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut screen = create_non_mirrored_screen(size);
    screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, TabId::new(0));

    let dialog = Run::Plugin(
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/path/to/fake/dialog",
            &None,
            None,
            None,
        )
        .unwrap(),
    );
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .new_pane(
            PaneId::Plugin(crate::PluginId::new(7)),
            Some("dialog".to_owned()),
            Some(dialog),
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(client_id),
            None,
        )
        .expect("TEST");
    screen
        .with_pane(PaneId::Plugin(crate::PluginId::new(7)), |pane| {
            pane.plugin_control_mut()
                .unwrap()
                .set_mouse_selection_support(true)
        })
        .expect("the dialog is in the tab");
    let (content_x, content_y) = screen
        .with_pane(PaneId::Plugin(crate::PluginId::new(7)), |pane| {
            (pane.get_content_x(), pane.get_content_y())
        })
        .expect("the dialog is in the tab");
    while plugin_rx.try_recv().is_ok() {}

    let press = Position::new((content_y + 1) as i32, (content_x + 2) as u16);
    let drag = Position::new((content_y + 2) as i32, (content_x + 3) as u16);
    screen.handle_mouse_event(MouseEvent::new_left_press_event(press), client_id);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(drag), client_id);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(drag), client_id);

    let events: Vec<_> = plugin_rx
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .filter_map(|(plugin, client, event)| match event {
            Event::Mouse(mouse) => Some((plugin, client, mouse)),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        [
            Mouse::LeftClick(1, 2),
            Mouse::Hold(2, 3),
            Mouse::Release(2, 3)
        ]
        .map(|mouse| (Some(crate::PluginId::new(7)), Some(client_id), mouse))
    );
}

/// A client that switches tabs mid-drag sends its release to the new tab, so the old tab never
/// sees that drag end. Its capture leaves with it. Another client in the old tab can then resize
/// a tiled border, plain or ctrl, once the clients still dragging there have released. Until
/// then the capture of a client that stayed still refuses the resize.
#[test]
fn a_client_leaving_a_tab_mid_drag_takes_its_capture_along() {
    let leaving = ClientId::new(1);
    let resizing = ClientId::new(2);
    let staying = ClientId::new(3);
    let left = PaneId::Terminal(TERMINAL_1);
    let right = PaneId::Terminal(TerminalId::new(2));
    type Events = [fn(Position) -> MouseEvent; 3];
    let plain: Events = [
        MouseEvent::new_left_press_event,
        MouseEvent::new_left_motion_event,
        MouseEvent::new_left_release_event,
    ];
    let ctrl: Events = [
        MouseEvent::new_left_press_with_ctrl_event,
        MouseEvent::new_left_motion_with_ctrl_event,
        MouseEvent::new_left_release_with_ctrl_event,
    ];
    for (name, [press, motion, release]) in [("plain", plain), ("ctrl", ctrl)] {
        let mut screen = create_non_mirrored_screen(Size {
            cols: 120,
            rows: 20,
        });
        new_tab(&mut screen, 1, TabId::new(0));
        add_pane_to_tab(&mut screen, TabId::new(0), right, false);
        new_tab(&mut screen, 3, TabId::new(1));
        screen.go_to_tab(TabNumber::new(1), leaving).unwrap();
        for client in [resizing, staying] {
            screen.add_client(client).unwrap();
        }
        let tab = screen.core.tabs.get_mut(&TabId::new(0)).unwrap();
        tab.set_pane_frames(yinz_types::input::options::PaneFrameStyle::Full);
        for (client, pane) in [(leaving, left), (resizing, left), (staying, right)] {
            tab.focus_pane_with_id(pane, false, false, client).unwrap();
        }
        let content = |pane: PaneId| {
            let pane = tab.get_pane_with_id(pane).unwrap();
            Position::new(pane.get_content_y() as i32, pane.get_content_x() as u16 + 1)
        };
        let (left_content, right_content) = (content(left), content(right));
        let geom = tab.get_pane_with_id(left).unwrap().current_geom();
        let border = Position::new(
            (geom.y + geom.rows.as_usize() / 2) as i32,
            (geom.x + geom.cols.as_usize() - 1) as u16,
        );
        let dragged = Position::new(border.line() as i32, border.column() as u16 + 4);
        let right_geom = |screen: &Screen| {
            let geom = screen.core.tabs[&TabId::new(0)]
                .get_pane_with_id(right)
                .unwrap()
                .current_geom();
            (geom.x, geom.cols.as_usize())
        };
        let captures = |screen: &Screen| {
            screen.core.tabs[&TabId::new(0)]
                .selecting_with_mouse_in_pane
                .clone()
        };
        let before = right_geom(&screen);

        screen.handle_mouse_event(MouseEvent::new_left_press_event(left_content), leaving);
        screen.handle_mouse_event(MouseEvent::new_left_press_event(right_content), staying);
        assert_eq!(
            captures(&screen),
            HashMap::from([(leaving, left), (staying, right)]),
            "{name}"
        );

        screen.go_to_tab(TabNumber::new(2), leaving).unwrap();
        assert_eq!(screen.get_active_tab(leaving).unwrap().id, TabId::new(1));
        assert_eq!(
            captures(&screen),
            HashMap::from([(staying, right)]),
            "{name}: the departing client's capture leaves with it, the staying one's remains"
        );
        screen.handle_mouse_event(MouseEvent::new_left_release_event(left_content), leaving);
        assert_eq!(
            captures(&screen),
            HashMap::from([(staying, right)]),
            "{name}: the release went to the new tab"
        );

        for event in [press(border), motion(dragged), release(dragged)] {
            screen.handle_mouse_event(event, resizing);
        }
        assert_eq!(
            right_geom(&screen),
            before,
            "{name}: a client still dragging here keeps the border from resizing"
        );
        screen.handle_mouse_event(MouseEvent::new_left_release_event(right_content), staying);
        assert!(captures(&screen).is_empty(), "{name}");

        for event in [press(border), motion(dragged), release(dragged)] {
            screen.handle_mouse_event(event, resizing);
        }
        assert_eq!(
            right_geom(&screen),
            (before.0 + 4, before.1 - 4),
            "{name}: with no drag left in the tab, the border resizes"
        );
    }
}

/// A Screen surface that opens for a client mid-drag takes that client's release, so its tab
/// never sees the drag end. The surface keeps the release, and the tab ends the drag where it
/// stands, for a terminal and for a plugin with mouse selection alike. The capture goes, so
/// another client resizes a border on its first try. Nothing is copied, the selection keeps the
/// extent the last motion gave it, and a terminal's selection stays on its text as output
/// scrolls it instead of growing. The plugin hears the press and the hold but no release.
#[test]
fn a_screen_surface_taking_a_drags_release_ends_that_drag() {
    use yinz_types::data::{PermissionType, PluginPermission};
    let dragging = ClientId::new(1);
    let resizing = ClientId::new(2);
    let terminal = PaneId::Terminal(TERMINAL_1);
    let plugin = PaneId::Plugin(crate::PluginId::new(7));
    let permission_modal: fn(&mut Screen, ClientId) = |screen, client| {
        let mut request = PluginPermission::new(
            "drag.scm".into(),
            vec![PermissionType::ReadApplicationState],
        );
        request.request_id = 1;
        screen.open_permission_modal(crate::PluginId::new(42), request);
        assert!(
            screen.permissions.permission_modal_open(client),
            "the modal goes to the client that last used the mouse"
        );
    };
    let command_palette: fn(&mut Screen, ClientId) = |screen, client| {
        screen.toggle_command_palette(client);
        assert!(screen.palette.command_palettes.contains_key(&client));
    };
    let still_open = |screen: &Screen, surface: &str| match surface {
        "permission modal" => screen.permissions.permission_modal_open(dragging),
        _ => screen.palette.command_palettes.contains_key(&dragging),
    };
    for (surface, open) in [
        ("permission modal", permission_modal),
        ("command palette", command_palette),
    ] {
        for dragged in [terminal, plugin] {
            let name = format!("{surface}, {dragged:?}");
            let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
            let mut screen = create_non_mirrored_screen(Size {
                cols: 120,
                rows: 20,
            });
            screen.core.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
            new_tab(&mut screen, 1, TabId::new(0));
            let right = if dragged == plugin {
                screen
                    .get_active_tab_mut(dragging)
                    .unwrap()
                    .new_pane(
                        plugin,
                        None,
                        None,
                        false,
                        false,
                        NewPanePlacement::Tiled {
                            direction: None,
                            borderless: None,
                        },
                        Some(dragging),
                        None,
                    )
                    .unwrap();
                plugin
            } else {
                let right = PaneId::Terminal(TerminalId::new(2));
                add_pane_to_tab(&mut screen, TabId::new(0), right, false);
                right
            };
            screen.add_client(resizing).unwrap();
            let (server_tx, server_rx) = channels::unbounded();
            let tab = screen.core.tabs.get_mut(&TabId::new(0)).unwrap();
            tab.senders.to_server = Some(SenderWithContext::new(server_tx));
            tab.set_pane_frames(yinz_types::input::options::PaneFrameStyle::Full);
            tab.handle_pty_bytes(TERMINAL_1, b"line1 abcdefgh\r\nline2".to_vec())
                .unwrap();
            if let Some(control) = tab
                .get_pane_with_id_mut(plugin)
                .and_then(|pane| pane.plugin_control_mut())
            {
                control.set_mouse_selection_support(true);
                control.handle_plugin_bytes(dragging, b"abcdefghi".to_vec());
            }
            for client in [dragging, resizing] {
                tab.focus_pane_with_id(dragged, false, false, client)
                    .unwrap();
            }
            let pane = tab.get_pane_with_id(dragged).unwrap();
            let at = |column: u16| {
                Position::new(
                    pane.get_content_y() as i32,
                    pane.get_content_x() as u16 + column,
                )
            };
            let (press, last_motion, release) = (at(0), at(4), at(8));
            let geom = tab.get_pane_with_id(terminal).unwrap().current_geom();
            let border = Position::new(
                (geom.y + geom.rows.as_usize() / 2) as i32,
                (geom.x + geom.cols.as_usize() - 1) as u16,
            );
            let dragged_border = Position::new(border.line() as i32, border.column() as u16 + 4);
            let right_geom = |screen: &Screen| {
                let geom = screen.core.tabs[&TabId::new(0)]
                    .get_pane_with_id(right)
                    .unwrap()
                    .current_geom();
                (geom.x, geom.cols.as_usize())
            };
            let selection = |screen: &Screen| {
                screen.core.tabs[&TabId::new(0)]
                    .get_pane_with_id(dragged)
                    .unwrap()
                    .get_selected_text(dragging)
            };
            let before = right_geom(&screen);
            while plugin_rx.try_recv().is_ok() {}

            screen.handle_mouse_event(MouseEvent::new_left_press_event(press), dragging);
            screen.handle_mouse_event(MouseEvent::new_left_motion_event(last_motion), dragging);
            assert_eq!(
                screen.core.tabs[&TabId::new(0)].selecting_with_mouse_in_pane,
                HashMap::from([(dragging, dragged)]),
                "{name}"
            );
            let selected = if dragged == plugin { "abcd" } else { "line" };
            assert_eq!(selection(&screen).as_deref(), Some(selected), "{name}");

            open(&mut screen, dragging);
            screen.handle_mouse_event(MouseEvent::new_left_release_event(release), dragging);
            assert!(still_open(&screen, surface), "{name}: the surface kept it");
            assert!(
                screen.core.tabs[&TabId::new(0)]
                    .selecting_with_mouse_in_pane
                    .is_empty(),
                "{name}: the drag ended with the release the surface took"
            );
            assert_eq!(
                selection(&screen).as_deref(),
                Some(selected),
                "{name}: the selection ends where the last motion left it"
            );
            assert!(
                !server_rx.try_iter().any(|(instruction, _)| matches!(
                    instruction,
                    ServerInstruction::ClipboardUpdate(_)
                )),
                "{name}: nothing is copied"
            );
            let plugin_mouse: Vec<_> = plugin_rx
                .try_iter()
                .flat_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(updates) => updates,
                    _ => vec![],
                })
                .filter_map(|(id, client, event)| match event {
                    Event::Mouse(mouse) => Some((id, client, mouse)),
                    _ => None,
                })
                .collect();
            let heard = if dragged == plugin {
                vec![Mouse::LeftClick(0, 0), Mouse::Hold(0, 4)]
            } else {
                vec![]
            };
            assert_eq!(
                plugin_mouse,
                heard
                    .into_iter()
                    .map(|mouse| (Some(crate::PluginId::new(7)), Some(dragging), mouse))
                    .collect::<Vec<_>>(),
                "{name}"
            );

            if dragged == terminal {
                let output: String = (0..25).map(|line| format!("\r\nout{line}")).collect();
                screen
                    .core
                    .tabs
                    .get_mut(&TabId::new(0))
                    .unwrap()
                    .handle_pty_bytes(TERMINAL_1, output.into_bytes())
                    .unwrap();
                assert_eq!(
                    selection(&screen).as_deref(),
                    Some(selected),
                    "{name}: output scrolls the selection along with its text"
                );
            }

            for event in [
                MouseEvent::new_left_press_event(border),
                MouseEvent::new_left_motion_event(dragged_border),
                MouseEvent::new_left_release_event(dragged_border),
            ] {
                screen.handle_mouse_event(event, resizing);
            }
            assert_eq!(
                right_geom(&screen),
                (before.0 + 4, before.1 - 4),
                "{name}: the other client's first border drag resizes"
            );
        }
    }
}
