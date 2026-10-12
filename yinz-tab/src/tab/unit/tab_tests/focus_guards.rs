use super::*;
use yinz_types::position::Position;

#[test]
fn invalid_focus_does_not_change_tiled_or_floating_history() {
    let client = ClientId::new(1);
    let missing = PaneId::Terminal(TerminalId::new(99));
    let mut tab = create_tab_with_two_floating_panes();
    let first = PaneId::Terminal(TerminalId::new(2));
    let second = PaneId::Terminal(TerminalId::new(3));
    tab.floating_panes.toggle_show_panes(true);
    tab.floating_panes.focus_pane(first, client);
    tab.floating_panes.focus_pane(second, client);
    tab.floating_panes.focus_pane(missing, client);
    tab.floating_panes.focus_last_pane(client);
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(first));
    tab.floating_panes
        .get_mut(&second)
        .unwrap()
        .set_selectable(false);
    tab.floating_panes.focus_pane(second, client);
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(first));

    let mut tab = create_new_tab(
        Size {
            cols: 120,
            rows: 20,
        },
        false,
    );
    tab.vertical_split(missing, None, None, client, None, None)
        .unwrap();
    let tiled_first = PaneId::Terminal(TerminalId::new(1));
    tab.tiled_panes.focus_pane(tiled_first, client);
    tab.tiled_panes.focus_pane(missing, client);
    let last = tab.tiled_panes.get_last_pane_id(client);
    tab.tiled_panes.focus_pane(first, client);
    assert_eq!(tab.tiled_panes.get_active_pane_id(client), Some(missing));
    assert_eq!(tab.tiled_panes.get_last_pane_id(client), last);
    tab.tiled_panes
        .get_pane_mut(tiled_first)
        .unwrap()
        .set_selectable(false);
    tab.tiled_panes.focus_pane(tiled_first, client);
    assert_eq!(tab.tiled_panes.get_last_pane_id(client), last);
}

#[test]
fn returning_client_joins_the_connected_clients_floating_focus() {
    let mut tab = create_tab_with_two_floating_panes();
    let first = ClientId::new(1);
    let returning = ClientId::new(2);
    let old = PaneId::Terminal(TerminalId::new(2));
    let current = PaneId::Terminal(TerminalId::new(3));
    tab.floating_panes.focus_pane(old, first);
    tab.add_client(returning, None).unwrap();
    tab.remove_client(returning);
    tab.floating_panes.focus_pane(current, first);
    assert_eq!(tab.floating_panes.active_pane_id(returning), Some(old));
    tab.add_client(returning, None).unwrap();
    assert_eq!(tab.floating_panes.active_pane_id(first), Some(current));
    assert_eq!(tab.floating_panes.active_pane_id(returning), Some(current));
}

#[test]
fn joining_alone_uses_valid_remembered_focus_then_recent_selectable_focus() {
    let mut tab = create_tab_with_two_floating_panes();
    let client = ClientId::new(1);
    let remembered = PaneId::Terminal(TerminalId::new(2));
    let recent = PaneId::Terminal(TerminalId::new(3));
    tab.floating_panes.focus_pane(remembered, client);
    tab.remove_client(client);
    tab.floating_panes
        .get_mut(&recent)
        .unwrap()
        .set_active_at(std::time::Instant::now());
    tab.add_client(client, None).unwrap();
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(remembered));
    tab.remove_client(client);
    tab.floating_panes
        .get_mut(&remembered)
        .unwrap()
        .set_selectable(false);
    tab.add_client(client, None).unwrap();
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(recent));
}

#[test]
fn floating_replacement_and_removal_retarget_focus_fullscreen_and_drag() {
    let mut tab = create_tab_with_two_floating_panes();
    let client = ClientId::new(1);
    let old = PaneId::Terminal(TerminalId::new(2));
    let other = PaneId::Terminal(TerminalId::new(3));
    let replacement_id = PaneId::Terminal(TerminalId::new(1));
    tab.floating_panes.toggle_show_panes(true);
    let replacement = tab.tiled_panes.remove_pane(replacement_id).unwrap();
    tab.floating_panes.focus_pane(old, client);
    tab.floating_panes.focus_pane(other, client);
    let base = tab.floating_panes.get(&old).unwrap().position_and_size();
    tab.floating_panes
        .set_pane_being_moved_with_mouse(old, Position::new(base.y as i32, base.x as u16));
    tab.floating_panes.toggle_pane_fullscreen(old);
    let removed = tab.floating_panes.replace_pane(old, replacement).unwrap();
    assert_eq!(removed.pid(), old);
    assert_eq!(
        tab.floating_panes.fullscreen_pane_id(),
        Some(replacement_id)
    );
    assert_eq!(
        tab.floating_panes.active_pane_id(client),
        Some(replacement_id)
    );
    tab.floating_panes.unset_fullscreen();
    assert_eq!(
        tab.floating_panes
            .get(&replacement_id)
            .unwrap()
            .position_and_size(),
        base
    );
    tab.floating_panes
        .move_pane_to_position(&Position::new(base.y as i32 + 1, base.x as u16 + 1));
    assert_ne!(
        tab.floating_panes
            .get(&replacement_id)
            .unwrap()
            .position_and_size(),
        base
    );
    tab.floating_panes.focus_pane(other, client);
    tab.floating_panes.focus_last_pane(client);
    assert_eq!(
        tab.floating_panes.active_pane_id(client),
        Some(replacement_id)
    );
    tab.floating_panes.toggle_pane_fullscreen(replacement_id);
    tab.floating_panes.remove_pane(replacement_id).unwrap();
    assert!(!tab.floating_panes.fullscreen_is_active());
    assert!(!tab.floating_panes.pane_is_being_moved_with_mouse());
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(other));
    assert_eq!(tab.floating_panes.rendered_pane_ids(), vec![other]);
    tab.floating_panes.focus_last_pane(client);
    assert_eq!(tab.floating_panes.active_pane_id(client), Some(other));
}

#[test]
fn readding_a_floating_pane_keeps_one_stack_entry() {
    let mut tab = create_tab_with_two_floating_panes();
    let pane_id = PaneId::Terminal(TerminalId::new(2));
    let pane = tab.floating_panes.remove_pane(pane_id).unwrap();
    tab.floating_panes.add_pane(pane_id, pane);
    // A replacement with the same id goes through add_pane in layout application.
    let mut source = create_tab_with_two_floating_panes();
    let pane = source.floating_panes.remove_pane(pane_id).unwrap();
    tab.floating_panes.add_pane(pane_id, pane);
    tab.floating_panes.toggle_show_panes(true);
    assert_eq!(
        tab.floating_panes
            .rendered_pane_ids()
            .iter()
            .filter(|id| **id == pane_id)
            .count(),
        1
    );
}

#[test]
fn mouse_resize_errors_clear_drag_state_on_motion_and_release() {
    use yinz_types::mouse::MouseEvent;
    let client = ClientId::new(1);
    let pane_id = PaneId::Terminal(TerminalId::new(2));
    for (overlay, release) in [(false, false), (false, true), (true, false), (true, true)] {
        let (sender, receiver) = unbounded();
        let mut senders = ThreadSenders::default();
        senders.replace_to_pty_writer(SenderWithContext::new(sender));
        let mut tab = create_new_tab_with_senders(
            Size {
                cols: 120,
                rows: 40,
            },
            false,
            senders,
        );
        if overlay {
            tab.new_pane(
                pane_id,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(client),
                None,
            )
            .unwrap();
        } else {
            tab.new_floating_pane(pane_id, None, None, false, true, None, None)
                .unwrap();
            tab.floating_panes.toggle_show_panes(true);
        }
        let geom = tab.get_pane_with_id(pane_id).unwrap().current_geom();
        let press = Position::new(geom.y as i32 + 2, geom.x as u16);
        tab.handle_mouse_event(&MouseEvent::new_left_press_with_ctrl_event(press), client)
            .unwrap();
        assert!(tab.pane_being_resized_with_mouse.is_some());
        drop(receiver);
        let end = Position::new(geom.y as i32 + 2, geom.x as u16 + 1);
        let event = if release {
            MouseEvent::new_left_release_with_ctrl_event(end)
        } else {
            MouseEvent::new_left_motion_with_ctrl_event(end)
        };
        assert!(tab.handle_mouse_event(&event, client).is_err());
        assert!(tab.pane_being_resized_with_mouse.is_none());
    }
}

#[test]
fn missing_fullscreen_targets_preserve_the_existing_fullscreen_pane() {
    let missing = PaneId::Terminal(TerminalId::new(99));
    let client = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let floating = PaneId::Terminal(TerminalId::new(2));
    tab.floating_panes.toggle_pane_fullscreen(floating);
    tab.floating_panes.toggle_pane_fullscreen(missing);
    tab.floating_panes.toggle_pane_no_ui_fullscreen(missing);
    assert_eq!(tab.floating_panes.fullscreen_pane_id(), Some(floating));
    let mut tab = create_new_tab(
        Size {
            cols: 120,
            rows: 20,
        },
        false,
    );
    tab.vertical_split(missing, None, None, client, None, None)
        .unwrap();
    let tiled = PaneId::Terminal(TerminalId::new(1));
    let missing = PaneId::Terminal(TerminalId::new(98));
    tab.tiled_panes.toggle_pane_fullscreen(tiled);
    tab.tiled_panes.toggle_pane_fullscreen(missing);
    tab.tiled_panes.toggle_pane_no_ui_fullscreen(missing);
    assert_eq!(tab.tiled_panes.fullscreen_pane_id(), Some(tiled));
}

#[test]
fn replacing_a_remembered_floating_pane_keeps_focus_last_working() {
    let mut tab = create_tab_with_two_floating_panes();
    let client = ClientId::new(1);
    let old = PaneId::Terminal(TerminalId::new(2));
    let other = PaneId::Terminal(TerminalId::new(3));
    let replacement_id = PaneId::Terminal(TerminalId::new(1));
    let third = PaneId::Terminal(TerminalId::new(4));
    tab.new_floating_pane(third, None, None, false, true, None, None)
        .unwrap();
    tab.floating_panes.toggle_show_panes(true);
    tab.floating_panes.focus_pane(old, client);
    tab.floating_panes.focus_pane(other, client);
    let replacement = tab.tiled_panes.remove_pane(replacement_id).unwrap();
    tab.floating_panes.replace_pane(old, replacement).unwrap();
    // A fallback to the most recent pane would choose this third pane, not the replacement.
    tab.floating_panes
        .get_mut(&third)
        .unwrap()
        .set_active_at(std::time::Instant::now());
    tab.floating_panes.focus_last_pane(client);
    assert_eq!(
        tab.floating_panes.active_pane_id(client),
        Some(replacement_id)
    );
}

#[test]
fn fixed_tiled_neighbor_resize_errors_clear_drag_state() {
    use crate::tab::{PaneEdge, PaneResizeState, ResizeDragTarget};
    use yinz_types::{mouse::MouseEvent, pane_size::Dimension};
    let client = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(1));
    let right = PaneId::Terminal(TerminalId::new(2));
    for release in [false, true] {
        let mut tab = create_new_tab(
            Size {
                cols: 120,
                rows: 20,
            },
            false,
        );
        tab.vertical_split(right, None, None, client, None, None)
            .unwrap();
        let pane = tab.tiled_panes.get_pane_mut(right).unwrap();
        let mut geom = pane.position_and_size();
        geom.cols = Dimension::fixed(geom.cols.as_usize());
        pane.set_geom(geom);
        let before = tab.tiled_panes.get_pane(left).unwrap().position_and_size();
        tab.pane_being_resized_with_mouse = Some(PaneResizeState {
            pane_id: left,
            edge: PaneEdge::Right,
            start_position: Position::new(5, 59),
            start_geom: before,
            target: ResizeDragTarget::Tiled,
        });
        let event = if release {
            MouseEvent::new_left_release_with_ctrl_event(Position::new(5, 60))
        } else {
            MouseEvent::new_left_motion_with_ctrl_event(Position::new(5, 60))
        };
        assert!(tab.handle_mouse_event(&event, client).is_err());
        assert!(tab.pane_being_resized_with_mouse.is_none());
        assert_eq!(
            tab.tiled_panes.get_pane(left).unwrap().position_and_size(),
            before
        );
    }
}
