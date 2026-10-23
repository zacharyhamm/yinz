use super::*;

#[test]
fn fullscreen_overlay_divider_drags_with_complete_borders_under_title_only_frames() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(2));
    let right = PaneId::Terminal(TerminalId::new(3));
    for ctrl in [false, true] {
        for grab_column in [39] {
            let mut tab = create_new_tab(size, ModeInfo::default());
            tab.set_pane_frames(PaneFrameStyle::Titles);
            let coordinates = |x| FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(x)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Fixed(30)),
                height: Some(PercentOrFixed::Fixed(10)),
                ..Default::default()
            };
            for (pane_id, x) in [(left, 2), (right, 32)] {
                tab.new_pane(
                    pane_id,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates(x)))),
                    Some(client_id),
                    None,
                )
                .unwrap();
            }
            tab.change_overlay_coordinates_batch(
                vec![(left, coordinates(2)), (right, coordinates(32))],
                false,
            )
            .unwrap();
            tab.create_multipane(
                [left, right],
                yinz_types::multipane::MultipaneSplit {
                    direction: yinz_types::multipane::MultipaneDirection::LeftRight,
                    ratio: 5000,
                },
                0,
                OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                    width: Some(PercentOrFixed::Fixed(60)),
                    ..coordinates(2)
                })),
            )
            .unwrap();
            tab.handle_pty_bytes(TerminalId::new(2), b"Agents".to_vec())
                .unwrap();
            tab.handle_pty_bytes(TerminalId::new(3), b"Chat".to_vec())
                .unwrap();
            tab.toggle_active_pane_fullscreen(client_id);
            if !ctrl && grab_column == 39 {
                let mut output = Output::default();
                tab.render(&mut output, None).unwrap();
                assert_snapshot!(take_snapshot(
                    output.serialize().unwrap().get(&client_id).unwrap(),
                    size.rows,
                    size.cols,
                    Palette::default(),
                ));
            }

            let event = |event| MouseEvent { ctrl, ..event };
            tab.handle_mouse_event(
                &event(MouseEvent::new_left_press_event(Position::new(
                    5,
                    grab_column,
                ))),
                client_id,
            )
            .unwrap();
            assert!(tab.overlay_panes.seam_is_being_dragged());
            for (column, expected_cols) in [(grab_column + 16, 54), (grab_column - 16, 22)] {
                let effect = tab
                    .handle_mouse_event(
                        &event(MouseEvent::new_left_motion_event(Position::new(5, column))),
                        client_id,
                    )
                    .unwrap();
                assert!(effect.state_changed);
                assert_eq!(
                    tab.overlay_panes
                        .get(&left)
                        .unwrap()
                        .current_geom()
                        .cols
                        .as_usize(),
                    expected_cols
                );
                assert_eq!(tab.get_active_pane_id(client_id), Some(right));
            }
            tab.handle_mouse_event(
                &event(MouseEvent::new_left_release_event(Position::new(
                    5,
                    grab_column - 16,
                ))),
                client_id,
            )
            .unwrap();
            assert!(!tab.overlay_panes.seam_is_being_dragged());
            tab.toggle_active_pane_fullscreen(client_id);
            for (pane_id, x, cols) in [(left, 3, 16), (right, 20, 41)] {
                let pane = tab.overlay_panes.get(&pane_id).unwrap();
                let geom = pane.current_geom();
                assert_eq!(
                    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize()),
                    (x, 3, cols, 8)
                );
            }
        }
    }
}

#[test]
fn plugin_wheel_receives_its_relative_position_before_each_scroll() {
    use yinz_types::data::{Event, Mouse};

    for overlay in [false, true] {
        let client_id = ClientId::new(1);
        let plugin_id = crate::PluginId::new(2);
        let pane_id = PaneId::Plugin(plugin_id);
        let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
            Size {
                cols: 121,
                rows: 20,
            },
            ModeInfo::default(),
        );
        let placement = if overlay {
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(10)),
                    y: Some(PercentOrFixed::Fixed(2)),
                    width: Some(PercentOrFixed::Fixed(20)),
                    height: Some(PercentOrFixed::Fixed(10)),
                    ..Default::default()
                },
            )))
        } else {
            NewPanePlacement::Tiled {
                direction: None,
                borderless: None,
            }
        };
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            placement,
            Some(client_id),
            None,
        )
        .unwrap();
        let pane = if overlay {
            tab.overlay_panes.get_pane(pane_id).unwrap()
        } else {
            tab.tiled_panes.get_pane(pane_id).unwrap()
        };
        let point = Position::new(
            (pane.get_content_y() + 2) as i32,
            (pane.get_content_x() + 3) as u16,
        );
        let next_point = Position::new((point.line() + 1) as i32, (point.column() + 1) as u16);
        let focused = tab.get_active_pane_id(client_id);
        // No preceding hover, then a second wheel at a different position without motion.
        tab.handle_scrollwheel_up(&point, 3, client_id).unwrap();
        tab.handle_scrollwheel_down(&next_point, 2, client_id)
            .unwrap();
        assert_eq!(tab.get_active_pane_id(client_id), focused);
        let events: Vec<_> = receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events,
                _ => Vec::new(),
            })
            .filter_map(|(pid, client, event)| match event {
                Event::Mouse(mouse) => Some((pid, client, mouse)),
                _ => None,
            })
            .collect();
        assert_eq!(
            events,
            vec![
                (Some(plugin_id), Some(client_id), Mouse::Hover(2, 3)),
                (Some(plugin_id), Some(client_id), Mouse::ScrollUp(3)),
                (Some(plugin_id), Some(client_id), Mouse::Hover(3, 4)),
                (Some(plugin_id), Some(client_id), Mouse::ScrollDown(2)),
            ],
            "overlay={overlay}"
        );
    }
}

#[test]
fn focused_plugin_receives_a_right_press_on_its_content_as_a_right_click() {
    use yinz_types::data::{Event, Mouse};

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Fixed(20)),
        height: Some(PercentOrFixed::Fixed(10)),
        ..Default::default()
    };
    let placements = [
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        NewPanePlacement::Floating(Some(coordinates.clone())),
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates))),
    ];
    for placement in placements {
        let label = format!("{placement:?}");
        let is_tiled = matches!(placement, NewPanePlacement::Tiled { .. });
        let client_id = ClientId::new(1);
        let plugin_id = crate::PluginId::new(2);
        let pane_id = PaneId::Plugin(plugin_id);
        let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
            Size {
                cols: 121,
                rows: 20,
            },
            ModeInfo::default(),
        );
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            placement,
            Some(client_id),
            None,
        )
        .unwrap();
        assert_eq!(tab.get_active_pane_id(client_id), Some(pane_id), "{label}");
        let pane = tab.get_pane_with_id(pane_id).unwrap();
        let line = (pane.get_content_y() + 2) as i32;
        let content = Position::new(line, (pane.get_content_x() + 3) as u16);
        let left_border = Position::new(line, pane.x() as u16);
        assert!(pane.position_is_on_frame(&left_border), "{label}");
        let mouse_events = || -> Vec<_> {
            receiver
                .try_iter()
                .flat_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(events) => events,
                    _ => Vec::new(),
                })
                .filter_map(|(pid, client, event)| match event {
                    Event::Mouse(mouse) => Some((pid, client, mouse)),
                    _ => None,
                })
                .collect()
        };
        mouse_events();

        // Only the press is a click: the drag and release after it, the middle button and a
        // right press on the frame send nothing.
        for event in [
            MouseEvent::new_right_press_event(content),
            MouseEvent::new_right_motion_event(content),
            MouseEvent::new_right_release_event(content),
            MouseEvent::new_middle_press_event(content),
            MouseEvent::new_middle_release_event(content),
            MouseEvent::new_right_press_event(left_border),
            MouseEvent::new_right_release_event(left_border),
        ] {
            tab.handle_mouse_event(&event, client_id).unwrap();
        }
        assert_eq!(
            mouse_events(),
            vec![(Some(plugin_id), Some(client_id), Mouse::RightClick(2, 3))],
            "{label}"
        );

        // A context menu takes the right press instead.
        tab.update_context_menu_on_right_click(true);
        let effect = tab
            .handle_mouse_event(&MouseEvent::new_right_press_event(content), client_id)
            .unwrap();
        assert!(effect.open_context_menu.is_some(), "{label}");
        assert_eq!(mouse_events(), vec![], "{label}");
        tab.update_context_menu_on_right_click(false);

        // An unfocused plugin gets no right click. Only the tiled plugin stays on screen when
        // the terminal beside it takes focus.
        if is_tiled {
            let terminal = tab
                .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
                .unwrap();
            let terminal_content = Position::new(
                (terminal.get_content_y() + 2) as i32,
                (terminal.get_content_x() + 3) as u16,
            );
            tab.handle_mouse_event(
                &MouseEvent::new_left_press_event(terminal_content),
                client_id,
            )
            .unwrap();
            tab.handle_mouse_event(
                &MouseEvent::new_left_release_event(terminal_content),
                client_id,
            )
            .unwrap();
            assert_eq!(
                tab.get_active_pane_id(client_id),
                Some(PaneId::Terminal(TerminalId::new(1)))
            );
            mouse_events();
            tab.handle_mouse_event(&MouseEvent::new_right_press_event(content), client_id)
                .unwrap();
            assert_eq!(mouse_events(), vec![]);
        }
    }
}

#[test]
fn multipane_plugin_hears_no_right_click_on_the_container_frame_or_divider() {
    use yinz_types::data::{Event, Mouse};

    let client_id = ClientId::new(1);
    let left_id = crate::PluginId::new(2);
    let left = PaneId::Plugin(left_id);
    let right = PaneId::Plugin(crate::PluginId::new(3));
    let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let coordinates = |x| FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(x)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(10)),
        ..Default::default()
    };
    for (pane_id, x) in [(left, 2), (right, 32)] {
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates(x)))),
            Some(client_id),
            None,
        )
        .unwrap();
    }
    tab.create_multipane(
        [left, right],
        yinz_types::multipane::MultipaneSplit {
            direction: yinz_types::multipane::MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
            width: Some(PercentOrFixed::Fixed(60)),
            ..coordinates(2)
        })),
    )
    .unwrap();
    // The first child is the one a press on the container's frame or divider routes to.
    tab.focus_pane_with_id(left, false, false, client_id)
        .unwrap();
    assert_eq!(tab.get_active_pane_id(client_id), Some(left));
    let outer = tab.overlay_panes.outer_geom(left).unwrap();
    let pane = tab.overlay_panes.get_pane(left).unwrap();
    let line = (pane.get_content_y() + 2) as i32;
    let column = (pane.get_content_x() + 3) as u16;
    let content = Position::new(line, column);
    let divider = Position::new(line, (pane.x() + pane.cols()) as u16);
    assert!(tab.overlay_panes.seam_at(&divider).is_some());
    let frame = [
        Position::new(line, outer.x as u16),
        Position::new(outer.y as i32, column),
        Position::new((outer.y + outer.rows.as_usize() - 1) as i32, column),
        Position::new(line, (outer.x + outer.cols.as_usize() - 1) as u16),
        divider,
    ];
    // Each lies outside the child's own geometry, so its own frame check cannot see it.
    for position in frame {
        assert!(!pane.contains(&position), "{position:?}");
        assert!(
            tab.overlay_panes.position_is_on_frame(left, &position)
                || tab.overlay_panes.seam_at(&position).is_some(),
            "{position:?}"
        );
    }
    let mouse_events = || -> Vec<_> {
        receiver
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(events) => events,
                _ => Vec::new(),
            })
            .filter_map(|(pid, client, event)| match event {
                Event::Mouse(mouse) => Some((pid, client, mouse)),
                _ => None,
            })
            .collect()
    };
    mouse_events();

    for position in frame {
        for event in [
            MouseEvent::new_right_press_event(position),
            MouseEvent::new_right_release_event(position),
        ] {
            tab.handle_mouse_event(&event, client_id).unwrap();
        }
        assert_eq!(mouse_events(), vec![], "{position:?}");
        assert_eq!(
            tab.get_active_pane_id(client_id),
            Some(left),
            "{position:?}"
        );
    }

    // Hover still follows the pointer onto the divider, and the content still takes a click.
    tab.handle_mouse_event(&MouseEvent::new_buttonless_motion(divider), client_id)
        .unwrap();
    let hovers = mouse_events();
    assert!(
        matches!(
            hovers.as_slice(),
            [(Some(pid), Some(client), Mouse::Hover(..))] if *pid == left_id && *client == client_id
        ),
        "{hovers:?}"
    );
    tab.handle_mouse_event(&MouseEvent::new_right_press_event(content), client_id)
        .unwrap();
    assert_eq!(
        mouse_events(),
        vec![(Some(left_id), Some(client_id), Mouse::RightClick(2, 3))]
    );
}

#[test]
fn hidden_surface_overlay_owns_mouse_events() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                ..Default::default()
            },
        ))),
        Some(client_id),
        None,
    )
    .unwrap();

    assert!(!tab.floating_panes.panes_are_visible());
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(3, 11)),
        client_id,
    )
    .unwrap();

    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));
}

#[test]
fn test_left_click_on_inactive_tiled_pane_changes_focus() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane".as_bytes()),
    )
    .unwrap();

    assert_eq!(tab.get_active_pane_id(client_id), Some(new_pane_id));

    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_event(Position::new(5, 30)),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

#[test]
fn test_left_release_after_selection_copies_to_clipboard() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let _output = Output::default();

    // Fill the pane with some text content
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Selectable text content here".as_bytes()),
    )
    .unwrap();

    // Start text selection with left press
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(1, 5)),
        client_id,
    )
    .unwrap();

    // Drag to create selection
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(1, 15)),
        client_id,
    )
    .unwrap();

    // Release should trigger clipboard copy
    let release_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_release_event(Position::new(1, 15)),
            client_id,
        )
        .unwrap();

    // Verify clipboard message was sent
    assert!(release_effect.leave_clipboard_message);
}

#[test]
fn triple_click_without_motion_requests_render() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let position = Position::new(1, 5);

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Selectable text content here".as_bytes()),
    )
    .unwrap();

    for _ in 0..2 {
        tab.handle_mouse_event(&MouseEvent::new_left_press_event(position), client_id)
            .unwrap();
        tab.handle_mouse_event(&MouseEvent::new_left_release_event(position), client_id)
            .unwrap();
    }

    let effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(position), client_id)
        .unwrap();

    assert!(effect.state_changed);
}

#[test]
fn configured_word_separators_reach_the_pane_on_double_click() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let position = Position::new(1, 6);

    tab.update_selection_options(true, ":".to_owned());
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("foo:bar baz".as_bytes()),
    )
    .unwrap();

    tab.handle_mouse_event(&MouseEvent::new_left_press_event(position), client_id)
        .unwrap();
    tab.handle_mouse_event(&MouseEvent::new_left_release_event(position), client_id)
        .unwrap();
    tab.handle_mouse_event(&MouseEvent::new_left_press_event(position), client_id)
        .unwrap();
    tab.handle_mouse_event(&MouseEvent::new_left_release_event(position), client_id)
        .unwrap();

    let selected_text = tab
        .get_active_pane(client_id)
        .unwrap()
        .get_selected_text(client_id);
    assert_eq!(selected_text, Some("bar".to_owned()));
}

#[test]
fn test_ctrl_click_on_tiled_pane_edge_starts_resize() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Click on the edge between panes with Ctrl
    // For a vertical split at cols=121, the edge is around column 60-61
    let edge_position = Position::new(5, 60);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_ctrl_event(edge_position),
            client_id,
        )
        .unwrap();

    // Verify resize started
    assert!(effect.state_changed);
}

#[test]
fn test_ctrl_drag_resizes_tiled_pane_horizontally() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Left pane content".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane content".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let edge_position = Position::new(5, 60);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(edge_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(5, 61);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_drag_resizes_tiled_pane_vertically() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.horizontal_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Top pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Bottom pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let edge_position = Position::new(10, 60);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(edge_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(11, 60);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_drag_resizes_unfocused_tiled_pane_vertically() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.horizontal_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Top pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Bottom pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let edge_position = Position::new(10, 60);
    let _ = tab.focus_pane_with_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        false,
        false,
        client_id,
    ); // focus top pane
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(edge_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(11, 60);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    let focused_pane = tab.get_active_pane_id(client_id).unwrap();

    assert_ne!(focused_pane, new_pane_id, "resize did not change focus");
    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_click_on_pane_body_does_nothing() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Click in the center of a pane (not on an edge)
    let center_position = Position::new(10, 30);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_ctrl_event(center_position),
            client_id,
        )
        .unwrap();

    // Should not trigger any state change
    assert!(!effect.state_changed);
}

#[test]
fn test_ctrl_drag_resizes_floating_pane_from_edge() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(10)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let edge_position = Position::new(7, 14); // Approximate right edge
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(edge_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(7, 15);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_drag_resizes_floating_pane_from_corner() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(10)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Corner resize test".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let corner_position = Position::new(14, 14);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(corner_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(15, 15);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_drag_resizes_pinned_floating_pane_when_floating_panes_not_shown() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(25)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(true),
        borderless: Some(false),
    };

    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Corner resize test".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    eprintln!("{}", snapshot_before);

    let corner_position = Position::new(14, 29);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(corner_position),
        client_id,
    )
    .unwrap();

    let motion_position = Position::new(15, 30);
    let motion_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(motion_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    eprintln!("{}", snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_click_on_floating_pin_button_toggles_pin() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    // Toggle floating panes mode and create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Pin toggle test".as_bytes()),
    )
    .unwrap();

    // Render before pin toggle
    tab.render(&mut output, None).unwrap();

    // Calculate pin button position (3 cells left of top-right corner of floating pane)
    // Default floating pane is roughly at y=5, right edge around x=90
    // Pin button should be at approximately (5, 87)
    let pin_position = Position::new(5, 87);

    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_ctrl_event(pin_position),
            client_id,
        )
        .unwrap();

    // Verify state changed (pin toggle should trigger this)
    assert!(effect.state_changed);

    // Render after pin toggle
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Snapshot should show pin indicator change
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_click_on_floating_frame_not_on_pin_starts_resize() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Toggle floating panes mode and create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Frame click test".as_bytes()),
    )
    .unwrap();

    // Click on frame but NOT on pin button location
    // Use left edge of frame instead
    let frame_position = Position::new(5, 30);

    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_ctrl_event(frame_position),
            client_id,
        )
        .unwrap();

    // Should start resize (if on edge), state_changed should be true
    assert!(effect.state_changed);
}

#[test]
fn test_left_drag_moves_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    // Toggle floating panes mode and create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Moving pane".as_bytes()),
    )
    .unwrap();

    // Render before move
    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Click on frame (not ctrl, not pin location) to start move
    let frame_position = Position::new(5, 30);
    let press_effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(frame_position), client_id)
        .unwrap();

    // Drag to move the pane
    let motion_position = Position::new(6, 31);
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(motion_position),
        client_id,
    )
    .unwrap();

    // Release
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(motion_position),
        client_id,
    )
    .unwrap();

    // Verify state changed
    assert!(press_effect.state_changed);

    // Render after move
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Snapshots should be different
    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_left_click_on_floating_pane_changes_focus() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let floating_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));

    let coordinates_1 = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(10)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    let coordinates_2 = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(20)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(10)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates_1)),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        floating_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates_2)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Pane 1".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(3), Vec::from("Pane 2".as_bytes()))
        .unwrap();

    assert_eq!(tab.get_active_pane_id(client_id), Some(floating_pane_id_2));

    let pane1_position = Position::new(7, 7);
    let effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(pane1_position), client_id)
        .unwrap();

    assert!(effect.state_changed);
    assert_eq!(tab.get_active_pane_id(client_id), Some(floating_pane_id_1));
}

#[test]
fn test_left_click_on_pinned_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let floating_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let mut output = Output::default();

    let coordinates_1 = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    let coordinates_2 = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(60)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(false),
    };

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();

    // will be pinned
    tab.new_pane(
        floating_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates_1)),
        Some(client_id),
        None,
    )
    .unwrap();

    // will not be pinned
    tab.new_pane(
        floating_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates_2)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Pinned pane".as_bytes()),
    )
    .unwrap();

    // render once so that the client's frame will be registered
    tab.render(&mut output, None).unwrap();

    let pin_position = Position::new(5, 31);
    tab.handle_mouse_event(&MouseEvent::new_left_press_event(pin_position), client_id)
        .unwrap();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();

    let pane_position = Position::new(6, 25);
    let _effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(pane_position), client_id)
        .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Verify pinned pane is still visible in snapshot
    assert_snapshot!(snapshot.to_string());
}

#[test]
fn test_alt_left_click_toggles_pane_group() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Alt+click on a pane to toggle group
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_alt_event(Position::new(5, 30)),
            client_id,
        )
        .unwrap();

    // Verify group_toggle effect returned
    assert!(effect.group_toggle.is_some());
    assert_eq!(
        effect.group_toggle,
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

#[test]
fn test_alt_left_drag_adds_panes_to_group() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(3));
    let _output = Output::default();

    // Create two vertical splits (3 panes)
    tab.vertical_split(new_pane_id_2, None, None, client_id, None, None)
        .unwrap();
    tab.vertical_split(new_pane_id_3, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Pane 1".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Pane 2".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(3), Vec::from("Pane 3".as_bytes()))
        .unwrap();

    // Alt+press on first pane
    let press_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_alt_event(Position::new(5, 10)),
            client_id,
        )
        .unwrap();
    assert!(press_effect.group_toggle.is_some());

    // Alt+drag to second pane
    let motion_effect_1 = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_alt_event(Position::new(5, 40)),
            client_id,
        )
        .unwrap();

    // Verify group_add effect
    assert!(motion_effect_1.group_add.is_some());

    // Alt+drag to third pane
    let motion_effect_2 = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_alt_event(Position::new(5, 80)),
            client_id,
        )
        .unwrap();

    // Verify group_add effect
    assert!(motion_effect_2.group_add.is_some());
}

#[test]
fn test_right_alt_click_ungroups_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // First, create a group with Alt+click
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_alt_event(Position::new(5, 30)),
        client_id,
    )
    .unwrap();

    // Right+Alt click to ungroup
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_right_press_with_alt_event(Position::new(5, 30)),
            client_id,
        )
        .unwrap();

    // Verify ungroup effect returned
    assert!(effect.ungroup);
}

#[test]
fn alt_click_is_forwarded_to_a_pane_the_client_is_descended_into() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[?1002h\u{1b}[?1006h".as_bytes()),
    )
    .unwrap();

    let effect = tab
        .handle_mouse_event_with_passthrough(
            &MouseEvent::new_left_press_with_alt_event(Position::new(5, 71)),
            client_id,
            Some(PaneId::Terminal(crate::TerminalId::new(1))),
        )
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec!["\u{1b}[<8;71;5M".to_string()],
        "the alt bit must survive into the SGR report written to the descended pane"
    );
    assert!(
        effect.group_toggle.is_none(),
        "an alt click inside a descended pane must not toggle a host pane group"
    );
    assert!(
        effect.group_add.is_none(),
        "an alt click inside a descended pane must not add to a host pane group"
    );
    assert!(
        !effect.ungroup,
        "an alt click inside a descended pane must not ungroup host panes"
    );
}

#[test]
fn alt_click_still_groups_a_mouse_tracking_pane_the_client_is_not_descended_into() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[?1002h\u{1b}[?1006h".as_bytes()),
    )
    .unwrap();

    let effect = tab
        .handle_mouse_event_with_passthrough(
            &MouseEvent::new_left_press_with_alt_event(Position::new(5, 71)),
            client_id,
            None,
        )
        .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        effect.group_toggle,
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "without a descend, alt click keeps grouping panes even when the pane tracks the mouse"
    );
    assert!(
        pty_instruction_bus.clone_output().is_empty(),
        "without a descend, nothing should be written to the pane"
    );
}

#[test]
fn alt_wheel_up_is_forwarded_to_a_pane_the_client_is_descended_into() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);

    let mut pty_instruction_bus = MockPtyInstructionBus::new();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        pty_instruction_bus.pty_write_sender(),
    );
    pty_instruction_bus.start();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\u{1b}[?1002h\u{1b}[?1006h".as_bytes()),
    )
    .unwrap();

    tab.handle_mouse_event_with_passthrough(
        &MouseEvent::new_alt_scroll_up_event(Position::new(5, 71)),
        client_id,
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
    )
    .unwrap();

    pty_instruction_bus.exit();

    assert_eq!(
        pty_instruction_bus.clone_output(),
        vec!["\u{1b}[<72;71;5M".to_string()],
        "alt wheel over a descended pane must reach the guest instead of jumping prompts"
    );
}

fn twenty_marked_commands() -> Vec<u8> {
    let mut content = String::new();
    for i in 0..20 {
        content.push_str(&format!(
            "\u{1b}]133;A\u{7}$ \u{1b}]133;B\u{7}cmd{i}\u{1b}]133;C\u{7}\r\nout{i}\u{1b}]133;D;0\u{7}\r\n"
        ));
    }
    content.into_bytes()
}

#[test]
fn alt_wheel_over_a_mouse_tracking_pane_is_forwarded_instead_of_jumping_prompts() {
    for (encoding, expected) in [
        ("", "\u{1b}[Mhg%"),
        ("\u{1b}[?1005h", "\u{1b}[Mhg%"),
        ("\u{1b}[?1006h", "\u{1b}[<72;71;5M"),
    ] {
        let size = Size {
            cols: 121,
            rows: 20,
        };
        let client_id = ClientId::new(1);

        let mut pty_instruction_bus = MockPtyInstructionBus::new();
        let mut tab = create_new_tab_with_mock_pty_writer(
            size,
            ModeInfo::default(),
            pty_instruction_bus.pty_write_sender(),
        );
        pty_instruction_bus.start();

        tab.handle_pty_bytes(
            crate::TerminalId::new(1),
            format!("\u{1b}[?1002h{encoding}").into_bytes(),
        )
        .unwrap();
        tab.handle_pty_bytes(crate::TerminalId::new(1), twenty_marked_commands())
            .unwrap();

        tab.handle_mouse_event_with_passthrough(
            &MouseEvent::new_alt_scroll_up_event(Position::new(5, 71)),
            client_id,
            None,
        )
        .unwrap();

        pty_instruction_bus.exit();

        assert_eq!(
            pty_instruction_bus.clone_output(),
            vec![expected.to_string()],
            "a pane that tracks the mouse receives alt wheel just like a plain wheel"
        );
        assert!(
            !tab.get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
                .map(|pane| pane.is_scrolled())
                .unwrap_or(false),
            "a forwarded alt wheel must not also scroll the host pane"
        );
    }
}

#[test]
fn alt_wheel_jumps_between_prompts_of_the_pane_below_the_cursor() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(crate::TerminalId::new(1), twenty_marked_commands())
        .unwrap();

    let pane_is_scrolled = |tab: &Tab| {
        tab.get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
            .map(|pane| pane.is_scrolled())
            .unwrap_or(false)
    };
    assert!(!pane_is_scrolled(&tab));

    tab.handle_mouse_event(
        &MouseEvent::new_alt_scroll_up_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();
    assert!(
        pane_is_scrolled(&tab),
        "alt wheel up must jump to the previous prompt"
    );

    tab.handle_mouse_event(
        &MouseEvent::new_alt_scroll_down_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();
    assert!(
        !pane_is_scrolled(&tab),
        "alt wheel down must jump back towards the bottom of the buffer"
    );
}

#[test]
fn alt_wheel_is_swallowed_when_advanced_mouse_actions_are_disabled() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.update_advanced_mouse_actions(false);
    tab.handle_pty_bytes(crate::TerminalId::new(1), twenty_marked_commands())
        .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_alt_scroll_up_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();

    assert!(
        !tab.get_pane_with_id(PaneId::Terminal(crate::TerminalId::new(1)))
            .map(|pane| pane.is_scrolled())
            .unwrap_or(false),
        "alt wheel must be swallowed when advanced mouse actions are disabled"
    );
}

#[test]
fn test_scroll_wheel_up_scrolls_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    // Fill the pane with many lines to enable scrolling
    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("Line {}\r\n", i));
    }
    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from(content.as_bytes()))
        .unwrap();

    // Render before scroll
    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Scroll up
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_scroll_up_event(Position::new(10, 60)),
            client_id,
        )
        .unwrap();

    // Render after scroll
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Snapshots should be different (viewport moved)
    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_scroll_wheel_down_scrolls_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let _output = Output::default();

    // Fill the pane with many lines to enable scrolling
    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("Line {}\r\n", i));
    }
    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from(content.as_bytes()))
        .unwrap();

    // First scroll up to have content above viewport
    tab.handle_mouse_event(
        &MouseEvent::new_scroll_up_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_scroll_up_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_scroll_up_event(Position::new(10, 60)),
        client_id,
    )
    .unwrap();

    // Render before scrolling down
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Scroll down
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_scroll_down_event(Position::new(10, 60)),
            client_id,
        )
        .unwrap();

    // Render after scroll
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Snapshots should be different (viewport moved)
    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_scroll_on_inactive_pane_scrolls_that_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    // Fill both panes with scrollable content
    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("Left Line {}\r\n", i));
    }
    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from(content.as_bytes()))
        .unwrap();

    let mut content = String::new();
    for i in 0..50 {
        content.push_str(&format!("Right Line {}\r\n", i));
    }
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from(content.as_bytes()))
        .unwrap();

    // Active pane is pane 2 (right), scroll on pane 1 (left)
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    // Render before scroll
    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Scroll up on the left (inactive) pane
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_scroll_up_event(Position::new(10, 30)),
            client_id,
        )
        .unwrap();

    // Render after scroll
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // Snapshots should be different (left pane scrolled)
    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_right_click_forwards_to_active_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let _output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Active pane".as_bytes()),
    )
    .unwrap();

    // Right click on the active pane
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_right_press_event(Position::new(10, 60)),
            client_id,
        )
        .unwrap();

    // Event should be forwarded (verified via MouseEffect or no error)
    // The effect may not indicate state change, but should succeed
    assert!(effect.group_toggle.is_none());
}

#[test]
fn test_middle_click_forwards_to_active_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let _output = Output::default();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Active pane".as_bytes()),
    )
    .unwrap();

    // Middle click on the active pane
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_middle_press_event(Position::new(10, 60)),
            client_id,
        )
        .unwrap();

    // Event should be forwarded (verified via MouseEffect or no error)
    assert!(effect.group_toggle.is_none());
}

#[test]
fn test_hover_over_inactive_pane_sets_hover_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Active pane is pane 2 (right)
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    // Hover over inactive pane (left) with buttonless motion
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(Position::new(10, 30)),
            client_id,
        )
        .unwrap();

    let hover_panes = tab.query_mouse_hover_pane_id();
    assert_snapshot!(format!("{:?}", hover_panes));
    assert!(effect.state_changed);
}

#[test]
fn test_hover_over_active_pane_unsets_hover() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Active pane is pane 2 (right)
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    // Hover over inactive pane (left) with buttonless motion
    let _effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(Position::new(10, 30)),
            client_id,
        )
        .unwrap();
    // Hover over active pane (right) with buttonless motion
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_buttonless_motion(Position::new(10, 90)),
            client_id,
        )
        .unwrap();

    let hover_panes = tab.query_mouse_hover_pane_id();
    assert_snapshot!(format!("{:?}", hover_panes));
    assert!(effect.state_changed);
}

#[test]
fn test_resize_tiled_then_move_floating_sequence() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let tiled_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(3));
    let _output = Output::default();

    // Create tiled split
    tab.vertical_split(tiled_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Tiled Left".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Tiled Right".as_bytes()),
    )
    .unwrap();

    // Resize tiled pane with Ctrl+drag
    let edge_position = Position::new(5, 60);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(edge_position),
        client_id,
    )
    .unwrap();
    let motion_position = Position::new(5, 70);
    let resize_effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(motion_position),
            client_id,
        )
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(resize_effect.state_changed);

    // Now toggle floating and create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(3), Vec::from("Floating".as_bytes()))
        .unwrap();

    // Move floating pane
    let frame_position = Position::new(5, 30);
    let move_effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(frame_position), client_id)
        .unwrap();
    let motion_position = Position::new(8, 40);
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(motion_position),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(motion_position),
        client_id,
    )
    .unwrap();

    // Both operations should have succeeded
    assert!(move_effect.state_changed);

    // Render final state
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot.to_string());
}

#[test]
fn test_alt_click_works_on_floating_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Toggle floating panes mode and create a floating pane
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    // Alt+click on floating pane to toggle group
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_left_press_with_alt_event(Position::new(10, 60)),
            client_id,
        )
        .unwrap();

    // Verify group_toggle effect returned
    assert!(effect.group_toggle.is_some());
    assert_eq!(
        effect.group_toggle,
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );
}

#[test]
fn test_left_click_on_tiled_frame_edge_starts_resize() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let _output = Output::default();

    // Create a vertical split
    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(crate::TerminalId::new(2), Vec::from("Right".as_bytes()))
        .unwrap();

    // Click on the edge between panes (without Ctrl)
    let edge_position = Position::new(5, 60);
    let effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(edge_position), client_id)
        .unwrap();

    // Edge detection should start resize even without Ctrl for tiled panes
    assert!(effect.state_changed);
}

#[test]
fn test_left_click_on_floating_frame_not_on_pin_moves_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Movable pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let frame_position = Position::new(5, 30);
    let press_effect = tab
        .handle_mouse_event(&MouseEvent::new_left_press_event(frame_position), client_id)
        .unwrap();

    let motion_position = Position::new(6, 31);
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(motion_position),
        client_id,
    )
    .unwrap();

    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(motion_position),
        client_id,
    )
    .unwrap();

    assert!(press_effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_up_on_active_tiled_pane_increases_pane_size() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let active_pane_position = Position::new(5, 70);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_up_event(active_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_down_on_active_tiled_pane_decreases_pane_size() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let active_pane_position = Position::new(5, 70);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_down_event(active_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_up_on_inactive_tiled_pane_increases_active_tiled_pane_size() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let inactive_pane_position = Position::new(5, 10);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_up_event(inactive_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_down_on_inactive_tiled_pane_decreases_active_tiled_pane_size() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();

    tab.handle_pty_bytes(crate::TerminalId::new(1), Vec::from("Left pane".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Right pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let inactive_pane_position = Position::new(5, 10);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_down_event(inactive_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_up_increases_floating_pane_size() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Tiled pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let floating_pane_position = Position::new(5, 50);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_up_event(floating_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_down_decreases_floating_pane_size() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Tiled pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let floating_pane_position = Position::new(5, 50);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_down_event(floating_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_up_increases_pinned_floating_pane_size_when_floating_panes_hidden() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(true),
        borderless: Some(false),
    };

    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Tiled pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let floating_pane_position = Position::new(10, 10);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_up_event(floating_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_down_decreases_pinned_floating_pane_size_when_floating_panes_hidden() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(5)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(true),
        borderless: Some(false),
    };

    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(Some(coordinates)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("Tiled pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("Floating pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot_before = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    let floating_pane_position = Position::new(6, 10);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_down_event(floating_pane_position),
            client_id,
        )
        .unwrap();

    assert!(effect.state_changed);

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot_after = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_ne!(snapshot_before, snapshot_after);
    eprintln!("{}", snapshot_before);
    eprintln!("{}", snapshot_after);
    assert_snapshot!(snapshot_after.to_string());
}

#[test]
fn test_ctrl_scroll_up_does_not_resize_pane_when_mouse_scroll_resize_disabled() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();
    tab.update_mouse_scroll_resize(false);

    let pane_geom_before = tab.get_active_pane(client_id).unwrap().position_and_size();

    let active_pane_position = Position::new(5, 70);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_up_event(active_pane_position),
            client_id,
        )
        .unwrap();

    let pane_geom_after = tab.get_active_pane(client_id).unwrap().position_and_size();

    assert!(!effect.state_changed);
    assert_eq!(pane_geom_before, pane_geom_after);
}

#[test]
fn test_ctrl_scroll_down_does_not_resize_pane_when_mouse_scroll_resize_disabled() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));

    tab.vertical_split(new_pane_id, None, None, client_id, None, None)
        .unwrap();
    tab.update_mouse_scroll_resize(false);

    let pane_geom_before = tab.get_active_pane(client_id).unwrap().position_and_size();

    let active_pane_position = Position::new(5, 70);
    let effect = tab
        .handle_mouse_event(
            &MouseEvent::new_ctrl_scroll_down_event(active_pane_position),
            client_id,
        )
        .unwrap();

    let pane_geom_after = tab.get_active_pane(client_id).unwrap().position_and_size();

    assert!(!effect.state_changed);
    assert_eq!(pane_geom_before, pane_geom_after);
}

#[test]
fn test_ctrl_scroll_up_merging_stacks_preserves_all_panes() {
    let size = Size {
        cols: 150,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let mut tab = create_new_tab_with_stacked_pane_list(
        size,
        ModeInfo::default(),
        true,
        Some(two_adjacent_stacks_layout()),
    );

    tab.render(&mut output, None).unwrap();

    let mut pane_ids_before = tab.get_all_pane_ids();
    pane_ids_before.sort();
    assert_eq!(
        pane_ids_before.len(),
        6,
        "all six panes exist before the merge"
    );
    assert_eq!(stack_member_pane_count(&tab), 6);

    let active_pane_position = Position::new(5, 40);
    tab.handle_mouse_event(
        &MouseEvent::new_ctrl_scroll_up_event(active_pane_position),
        client_id,
    )
    .unwrap();

    let dissolved_into_grid = !tab.has_stack_lists();
    assert!(
        dissolved_into_grid,
        "stack lists must be dissolved into in-grid stacks before the classic merge mutation"
    );
    let mut in_grid_ids = tab.get_static_and_floating_pane_ids();
    in_grid_ids.sort();
    assert_eq!(
        in_grid_ids, pane_ids_before,
        "all panes must be present in the grid after the merge, none orphaned"
    );

    let mut pane_ids_after = tab.get_all_pane_ids();
    pane_ids_after.sort();
    assert_eq!(
        pane_ids_before, pane_ids_after,
        "no panes are lost when merging stacks via ctrl+scroll resize"
    );

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let mut pane_ids_final = tab.get_all_pane_ids();
    pane_ids_final.sort();
    assert_eq!(pane_ids_before, pane_ids_final);
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

// Regression: when stacked_pane_list mode is active, adding a stacked pane
// used to send the pre-existing pane an intermediate `resize_pty!` at 1 row
// (from the classic in-grid collapse) before groupify_all moved it into
// suppressed_panes and resized it back to the visible size. Shells (nushell,
// pwsh, bash) redraw their prompt on each WINCH, which blanks out
// previously-visible rows of output — visible to the user as "truncated
// scrollback" when they focus back to that pane.
//
// The fix syncs stacked_pane_list mode immediately after the classic in-grid
// mutation (before focus/frame reapplication), so the collapsed pane is
// promoted to a stack-list suppressed pane before any resize_pty at the
// 1-row size can leak out to its shell. This test pins that end-state:
// after adding a stacked pane, the pre-existing pane must have been moved
// into suppressed_panes (indicating groupify_all ran), rather than left in
// tiled_panes at rows=1 waiting for the next render.
#[test]
fn adding_stacked_pane_in_stack_list_mode_moves_existing_pane_to_suppressed_immediately() {
    let size = Size {
        cols: 121,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_with_stacked_pane_list(size, ModeInfo::default(), true, None);

    // Add a second tiled pane so we have a real stack candidate.
    let existing_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.new_pane(
        existing_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    // Sanity: pane 2 is a tiled pane and no stack lists exist yet.
    assert!(tab.tiled_panes.get_pane(existing_pane_id).is_some());
    assert!(!tab.has_stack_lists());
    assert!(!tab.suppressed_panes.contains_key(&existing_pane_id));

    // Now add a stacked pane on top of the active pane (pane 2).
    let stacked_pane_id = PaneId::Terminal(crate::TerminalId::new(3));
    tab.new_pane(
        stacked_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Stacked {
            pane_id_to_stack_under: None,
            borderless: None,
        },
        Some(client_id),
        None,
    )
    .unwrap();

    // After the operation, stacked_pane_list mode should have picked up the
    // new stack: pane 2 (the pre-existing member) must be in suppressed_panes,
    // and the visible member (pane 3) must be in tiled_panes with a non-fixed
    // row dimension (i.e., not the collapsed 1-row tab-strip form).
    assert!(
        tab.has_stack_lists(),
        "stacked_pane_list mode should have groupified the new stack immediately"
    );
    assert!(
        tab.suppressed_panes.contains_key(&existing_pane_id),
        "pane 2 (pre-existing pane) should be in suppressed_panes after the \
         stacked pane is added; if it is still in tiled_panes at rows=1 the \
         next `reapply_pane_frames` will send a resize_pty to its shell at \
         the collapsed size, blanking its viewport"
    );
    assert!(
        tab.tiled_panes.get_pane(stacked_pane_id).is_some(),
        "the newly-added stacked pane should be the visible tiled pane"
    );
    let visible_geom = tab
        .tiled_panes
        .get_pane(stacked_pane_id)
        .unwrap()
        .position_and_size();
    assert!(
        !visible_geom.rows.is_fixed(),
        "the visible member of the stack must not be at fixed(1) rows"
    );
}

#[test]
fn test_ctrl_scroll_down_in_stack_dissolves_stack_lists_before_mutation() {
    let size = Size {
        cols: 150,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let mut tab = create_new_tab_with_stacked_pane_list(
        size,
        ModeInfo::default(),
        true,
        Some(two_adjacent_stacks_layout()),
    );

    tab.render(&mut output, None).unwrap();

    let mut pane_ids_before = tab.get_all_pane_ids();
    pane_ids_before.sort();
    assert_eq!(pane_ids_before.len(), 6);
    assert_eq!(stack_member_pane_count(&tab), 6);

    tab.handle_mouse_event(
        &MouseEvent::new_ctrl_scroll_down_event(Position::new(5, 40)),
        client_id,
    )
    .unwrap();

    assert!(
        !tab.has_stack_lists(),
        "stack lists must be dissolved into in-grid stacks before the classic merge mutation"
    );
    let mut in_grid_ids = tab.get_static_and_floating_pane_ids();
    in_grid_ids.sort();
    assert_eq!(
        in_grid_ids, pane_ids_before,
        "all panes must be present in the grid after the resize, none orphaned"
    );

    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let mut pane_ids_final = tab.get_all_pane_ids();
    pane_ids_final.sort();
    assert_eq!(pane_ids_before, pane_ids_final);
}

#[test]
fn resize_hint_text_tracks_mouse_scroll_resize_updates() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());

    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.mouse_help_text_visible.insert(client_id, true);

    tab.update_mouse_scroll_resize(false);
    let disabled_hints = tab.resolve_hint_text(client_id);
    assert!(!disabled_hints.is_empty());
    assert!(disabled_hints
        .values()
        .all(|hint| !hint.text.contains("MouseScroll")));

    tab.update_mouse_scroll_resize(true);
    let enabled_hints = tab.resolve_hint_text(client_id);
    assert!(!enabled_hints.is_empty());
    assert!(enabled_hints
        .values()
        .all(|hint| hint.text.contains("MouseScroll")));
}

/// A modal is not resizable (`OverlayPanes::is_resizable`), so nothing may offer to resize it -
/// this surface counted only tiled and floating panes and had never been asked about overlays, so
/// a focused modal was told about a gesture both ctrl+scroll and ctrl+drag refuse.
#[test]
fn a_focused_modal_is_not_told_how_to_resize_itself() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.mouse_help_text_visible.insert(client_id, true);
    assert!(
        !tab.resolve_hint_text(client_id).is_empty(),
        "pre-condition: an ordinary focused pane does get the resize hint"
    );

    let modal = PaneId::Terminal(crate::TerminalId::new(3));
    tab.new_pane(
        modal,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::modal(Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(10)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(8)),
            ..Default::default()
        }))),
        Some(client_id),
        None,
    )
    .unwrap();
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(modal),
        "pre-condition: the modal has the keyboard"
    );

    assert!(
        tab.resolve_hint_text(client_id).is_empty(),
        "a modal is sized by its plugin, so it says nothing about resizing"
    );
}

/// A suspended pane's hint has to win over the resize help: the pane is stuck,
/// and telling the user how to resize it instead of how to wake it is the one
/// thing the hint must never do.
#[test]
fn a_suspended_pane_says_how_to_resume_itself() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.mouse_help_text_visible.insert(client_id, true);

    let focused = tab.get_active_pane_id(client_id).expect("no focused pane");
    tab.set_pane_suspended(focused, true).unwrap();

    let hints = tab.resolve_hint_text(client_id);
    assert!(!hints.is_empty(), "a suspended pane must say something");
    assert!(
        hints
            .values()
            .all(|hint| hint.text.contains("SUSPENDED") && hint.text.contains("resume")),
        "every width of the hint must name the state and the way out: {hints:?}"
    );

    // ...and it goes away again once the process is running, rather than
    // sticking around over a pane that is perfectly usable.
    tab.set_pane_suspended(focused, false).unwrap();
    let hints = tab.resolve_hint_text(client_id);
    assert!(hints.values().all(|hint| !hint.text.contains("SUSPENDED")));
}

#[test]
fn in_place_pane_with_close_replaced_pane_false_restores_original() {
    // When an in-place pane is closed and close_replaced_pane=false (the default),
    // the pane that was replaced is restored to its original position.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    // Create a second tiled pane so closing the in-place pane doesn't leave an empty tab
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        false,
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        Some(client_id),
        None,
    )
    .unwrap();

    // Write distinguishing content to each pane
    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\n\n\nI am pane one".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("\n\n\nI am pane two".as_bytes()),
    )
    .unwrap();

    // Open pane 3 in-place of pane 2 without closing the replaced pane (suppress it)
    tab.new_in_place_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        false, // close_replaced_pane
        Some(client_id),
        None,
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(3),
        Vec::from("\n\n\nI am the in-place pane".as_bytes()),
    )
    .unwrap();

    // Close the in-place pane — pane 2 should be restored
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    // Snapshot shows both pane 1 and the restored pane 2
    assert_snapshot!(snapshot);
}

#[test]
fn in_place_pane_with_close_replaced_pane_true_closes_original() {
    // When an in-place pane is closed and close_replaced_pane=true,
    // the replaced pane is permanently destroyed rather than restored.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    // Create a second tiled pane
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        false,
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        Some(client_id),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId::new(1),
        Vec::from("\n\n\nI am pane one".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("\n\n\nI am pane two".as_bytes()),
    )
    .unwrap();

    // Open pane 3 in-place of pane 2, closing the replaced pane permanently
    tab.new_in_place_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        true, // close_replaced_pane
        Some(client_id),
        None,
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(3),
        Vec::from("\n\n\nI am the in-place pane".as_bytes()),
    )
    .unwrap();

    // Close the in-place pane — pane 2 should NOT be restored (it was closed)
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    // Snapshot shows only pane 1 (pane 2 was permanently closed, not restored)
    assert_snapshot!(snapshot);
}

#[test]
fn reserving_a_suppressed_pane_does_not_evict_the_tabs_working_pane() {
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        NewPanePlacement::Floating(None),
        NewPanePlacement::Stacked {
            pane_id_to_stack_under: None,
            borderless: None,
        },
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
    ] {
        let mut tab = create_new_tab(Size { rows: 20, cols: 80 }, ModeInfo::default());
        tab.max_panes = Some(1);
        let working = tab.get_all_pane_ids();
        let pid = PaneId::Terminal(TerminalId::new(99));
        tab.new_pane(pid, None, None, true, false, placement, None, None)
            .unwrap();
        assert!(working.iter().all(|id| tab.has_pane_with_pid(id)));
        assert!(tab.get_pane_info(pid).unwrap().is_suppressed);
    }
}

#[test]
fn clipboard_without_terminal_bytes_still_reports_completion() {
    use crate::tab::clipboard::ClipboardProvider;
    use crate::tab::copy_command::CopyCommand;
    use yinz_types::data::{CopyDestination, Event};
    for command in [false, true] {
        let (mut tab, plugin_rx) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        let (sender, server_rx) = channels::unbounded();
        tab.senders.to_server = Some(SenderWithContext::new(sender));
        let destination = if command {
            tab.clipboard_provider =
                ClipboardProvider::Command(CopyCommand::new("/usr/bin/true".into()));
            CopyDestination::Command
        } else {
            tab.remove_client(ClientId::new(1));
            tab.clipboard_provider.as_copy_destination()
        };
        // Empty input avoids depending on when the command closes stdin.
        tab.write_selection_to_clipboard("").unwrap();
        assert!(server_rx.try_recv().is_err());
        let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
        loop {
            let (instruction, _) = plugin_rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("clipboard completion");
            if matches!(instruction, PluginInstruction::Update(events) if events.iter().any(|(_, _, event)|
                matches!(event, Event::CopyToClipboard(actual) if *actual == destination)))
            {
                break;
            }
        }
    }
}

#[test]
fn closing_a_pane_during_mouse_resize_cancels_the_drag_on_every_surface() {
    use crate::tab::ResizeDragTarget;
    let client = ClientId::new(1);
    let pane_id = PaneId::Terminal(TerminalId::new(2));
    for target in [
        ResizeDragTarget::Tiled,
        ResizeDragTarget::Floating,
        ResizeDragTarget::Overlay,
    ] {
        let mut tab = create_new_tab(
            Size {
                cols: 120,
                rows: 40,
            },
            ModeInfo::default(),
        );
        match target {
            ResizeDragTarget::Tiled => tab
                .vertical_split(pane_id, None, None, client, None, None)
                .unwrap(),
            ResizeDragTarget::Floating => {
                tab.new_floating_pane(pane_id, None, None, false, true, None, None)
                    .unwrap();
                tab.floating_panes.toggle_show_panes(true);
            },
            ResizeDragTarget::Overlay => tab
                .new_pane(
                    pane_id,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                    Some(client),
                    None,
                )
                .unwrap(),
        }
        let geom = tab.get_pane_with_id(pane_id).unwrap().current_geom();
        let press = Position::new(geom.y as i32 + 2, geom.x as u16);
        tab.handle_mouse_event(&MouseEvent::new_left_press_with_ctrl_event(press), client)
            .unwrap();
        assert_eq!(
            tab.pane_being_resized_with_mouse
                .as_ref()
                .map(|state| state.target),
            Some(target)
        );
        assert!(tab.extract_pane(pane_id, true).is_some());
        let motion = Position::new(geom.y as i32 + 2, geom.x as u16 + 1);
        tab.handle_mouse_event(&MouseEvent::new_left_motion_with_ctrl_event(motion), client)
            .unwrap();
        assert!(tab.pane_being_resized_with_mouse.is_none(), "{target:?}");
        tab.handle_mouse_event(
            &MouseEvent::new_left_release_with_ctrl_event(motion),
            client,
        )
        .unwrap();
        assert!(tab.pane_being_resized_with_mouse.is_none());
    }
}

#[test]
fn clipboard_command_failures_report_failure_without_success() {
    use crate::tab::clipboard::ClipboardProvider;
    use crate::tab::copy_command::CopyCommand;
    use yinz_test_support::{default_timeout, write_warm_script};
    use yinz_types::data::Event;

    let dir = tempfile::tempdir().unwrap();
    let closes_stdin = dir.path().join("closes-stdin");
    write_warm_script(&closes_stdin, "exec 0<&-\nexit 0");
    for command in [dir.path().join("missing"), closes_stdin] {
        let (mut tab, plugin_rx) =
            create_new_tab_with_plugin_receiver(Size { cols: 80, rows: 24 }, ModeInfo::default());
        tab.clipboard_provider =
            ClipboardProvider::Command(CopyCommand::new(command.display().to_string()));
        tab.write_selection_to_clipboard(&"x".repeat(4 * 1024 * 1024))
            .unwrap();
        let deadline = std::time::Instant::now() + default_timeout();
        loop {
            let (instruction, _) = plugin_rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("clipboard failure notification");
            if let PluginInstruction::Update(events) = instruction {
                assert!(!events
                    .iter()
                    .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))));
                if events
                    .iter()
                    .any(|(_, _, event)| matches!(event, Event::SystemClipboardFailure))
                {
                    break;
                }
            }
        }
    }
}
