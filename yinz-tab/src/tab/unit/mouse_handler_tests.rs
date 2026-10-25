use super::*;

fn mouse_event_context(mouse_scroll_resize: bool) -> MouseEventContext {
    MouseEventContext {
        pane_id_at_position: Some(PaneId::Terminal(TerminalId::new(1))),
        active_pane_id: Some(PaneId::Terminal(TerminalId::new(1))),
        floating_visible: false,
        pane_being_resized: false,
        selecting_with_mouse: false,
        pane_being_moved: None,
        seam_being_dragged: false,
        clicked_pane: None,
        advanced_mouse_actions: true,
        pinned_selectable: None,
        pinned_unselectable: None,
        focus_follows_mouse: false,
        mouse_click_through: false,
        mouse_scroll_resize,
        context_menu_enabled: false,
        context_menu_override_agent_mouse: true,
        passthrough_pane_id: None,
    }
}

/// A frame press on a pane that is either not an overlay, or a *resizable* (non-modal) one.
fn frame_click_on(pane_id: PaneId, is_overlay: bool) -> ClickedPaneDetails {
    ClickedPaneDetails {
        pane_id,
        on_frame: true,
        frame_intercepted: false,
        edge: Some(PaneEdge::TopLeft),
        is_floating: false,
        is_overlay,
        overlay_is_resizable: is_overlay,
        overlay_is_movable: true,
        on_seam: None,
        terminal_wants_mouse: false,
        terminal_emulator_wants_mouse: false,
        is_agent: false,
    }
}

fn frame_click_on_modal(pane_id: PaneId) -> ClickedPaneDetails {
    ClickedPaneDetails {
        overlay_is_resizable: false,
        ..frame_click_on(pane_id, true)
    }
}

/// An overlay drags its *own* collection. `should_start_moving` asks whether the floating
/// *layer* is up, not whether the clicked pane is on it - and `execute_move_floating_pane`
/// then throws the pane id away and re-resolves the position through `FloatingPanes` alone.
/// So an overlay routed through the floating branch drags whatever it is covering, which is
/// why the two are separate actions rather than one action with a flag.
#[test]
fn a_press_on_an_overlay_frame_starts_an_overlay_move_and_not_a_floating_one() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.floating_visible = true;
    context.pane_id_at_position = Some(overlay);
    context.clicked_pane = Some(frame_click_on(overlay, true));

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert!(
        matches!(action, MouseAction::StartMovingOverlay { .. }),
        "got {action:?}"
    );
    assert!(
        !matches!(action, MouseAction::StartMovingFloatingPane { .. }),
        "got {action:?}"
    );

    // the same click on a pane that really is under the visible layer still moves it
    context.clicked_pane = Some(frame_click_on(PaneId::Terminal(TerminalId::new(1)), false));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::StartMovingFloatingPane { .. }
    ));
}

/// An expanded overlay fills the screen, so `OverlayPanes::move_pane_with_mouse` refuses the
/// drag: routing the press there anyway set nothing moving, and the click that followed was
/// swallowed by a drag that had never started. It is an ordinary click on the pane instead -
/// and in particular not a fall-through to the floating branch, which would drag whatever the
/// overlay is covering.
#[test]
fn a_press_on_an_expanded_overlay_s_frame_is_a_click_and_not_a_move() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.floating_visible = true;
    context.pane_id_at_position = Some(overlay);
    context.clicked_pane = Some(ClickedPaneDetails {
        overlay_is_movable: false,
        overlay_is_resizable: false,
        ..frame_click_on(overlay, true)
    });

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert_eq!(
        action,
        MouseAction::FocusPane {
            pane_id: overlay,
            position: event.position,
        },
        "got {action:?}"
    );
}

/// An overlay is not on a layer, so nothing has to be raised for its frame to be draggable.
/// The floating branch would refuse this press outright.
#[test]
fn an_overlay_frame_drags_with_the_floating_layer_down() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.floating_visible = false;
    context.pane_id_at_position = Some(overlay);
    context.clicked_pane = Some(frame_click_on(overlay, true));

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::StartMovingOverlay { .. }
    ));
}

/// The seam between two linked overlays is on both their frames, so the seam check has to
/// come before the move: a press on the divider drags the divider, not the panel.
#[test]
fn a_press_on_a_linked_overlay_seam_starts_a_seam_drag_and_not_a_move() {
    let (left, right) = (
        PaneId::Plugin(PluginId::new(8)),
        PaneId::Plugin(PluginId::new(9)),
    );
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(right);
    let mut details = frame_click_on(right, true);
    details.on_seam = Some((left, right));
    context.clicked_pane = Some(details);

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert!(
        matches!(
            action,
            MouseAction::StartOverlaySeamDrag { left: l, right: r, .. } if l == left && r == right
        ),
        "got {action:?}"
    );

    // the same press on a frame cell that is not the seam still picks the panel up
    context.clicked_pane = Some(frame_click_on(right, true));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::StartMovingOverlay { .. }
    ));

    // and a ctrl-press on the seam is a seam drag too: the seam is the one frame edge that
    // is not the panel's, and resizing one half into the other is not a resize anybody wants
    let mut details = frame_click_on(right, true);
    details.on_seam = Some((left, right));
    context.clicked_pane = Some(details);
    let ctrl_press = MouseEvent {
        ctrl: true,
        ..MouseEvent::new_left_press_event(Position::new(1, 1))
    };
    assert!(matches!(
        MouseHandler::determine_mouse_action(&ctrl_press, &context).unwrap(),
        MouseAction::StartOverlaySeamDrag { .. }
    ));
    // while a ctrl-press on any other frame cell of the half is the resize it always was
    context.clicked_pane = Some(frame_click_on(right, true));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&ctrl_press, &context).unwrap(),
        MouseAction::StartResize { .. }
    ));
}

/// An in-flight seam drag routes motion and release to the seam, ahead of any move.
#[test]
fn an_in_flight_seam_drag_routes_to_the_seam() {
    let position = Position::new(3, 3);
    let mut context = mouse_event_context(false);
    context.seam_being_dragged = true;
    assert!(matches!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_left_motion_event(position),
            &context
        )
        .unwrap(),
        MouseAction::DragOverlaySeam { .. }
    ));
    assert!(matches!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_left_release_event(position),
            &context
        )
        .unwrap(),
        MouseAction::StopOverlaySeamDrag { .. }
    ));
}

/// The drag is the frame's, not the pane's: a press in an overlay's body is still a press in
/// a live terminal or plugin, and must not pick the popup up.
#[test]
fn a_press_inside_an_overlay_does_not_start_a_move() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.floating_visible = true;
    context.pane_id_at_position = Some(overlay);
    let mut details = frame_click_on(overlay, true);
    details.on_frame = false;
    details.edge = None;
    context.clicked_pane = Some(details);

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert!(
        !matches!(action, MouseAction::StartMovingOverlay { .. }),
        "got {action:?}"
    );
}

/// A plugin overlay's frame buttons keep working: the interception is checked before the drag,
/// as it is on the floating branch.
#[test]
fn an_intercepted_overlay_frame_press_is_not_a_move() {
    let overlay = PaneId::Plugin(PluginId::new(9));
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(overlay);
    let mut details = frame_click_on(overlay, true);
    details.frame_intercepted = true;
    context.clicked_pane = Some(details);

    let event = MouseEvent::new_left_press_event(Position::new(1, 1));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::FrameIntercepted { .. }
    ));
}

/// An in-flight drag routes motion and release to the collection that is holding it. Reading
/// the target off a single enum rather than two booleans is what makes this unambiguous.
#[test]
fn an_in_flight_drag_routes_to_the_collection_holding_it() {
    let position = Position::new(3, 3);
    let motion = MouseEvent::new_left_motion_event(position);
    let release = MouseEvent::new_left_release_event(position);

    let mut context = mouse_event_context(false);
    context.pane_being_moved = Some(MouseDragTarget::Overlay);
    assert!(matches!(
        MouseHandler::determine_mouse_action(&motion, &context).unwrap(),
        MouseAction::ContinueMovingOverlay { .. }
    ));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&release, &context).unwrap(),
        MouseAction::StopMovingOverlay { .. }
    ));

    context.pane_being_moved = Some(MouseDragTarget::Floating);
    assert!(matches!(
        MouseHandler::determine_mouse_action(&motion, &context).unwrap(),
        MouseAction::ContinueMovingFloatingPane { .. }
    ));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&release, &context).unwrap(),
        MouseAction::StopMovingFloatingPane { .. }
    ));
}

/// A *plain* frame press on an overlay is a move, never a resize - the floating-pane split,
/// which is what "resizable like a floating pane" means. With the layer down this press used
/// to fall to `StartResize` for an overlay, and `start_pane_resize_with_mouse` then looked in
/// the floating and tiled collections only, so the overlay id missed both and errored the
/// whole mouse event.
#[test]
fn a_plain_press_on_an_overlay_frame_does_not_start_a_resize() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(overlay);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(frame_click_on(overlay, true));

    let action = MouseHandler::determine_mouse_action(
        &MouseEvent::new_left_press_event(Position::new(1, 1)),
        &context,
    )
    .unwrap();
    assert!(
        !matches!(action, MouseAction::StartResize { .. }),
        "got {action:?}"
    );
}

/// The ctrl-drag half: a non-modal overlay resizes, and it resizes through its own collection
/// rather than being handed to the floating or tiled one, neither of which holds it.
#[test]
fn a_ctrl_press_on_an_overlay_frame_starts_an_overlay_resize() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(overlay);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(frame_click_on(overlay, true));

    let mut event = MouseEvent::new_left_press_event(Position::new(1, 1));
    event.ctrl = true;
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert!(
        matches!(
            action,
            MouseAction::StartResize {
                target: ResizeDragTarget::Overlay,
                ..
            }
        ),
        "got {action:?}"
    );
}

/// A modal keeps the size its plugin asked for, so the ctrl-drag is nothing at all - and in
/// particular not a `StartResize` that `start_pane_resize_with_mouse` would then fail to
/// resolve.
#[test]
fn a_ctrl_press_on_a_modal_frame_does_not_start_a_resize() {
    let modal = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(modal);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(frame_click_on_modal(modal));

    let mut event = MouseEvent::new_left_press_event(Position::new(1, 1));
    event.ctrl = true;
    let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
    assert!(matches!(action, MouseAction::NoAction), "got {action:?}");
}

/// A modal is still draggable - only its *size* is the plugin's.
#[test]
fn a_plain_press_on_a_modal_frame_still_starts_a_move() {
    let modal = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(false);
    context.pane_id_at_position = Some(modal);
    context.clicked_pane = Some(frame_click_on_modal(modal));

    let action = MouseHandler::determine_mouse_action(
        &MouseEvent::new_left_press_event(Position::new(1, 1)),
        &context,
    )
    .unwrap();
    assert!(
        matches!(action, MouseAction::StartMovingOverlay { .. }),
        "got {action:?}"
    );
}

/// The scroll half of the ctrl-drag pair above: the overlay under the pointer is the pane that
/// resizes. Without this the action still carried the overlay's id, but
/// `handle_resize_scroll` knew only the floating and tiled collections and fell through to
/// resizing whatever the client was focused on.
#[test]
fn a_ctrl_scroll_over_a_resizable_overlay_asks_for_a_resize() {
    let overlay = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(true);
    context.pane_id_at_position = Some(overlay);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(ClickedPaneDetails {
        is_overlay: true,
        overlay_is_resizable: true,
        ..content_click_on(overlay)
    });

    let position = Position::new(1, 1);
    assert_eq!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_ctrl_scroll_up_event(position),
            &context
        )
        .unwrap(),
        MouseAction::ResizeScrollUp { pane_id: overlay }
    );
    assert_eq!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_ctrl_scroll_down_event(position),
            &context
        )
        .unwrap(),
        MouseAction::ResizeScrollDown { pane_id: overlay }
    );
}

/// A modal's size is the plugin's, so the scroll is nothing at all - and in particular not a
/// plain scroll and not a resize of the pane behind it, which is where a fall-through would
/// land it.
#[test]
fn a_ctrl_scroll_over_a_modal_does_nothing() {
    let modal = PaneId::Terminal(TerminalId::new(9));
    let mut context = mouse_event_context(true);
    context.pane_id_at_position = Some(modal);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(ClickedPaneDetails {
        is_overlay: true,
        overlay_is_resizable: false,
        ..content_click_on(modal)
    });

    let position = Position::new(1, 1);
    for event in [
        MouseEvent::new_ctrl_scroll_up_event(position),
        MouseEvent::new_ctrl_scroll_down_event(position),
    ] {
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::NoAction
        );
    }
}

#[test]
fn disabled_ctrl_scroll_does_not_fall_through_to_regular_scrolling() {
    let context = mouse_event_context(false);
    let position = Position::new(1, 1);
    let events = [
        MouseEvent::new_ctrl_scroll_up_event(position),
        MouseEvent::new_ctrl_scroll_down_event(position),
    ];

    for event in events {
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::NoAction
        );
    }
}

/// A content click - not on the frame, nothing intercepted, no mouse tracking. The shape every
/// context-menu case below varies one field of.
fn content_click_on(pane_id: PaneId) -> ClickedPaneDetails {
    ClickedPaneDetails {
        pane_id,
        on_frame: false,
        frame_intercepted: false,
        edge: None,
        is_floating: false,
        is_overlay: false,
        overlay_is_resizable: false,
        overlay_is_movable: true,
        on_seam: None,
        terminal_wants_mouse: false,
        terminal_emulator_wants_mouse: false,
        is_agent: false,
    }
}

fn right_press_context(pane_id: PaneId, enabled: bool) -> MouseEventContext {
    let mut context = mouse_event_context(false);
    context.context_menu_enabled = enabled;
    context.pane_id_at_position = Some(pane_id);
    context.clicked_pane = Some(content_click_on(pane_id));
    context
}

#[test]
fn agent_override_preserves_other_mouse_actions() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let mut context = right_press_context(pane_id, true);
    context.clicked_pane = Some(ClickedPaneDetails {
        is_agent: true,
        terminal_wants_mouse: true,
        terminal_emulator_wants_mouse: true,
        ..content_click_on(pane_id)
    });
    let p = Position::new(4, 7);
    for event in [
        MouseEvent::new_left_press_event(p),
        MouseEvent::new_left_motion_event(p),
        MouseEvent::new_left_release_event(p),
        MouseEvent::new_middle_press_event(p),
        MouseEvent::new_middle_motion_event(p),
        MouseEvent::new_middle_release_event(p),
        MouseEvent::new_scroll_up_event(p),
        MouseEvent::new_scroll_down_event(p),
        MouseEvent::new_right_press_with_alt_event(p),
        MouseEvent {
            left: false,
            ..MouseEvent::new_left_motion_event(p)
        },
    ] {
        context.context_menu_override_agent_mouse = false;
        let expected = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        context.context_menu_override_agent_mouse = true;
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            expected,
            "{event:?}"
        );
    }
    context.passthrough_pane_id = Some(pane_id);
    let event = MouseEvent::new_right_press_with_alt_event(p);
    assert_eq!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::SendToTerminal { pane_id, event },
    );
    context.clicked_pane.as_mut().unwrap().frame_intercepted = true;
    assert!(!matches!(
        MouseHandler::determine_mouse_action(&MouseEvent::new_right_press_event(p), &context)
            .unwrap(),
        MouseAction::OpenContextMenu { .. }
    ));
}

#[test]
fn a_right_press_opens_the_context_menu_when_it_is_enabled() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let context = right_press_context(pane_id, true);
    let event = MouseEvent::new_right_press_event(Position::new(4, 7));
    assert_eq!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::OpenContextMenu {
            pane_id,
            position: Position::new(4, 7),
            on_frame: false,
        },
        "and the position is the absolute click, which is what anchors the menu"
    );
}

/// The regression guard for every existing right-click test: with the option off nothing at
/// all about right-click changes.
#[test]
fn a_right_press_with_the_option_off_still_reaches_the_terminal() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let context = right_press_context(pane_id, false);
    let event = MouseEvent::new_right_press_event(Position::new(4, 7));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::SendToTerminal { .. }
    ));
}

#[test]
fn a_right_press_in_a_pane_tracking_the_mouse_belongs_to_the_program() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let mut context = right_press_context(pane_id, true);
    context.clicked_pane = Some(ClickedPaneDetails {
        terminal_emulator_wants_mouse: true,
        ..content_click_on(pane_id)
    });
    let event = MouseEvent::new_right_press_event(Position::new(4, 7));
    assert!(
        matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::SendToTerminal { .. }
        ),
        "an editor that asked for right-click keeps it"
    );
}

/// A background TUI still owns its own right-click, so this must not be conditioned on the
/// pane being active the way `terminal_wants_mouse` is.
#[test]
fn mouse_tracking_wins_even_in_a_pane_that_is_not_focused() {
    let clicked = PaneId::Terminal(TerminalId::new(2));
    let mut context = right_press_context(clicked, true);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    context.clicked_pane = Some(ClickedPaneDetails {
        terminal_emulator_wants_mouse: true,
        ..content_click_on(clicked)
    });
    let event = MouseEvent::new_right_press_event(Position::new(4, 7));
    assert_eq!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::NoAction,
        "not the menu, and not another pane's terminal either"
    );
}

#[test]
fn mouse_tracking_does_not_block_a_frame_menu() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let mut context = right_press_context(pane_id, true);
    context.clicked_pane = Some(ClickedPaneDetails {
        on_frame: true,
        terminal_emulator_wants_mouse: true,
        ..content_click_on(pane_id)
    });
    let event = MouseEvent::new_right_press_event(Position::new(0, 3));
    assert!(matches!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::OpenContextMenu { on_frame: true, .. }
    ));
}

#[test]
fn a_right_press_on_an_unfocused_pane_still_opens_a_menu() {
    let clicked = PaneId::Terminal(TerminalId::new(2));
    let mut context = right_press_context(clicked, true);
    context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
    assert!(matches!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_right_press_event(Position::new(4, 7)),
            &context
        )
        .unwrap(),
        MouseAction::OpenContextMenu { .. }
    ));
}

/// The release that follows the opening press must not open a second menu.
#[test]
fn only_the_press_opens_a_menu_not_the_release() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let context = right_press_context(pane_id, true);
    let release = MouseEvent::new_right_release_event(Position::new(4, 7));
    assert!(!matches!(
        MouseHandler::determine_mouse_action(&release, &context).unwrap(),
        MouseAction::OpenContextMenu { .. }
    ));
}

#[test]
fn a_right_press_on_a_frame_opens_a_menu_and_says_it_was_the_frame() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let mut context = right_press_context(pane_id, true);
    context.clicked_pane = Some(ClickedPaneDetails {
        on_frame: true,
        ..content_click_on(pane_id)
    });
    assert_eq!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_right_press_event(Position::new(0, 3)),
            &context
        )
        .unwrap(),
        MouseAction::OpenContextMenu {
            pane_id,
            position: Position::new(0, 3),
            on_frame: true,
        }
    );
}

#[test]
fn a_frame_that_claims_its_own_mouse_events_keeps_the_right_click() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let mut context = right_press_context(pane_id, true);
    context.clicked_pane = Some(ClickedPaneDetails {
        on_frame: true,
        frame_intercepted: true,
        ..content_click_on(pane_id)
    });
    assert!(!matches!(
        MouseHandler::determine_mouse_action(
            &MouseEvent::new_right_press_event(Position::new(0, 3)),
            &context
        )
        .unwrap(),
        MouseAction::OpenContextMenu { .. }
    ));
}

/// Alt+right is `Ungroup`, and was before menus existed. The menu branch sits after the alt
/// branch precisely so this keeps working.
#[test]
fn alt_right_still_ungroups_rather_than_opening_a_menu() {
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    let context = right_press_context(pane_id, true);
    let mut event = MouseEvent::new_right_press_event(Position::new(4, 7));
    event.alt = true;
    assert_eq!(
        MouseHandler::determine_mouse_action(&event, &context).unwrap(),
        MouseAction::Ungroup
    );
}
