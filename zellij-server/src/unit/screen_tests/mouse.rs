#[cfg(test)]
const TERMINAL_1: zellij_types::internal_ids::TerminalId =
    zellij_types::internal_ids::TerminalId::new(1);

use super::*;
use crate::screen::Notification;
use crate::ui::notification::{notification_height, notification_width};
use std::time::Instant;
use zellij_test_support::Watched;
use zellij_types::animation::Tween;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TabPosition;

/// A toast that has finished arriving and is sitting still, which is what every hit-test below
/// wants: the animation is not under test here, the geometry is.
fn settled_notification(id: u64, text: &str, pane_id: PaneId) -> Notification {
    let mut notification = Notification::new(id, text.to_owned(), Some(pane_id), Instant::now());
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
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
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
    screen.notifications.push(settled_notification(
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
    assert!(screen.notifications[0].is_dismissing());
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
    screen.notifications.push(settled_notification(
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
    assert!(screen.notifications[0].is_dismissing());
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
        zellij_types::animation::Easing::EaseOut,
        Instant::now(),
    );
    screen.notifications.extend([newer, older]);
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
    assert!(!screen.notifications[0].is_dismissing());
    assert!(screen.notifications[1].is_dismissing());
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
        assert_snapshot!(format!("{}", snapshot));
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
        assert_snapshot!(format!("{}", snapshot));
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
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
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
            .horizontal_split(new_pane_id, None, client_id, None, None)
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
            .horizontal_split(pane_two, None, client_id, None, None)
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
        screen.nested_fullscreen_panes,
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
        screen.nested_fullscreen_panes,
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
        screen.nested_fullscreen_panes.is_empty(),
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
        assert_eq!(screen.current_pane_group.borrow().clone_inner().get(&client_id), Some(&vec![PaneId::Terminal(crate::TerminalId::new(4)), PaneId::Terminal(crate::TerminalId::new(3))]), "Pane Id of newly focused pane not added to group after the group marking was toggled off");
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
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, TabId::new(0));

    let dialog = Run::Plugin(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
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
            pane.set_mouse_selection_support(true)
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
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .filter_map(|(plugin_id, _, event)| {
                    (plugin_id == Some(crate::PluginId::new(7))).then_some(event)
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();

    // Content-relative, so the press lands one row and two columns in.
    assert!(
        events.contains(&Event::Mouse(Mouse::LeftClick(1, 2))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Hold(2, 3))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Release(2, 3))),
        "{events:?}"
    );
}
