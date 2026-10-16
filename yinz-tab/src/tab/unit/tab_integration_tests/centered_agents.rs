use super::*;
use yinz_terminal::panes::terminal_character::AnsiCode;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::colors::PaletteColor;

fn host_grid(size: Size) -> Grid {
    Grid::new(
        size.rows,
        size.cols,
        Rc::new(RefCell::new(Palette::default())),
        Default::default(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Default::default(),
        Default::default(),
        Default::default(),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    )
}

fn render_into(tab: &mut Tab, grid: &mut Grid) {
    vte::Parser::new().advance(grid, draw(tab).as_bytes());
}

fn assert_surround(tab: &Tab, id: PaneId, grid: &Grid) {
    let pane = tab.get_pane_with_id(id).unwrap();
    let outer = pane.current_geom();
    let inner = pane.render_geom();
    assert!(inner.x > outer.x);
    for y in outer.y..outer.y + outer.rows.as_usize() {
        for x in (outer.x..inner.x)
            .chain(inner.x + inner.cols.as_usize()..outer.x + outer.cols.as_usize())
        {
            let cell = grid.get_character_at(x, y).unwrap();
            assert_eq!(cell.character, ' ', "margin at {x},{y}");
            assert_eq!(
                cell.styles.background,
                Some(tab.style.pane_surround().into()),
                "margin at {x},{y}"
            );
        }
    }
}

fn mark_agent(tab: &mut Tab, id: PaneId) {
    tab.set_pane_agent(
        id,
        Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
    );
}

fn draw(tab: &mut Tab) -> String {
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

fn moving_pane_repaints_agent_margins(overlay: bool, vertical: bool) {
    let size = Size {
        cols: 400,
        rows: 30,
    };
    let client = ClientId::new(1);
    let moving = PaneId::Terminal(TerminalId::new(2));
    for x in [40, 320] {
        let mut tab = create_new_tab(size, ModeInfo::default());
        tab.max_agent_width = 80;
        mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(1)));
        let coordinates = Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(x)),
            y: Some(PercentOrFixed::Fixed(8)),
            width: Some(PercentOrFixed::Fixed(30)),
            height: Some(PercentOrFixed::Fixed(8)),
            ..Default::default()
        });
        let placement = if overlay {
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(coordinates))
        } else {
            NewPanePlacement::Floating(coordinates)
        };
        tab.new_pane(
            moving,
            None,
            None,
            false,
            true,
            placement,
            Some(client),
            None,
        )
        .unwrap();
        tab.handle_pty_bytes(TerminalId::new(2), b"MOVINGXXXXXXXXXXXXXXXXXXXXXX".to_vec())
            .unwrap();
        let mut output = draw(&mut tab);
        assert!(
            take_snapshot(&output, size.rows, size.cols, Palette::default()).contains("MOVING")
        );
        for forward in [true, false] {
            let before = tab.get_pane_with_id(moving).unwrap().render_geom();
            match (vertical, forward) {
                (true, true) => tab.move_pane_down(moving),
                (true, false) => tab.move_pane_up(moving),
                (false, true) => tab.move_pane_right(moving),
                (false, false) => tab.move_pane_left(moving),
            }
            output.push_str(&draw(&mut tab));
            let after = tab.get_pane_with_id(moving).unwrap().render_geom();
            assert_ne!((before.x, before.y), (after.x, after.y));
            // Replay every frame into one terminal so stale text and borders stay observable.
            let snapshot = take_snapshot(&output, size.rows, size.cols, Palette::default());
            assert!(snapshot.contains("MOVING"));
            for (y, line) in snapshot.lines().enumerate() {
                let cells = line.split_once(": ").unwrap().1;
                for (x, cell) in cells.chars().enumerate() {
                    let position = Position::new(y as i32, x as u16);
                    if before.contains(&position) && !after.contains(&position) {
                        assert_eq!(cell, ' ', "vacated cell ({x}, {y}), overlay={overlay}, vertical={vertical}, forward={forward}");
                    }
                    if (x, y) == (after.x, after.y + 1) {
                        assert_eq!(cell, '│', "moved pane border");
                    }
                }
            }
            assert!(tab.pending_render_damage.is_empty());
        }
    }
}

#[test]
fn centered_agent_floating_horizontal_movement_repaints_margins() {
    moving_pane_repaints_agent_margins(false, false);
}

#[test]
fn centered_agent_floating_vertical_movement_repaints_margins() {
    moving_pane_repaints_agent_margins(false, true);
}

#[test]
fn centered_agent_overlay_vertical_movement_repaints_margins() {
    moving_pane_repaints_agent_margins(true, true);
}

fn stack_entry_uses_rendered_spans(down: bool) {
    let row = r#"pane split_direction="vertical" { pane; pane; pane; }"#;
    let stacks = r#"pane split_direction="vertical" {
        pane stacked=true { pane; pane; }
        pane stacked=true { pane; pane; }
    }"#;
    let layout = if down {
        format!("layout {{\n{row}\n{stacks}\n}}")
    } else {
        format!("layout {{\n{stacks}\n{row}\n}}")
    };
    let client = ClientId::new(1);
    for limit in [0, 80] {
        for move_pane in [false, true] {
            let mut tab = create_new_tab_with_layout(
                Size {
                    cols: 400,
                    rows: 30,
                },
                ModeInfo::default(),
                &layout,
            );
            let mut row_ids = Vec::new();
            let mut left_stack = Vec::new();
            let mut right_stack = Vec::new();
            for (id, pane) in tab.tiled_panes.get_panes() {
                let geom = pane.current_geom();
                if !geom.is_stacked() {
                    row_ids.push(*id);
                } else if geom.x == 0 {
                    left_stack.push(*id);
                } else {
                    right_stack.push(*id);
                }
            }
            row_ids.sort_by_key(|id| tab.get_pane_with_id(*id).unwrap().current_geom().x);
            for stack in [&mut left_stack, &mut right_stack] {
                stack.sort_by_key(|id| tab.get_pane_with_id(*id).unwrap().current_geom().y);
                if !down {
                    stack.reverse();
                }
            }
            tab.max_agent_width = limit;
            for id in row_ids.iter().chain(&right_stack) {
                mark_agent(&mut tab, *id);
            }
            let source = row_ids[1];
            tab.focus_pane_with_id(right_stack[0], false, false, client)
                .unwrap();
            tab.focus_pane_with_id(source, false, false, client)
                .unwrap();
            let recent = std::time::Instant::now() + std::time::Duration::from_secs(1);
            for id in &right_stack {
                tab.get_pane_with_id_mut(*id).unwrap().set_active_at(recent);
            }
            draw(&mut tab);
            if limit != 0 {
                let source_geom = tab.get_pane_with_id(source).unwrap().render_geom();
                let right_geom = tab.get_pane_with_id(right_stack[0]).unwrap().render_geom();
                assert!(source_geom.x + source_geom.cols.as_usize() <= right_geom.x);
            }
            if move_pane {
                let destination = tab.get_pane_with_id(right_stack[0]).unwrap().current_geom();
                if down {
                    tab.move_pane_down(source);
                } else {
                    tab.move_pane_up(source);
                }
                let moved = tab.get_pane_with_id(source).unwrap().current_geom();
                assert_eq!(moved.x, destination.x, "pane movement uses requested spans");
                assert_eq!(moved.cols, destination.cols);
            } else {
                let expected = if limit == 0 {
                    &right_stack
                } else {
                    &left_stack
                };
                for id in expected {
                    assert!(if down {
                        tab.move_focus_down(client)
                    } else {
                        tab.move_focus_up(client)
                    }
                    .unwrap());
                    assert_eq!(
                        tab.get_active_pane_id(client),
                        Some(*id),
                        "down={down}, limit={limit}"
                    );
                    draw(&mut tab);
                }
                assert!(if down {
                    tab.move_focus_up(client)
                } else {
                    tab.move_focus_down(client)
                }
                .unwrap());
                assert_eq!(tab.get_active_pane_id(client), Some(expected[0]));
            }
        }
    }
}

#[test]
fn centered_agent_stack_entry_down_uses_rendered_spans() {
    stack_entry_uses_rendered_spans(true);
}

#[test]
fn centered_agent_stack_entry_up_uses_rendered_spans() {
    stack_entry_uses_rendered_spans(false);
}

#[test]
fn centered_agent_floating_swaps_preserve_position_anchors() {
    let client = ClientId::new(1);
    let ids = [2, 3].map(|id| PaneId::Terminal(TerminalId::new(id)));
    for limit in [0, 80] {
        for swap in 0..3 {
            for anchor_second in [false, true] {
                let mut tab = create_new_tab(
                    Size {
                        cols: 400,
                        rows: 30,
                    },
                    ModeInfo::default(),
                );
                tab.max_agent_width = limit;
                for (id, x) in [(ids[0], if limit == 0 { 20 } else { 1 }), (ids[1], 200)] {
                    tab.new_pane(
                        id,
                        None,
                        None,
                        false,
                        true,
                        NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                            x: Some(PercentOrFixed::Fixed(x)),
                            y: Some(PercentOrFixed::Fixed(3)),
                            width: Some(PercentOrFixed::Fixed(120)),
                            height: Some(PercentOrFixed::Fixed(20)),
                            ..Default::default()
                        })),
                        Some(client),
                        None,
                    )
                    .unwrap();
                    mark_agent(&mut tab, id);
                }
                draw(&mut tab);
                tab.move_pane_right(ids[0]);
                if anchor_second {
                    tab.move_pane_up(ids[1]);
                }
                draw(&mut tab);
                let before = ids.map(|id| tab.get_pane_with_id(id).unwrap().render_geom());
                assert_eq!(before[0].x, 30);
                match swap {
                    0 => tab.floating_panes.move_pane(false, ids[0]),
                    1 => tab.floating_panes.move_pane(true, ids[0]),
                    _ => {
                        tab.floating_panes.focus_pane(ids[0], client);
                        tab.floating_panes
                            .switch_active_pane_with(&mut tab.os_api, ids[1]);
                    },
                }
                for cols in [None, None, Some(250), Some(400)] {
                    if let Some(cols) = cols {
                        tab.resize_whole_tab(Size { cols, rows: 30 }).unwrap();
                    }
                    draw(&mut tab);
                    if cols != Some(250) {
                        let after = ids.map(|id| tab.get_pane_with_id(id).unwrap().render_geom());
                        assert_eq!(
                            after,
                            [before[1], before[0]],
                            "limit={limit}, swap={swap}, anchor_second={anchor_second}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn centered_agent_floating_focus_uses_displayed_order_and_spans() {
    let client = ClientId::new(1);
    let ids = [2, 3, 4].map(|id| PaneId::Terminal(TerminalId::new(id)));
    for vertical in [false, true] {
        let mut tab = create_new_tab(
            Size {
                cols: 400,
                rows: 40,
            },
            ModeInfo::default(),
        );
        tab.max_agent_width = 80;
        for (id, x, y, width) in [
            (ids[0], 0, 2, 300),
            (ids[1], 20, if vertical { 12 } else { 2 }, 82),
            (ids[2], 110, 22, 82),
        ] {
            tab.new_pane(
                id,
                None,
                None,
                false,
                true,
                NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(x)),
                    y: Some(PercentOrFixed::Fixed(y)),
                    width: Some(PercentOrFixed::Fixed(width)),
                    height: Some(PercentOrFixed::Fixed(8)),
                    ..Default::default()
                })),
                Some(client),
                None,
            )
            .unwrap();
            mark_agent(&mut tab, id);
        }
        draw(&mut tab);
        assert_eq!(tab.get_pane_with_id(ids[0]).unwrap().render_geom().x, 109);
        assert_eq!(tab.get_pane_with_id(ids[1]).unwrap().render_geom().x, 20);
        if vertical {
            tab.floating_panes.focus_pane(ids[0], client);
            assert!(tab.move_focus_down(client).unwrap());
            assert_eq!(tab.floating_panes.active_pane_id(client), Some(ids[2]));
            assert!(tab.move_focus_up(client).unwrap());
            assert_eq!(tab.floating_panes.active_pane_id(client), Some(ids[0]));
        } else {
            tab.floating_panes.focus_pane(ids[1], client);
            assert!(tab.move_focus_right(client).unwrap());
            assert_eq!(tab.floating_panes.active_pane_id(client), Some(ids[0]));
            assert!(tab.move_focus_left(client).unwrap());
            assert_eq!(tab.floating_panes.active_pane_id(client), Some(ids[1]));
            tab.floating_panes
                .focus_pane_on_edge(Direction::Right, client);
            assert_eq!(tab.floating_panes.active_pane_id(client), Some(ids[1]));
        }
    }
}

#[test]
fn centered_agent_seam_drag_against_cap_preserves_requested_split() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    for (agent_index, ratio, direction) in [(0, 7500, 1), (1, 2500, -1)] {
        let client = ClientId::new(1);
        let ids = [2, 3].map(|id| PaneId::Terminal(TerminalId::new(id)));
        let mut tab = create_new_tab(
            Size {
                cols: 300,
                rows: 20,
            },
            ModeInfo::default(),
        );
        for id in ids {
            tab.new_pane(
                id,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(client),
                None,
            )
            .unwrap();
        }
        tab.create_multipane(
            ids,
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio,
            },
            0,
            OverlayPlacement::dismissible(None),
        )
        .unwrap();
        tab.toggle_pane_fullscreen(ids[0]);
        draw(&mut tab);
        let requested = ids.map(|id| tab.get_pane_with_id(id).unwrap().render_geom());
        tab.max_agent_width = 80;
        mark_agent(&mut tab, ids[agent_index]);
        draw(&mut tab);
        let first = tab.get_pane_with_id(ids[0]).unwrap().render_geom();
        let seam = Position::new(first.y as i32, (first.x + first.cols.as_usize()) as u16);
        assert_eq!(tab.overlay_panes.seam_at(&seam), Some((ids[0], ids[1])));
        tab.handle_mouse_event(&MouseEvent::new_left_press_event(seam), client)
            .unwrap();
        assert!(tab.overlay_panes.seam_is_being_dragged());
        for distance in [0, 1, 10, 100] {
            let beyond_cap = Position::new(
                first.y as i32,
                (seam.column() as i32 + direction * distance) as u16,
            );
            assert!(!tab.overlay_panes.drag_seam_to(&beyond_cap));
            assert_eq!(
                tab.overlay_panes
                    .multipane_info(&ids[0])
                    .unwrap()
                    .split
                    .ratio,
                ratio
            );
        }
        let beyond_cap = Position::new(first.y as i32, (seam.column() as i32 + direction) as u16);
        tab.handle_mouse_event(&MouseEvent::new_left_release_event(beyond_cap), client)
            .unwrap();
        assert!(!tab.overlay_panes.seam_is_being_dragged());
        assert_eq!(
            tab.overlay_panes
                .multipane_info(&ids[0])
                .unwrap()
                .split
                .ratio,
            ratio
        );
        tab.max_agent_width = 0;
        draw(&mut tab);
        assert_eq!(
            ids.map(|id| tab.get_pane_with_id(id).unwrap().render_geom()),
            requested
        );
        tab.max_agent_width = 80;
        draw(&mut tab);
        tab.overlay_panes.start_seam_drag(ids[0], ids[1], seam);
        let away_from_cap =
            Position::new(first.y as i32, (seam.column() as i32 - direction) as u16);
        assert!(tab.overlay_panes.drag_seam_to(&away_from_cap));
        tab.overlay_panes.stop_seam_drag(away_from_cap);
        assert_ne!(
            tab.overlay_panes
                .multipane_info(&ids[0])
                .unwrap()
                .split
                .ratio,
            ratio
        );
        draw(&mut tab);
        let moved = tab.get_pane_with_id(ids[0]).unwrap().render_geom();
        assert_eq!(moved.x + moved.cols.as_usize(), away_from_cap.column());
    }
}

#[test]
fn centered_agent_resizes_pty_and_aligns_frame_cursor_and_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let (sender, receiver) = channels::unbounded();
    let mut tab = create_new_tab_with_mock_pty_writer(
        size,
        ModeInfo::default(),
        SenderWithContext::new(sender),
    );
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, id);
    tab.handle_pty_bytes(TerminalId::new(1), b"Centered agent output".to_vec())
        .unwrap();
    let output = draw(&mut tab);
    let pane = tab.get_pane_with_id(id).unwrap();
    assert_eq!(pane.current_geom().cols.as_usize(), 121);
    assert_eq!(pane.render_geom().x, 19);
    assert_eq!(pane.render_geom().cols.as_usize(), 82);
    assert_eq!(pane.get_content_columns(), 80);
    assert_eq!(pane.get_content_x(), 20);
    assert_eq!(pane.get_content_rows(), 18);
    assert!(!pane.contains(&Position::new(2, 0)));
    assert!(pane.position_is_on_frame(&Position::new(2, 19)));
    assert!(!pane.position_is_on_frame(&Position::new(2, 20)));
    assert_eq!(
        pane.relative_position(&Position::new(2, 20)),
        Position::new(1, 0)
    );
    assert_eq!(
        tab.get_active_terminal_cursor_position(ClientId::new(1)),
        Some((41, 1, true))
    );
    assert!(receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction,
        PtyWriteInstruction::ResizePty(pid, 80, 18, _, _) if pid == TerminalId::new(1))));
    assert_snapshot!(take_snapshot(
        &output,
        size.rows,
        size.cols,
        Palette::default()
    ));
    draw(&mut tab);
    assert!(receiver
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::ResizePty(..))));
}

#[test]
fn centered_agent_limit_tracks_identity_config_and_screen_size() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 119);
    mark_agent(&mut tab, id);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
    tab.set_pane_agent(id, None);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
    for (columns, expected_width, expected_x) in [(122, 80, 20), (60, 58, 0), (120, 80, 19)] {
        tab.resize_whole_tab(Size {
            cols: columns,
            rows: 20,
        })
        .unwrap();
        draw(&mut tab);
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(pane.get_content_columns(), expected_width);
        assert_eq!(pane.render_geom().x, expected_x);
    }
    for limit in [0, 1, 79, 80, 81, 79, 80, 0, 100, usize::MAX, 0] {
        tab.max_agent_width = limit;
        draw(&mut tab);
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(
            pane.get_content_columns(),
            if limit < 80 { 118 } else { limit.min(118) }
        );
    }
}

#[test]
fn centered_agent_split_fullscreen_and_close_preserve_layout() {
    let size = Size {
        cols: 241,
        rows: 20,
    };
    let client = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let agent = PaneId::Terminal(TerminalId::new(1));
    let other = PaneId::Terminal(TerminalId::new(2));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    tab.new_pane(
        other,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client),
        None,
    )
    .unwrap();
    draw(&mut tab);
    let split = tab.get_pane_with_id(agent).unwrap().current_geom();
    let capped_split = tab.get_pane_with_id(agent).unwrap().render_geom();
    assert_eq!(capped_split.cols.as_usize(), 82);
    assert_eq!(capped_split.x, 0);
    assert_eq!(tab.get_pane_with_id(other).unwrap().render_geom().x, 82);
    assert!(split.cols.as_usize() > 50);
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    tab.unset_fullscreen();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().render_geom(),
        capped_split
    );
    tab.close_pane(other, false, None);
    draw(&mut tab);
    let pane = tab.get_pane_with_id(agent).unwrap();
    assert_eq!(pane.current_geom().cols.as_usize(), size.cols);
    assert_eq!(pane.get_content_columns(), 80);
}

#[test]
fn pane_surround_fullscreen_in_each_layer_and_frame_style() {
    let size = Size {
        cols: 241,
        rows: 20,
    };
    let client = ClientId::new(1);
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        for placement in [
            NewPanePlacement::default(),
            NewPanePlacement::Floating(None),
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        ] {
            let mut tab = create_new_tab(size, ModeInfo::default());
            tab.set_pane_frames(style);
            let id = PaneId::Terminal(TerminalId::new(2));
            tab.new_pane(id, None, None, false, true, placement, Some(client), None)
                .unwrap();
            tab.max_agent_width = 80;
            mark_agent(&mut tab, id);
            draw(&mut tab);
            let original = tab.get_pane_with_id(id).unwrap().current_geom();
            let capped = tab.get_pane_with_id(id).unwrap().render_geom();
            assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
            tab.toggle_pane_fullscreen(id);
            let mut grid = host_grid(size);
            render_into(&mut tab, &mut grid);
            assert_surround(&tab, id, &grid);
            let pane = tab.get_pane_with_id(id).unwrap();
            assert_eq!(pane.get_content_columns(), 80);
            let frame = pane.render_geom();
            assert_eq!(frame.x, (size.cols - frame.cols.as_usize()) / 2);
            assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
                &mut tab,
                &Position::new(3, 0),
                false
            )
            .unwrap()
            .is_none());
            tab.toggle_pane_no_ui_fullscreen(id);
            render_into(&mut tab, &mut grid);
            assert_surround(&tab, id, &grid);
            let mut theme = tab.style.colors;
            theme.pane_surround = Some(PaletteColor::Rgb((1, 2, 3)));
            tab.update_theme(theme);
            render_into(&mut tab, &mut grid);
            assert_surround(&tab, id, &grid);
            tab.handle_pty_bytes(
                TerminalId::new(2),
                b"\x1b]11;#ff0000\x07\x1b[44m\x1b[2J".to_vec(),
            )
            .unwrap();
            render_into(&mut tab, &mut grid);
            assert_surround(&tab, id, &grid);
            let pane = tab.get_pane_with_id(id).unwrap();
            assert_eq!(pane.get_content_columns(), 80);
            assert_eq!(pane.get_content_rows(), size.rows);
            assert_eq!(pane.render_geom().cols.as_usize(), 80);
            tab.unset_fullscreen();
            render_into(&mut tab, &mut grid);
            for y in 0..size.rows {
                assert_ne!(
                    grid.get_character_at(0, y).unwrap().styles.background,
                    Some(AnsiCode::RgbCode((1, 2, 3)))
                );
            }
            assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), original);
            assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom(), capped);
        }
    }
}

#[test]
fn centered_agent_ignores_nonselectable_ui_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab_with_layout(
        size,
        ModeInfo::default(),
        r#"
        layout {
            pane size=1 borderless=true
            pane
            pane size=1 borderless=true
        }
    "#,
    );
    for terminal in [0, 2] {
        tab.get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(terminal)))
            .unwrap()
            .set_selectable(false);
    }
    let agent = PaneId::Terminal(TerminalId::new(1));
    mark_agent(&mut tab, agent);
    tab.max_agent_width = 80;
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    for terminal in [0, 2] {
        let pane = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(terminal)))
            .unwrap();
        assert_eq!(pane.render_geom(), pane.current_geom());
    }
}

#[test]
fn centered_agent_stack_shares_the_limit() {
    let mut tab = create_new_tab(
        Size {
            cols: 241,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    tab.new_stacked_pane(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        false,
        true,
        Some(agent),
        Some(client),
        None,
        None,
    )
    .unwrap();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    tab.unset_fullscreen();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
}

#[test]
fn pane_surround_plugin_tabs_share_the_limit_and_restore_it() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
    let (mut tab, receiver) = create_new_tab_with_plugin_receiver(
        Size {
            cols: 241,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    let layout = PaneTabsLayout {
        active: 0,
        tabs: ["Conversation", "Overview"]
            .map(|label| PaneTabDescriptor {
                label: label.into(),
                location: "yinz:fixture-plugin-for-tests".into(),
                initial_cwd: None,
                configuration: BTreeMap::new(),
            })
            .to_vec(),
    };
    let info = tab.mount_pane_tabs(agent, layout, 0, client).unwrap();
    draw(&mut tab);
    tab.select_pane_tab(info.id, 1, client).unwrap();
    let generation = receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::LoadPaneTab { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    let plugin = crate::PluginId::new(101);
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:fixture-plugin-for-tests",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(info.id, 1, generation, Ok((plugin, run)))
        .unwrap();
    let mut grid = host_grid(Size {
        cols: 241,
        rows: 20,
    });
    render_into(&mut tab, &mut grid);
    tab.handle_plugin_bytes(plugin, client, b"Plugin view".to_vec())
        .unwrap();
    render_into(&mut tab, &mut grid);
    assert_surround(&tab, PaneId::Plugin(plugin), &grid);
    let mut theme = tab.style.colors;
    theme.pane_surround = Some(PaletteColor::Rgb((4, 5, 6)));
    tab.update_theme(theme);
    render_into(&mut tab, &mut grid);
    assert_surround(&tab, PaneId::Plugin(plugin), &grid);
    let panel = tab.get_pane_with_id(PaneId::Plugin(plugin)).unwrap();
    assert_eq!(panel.get_content_columns(), 80);
    assert_eq!(panel.render_geom().x, 79);
    tab.select_pane_tab(info.id, 0, client).unwrap();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    tab.max_agent_width = 0;
    draw(&mut tab);
    for id in [agent, PaneId::Plugin(plugin)] {
        assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 239);
    }
}

#[test]
fn centered_agent_panel_gives_spare_width_to_ordinary_child() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    let mut tab = create_new_tab(
        Size {
            cols: 301,
            rows: 20,
        },
        ModeInfo::default(),
    );
    let client = ClientId::new(1);
    let left = PaneId::Terminal(TerminalId::new(2));
    let right = PaneId::Terminal(TerminalId::new(3));
    for id in [left, right] {
        tab.new_pane(
            id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(client),
            None,
        )
        .unwrap();
    }
    tab.create_multipane(
        [left, right],
        MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 5000,
        },
        0,
        OverlayPlacement::dismissible(None),
    )
    .unwrap();
    mark_agent(&mut tab, left);
    tab.max_agent_width = 80;
    tab.toggle_pane_fullscreen(left);
    draw(&mut tab);
    let agent = tab.get_pane_with_id(left).unwrap();
    let ordinary = tab.get_pane_with_id(right).unwrap();
    assert_eq!(agent.get_content_columns(), 80);
    assert_eq!(ordinary.get_content_columns(), 218);
    assert_eq!(agent.render_geom().x, 1);
    assert_eq!(ordinary.render_geom().x, 82);
    assert_eq!(
        tab.overlay_panes.seam_at(&Position::new(3, 81)),
        Some((left, right))
    );
}

#[test]
fn centered_agents_form_a_contiguous_grid_and_keep_chrome_full_width() {
    let size = Size {
        cols: 201,
        rows: 24,
    };
    let mut tab = create_new_tab_with_layout(
        size,
        ModeInfo::default(),
        r#"
        layout {
            pane size=1 borderless=true
            pane split_direction="vertical" {
                pane
                pane {
                    pane
                    pane
                }
            }
            pane size=1 borderless=true
        }
    "#,
    );
    for id in [0, 2] {
        tab.get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
            .unwrap()
            .set_selectable(false);
    }
    for id in [1, 3, 4] {
        mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(id)));
    }
    tab.max_agent_width = 80;
    draw(&mut tab);
    let geom = |id| {
        tab.get_pane_with_id(PaneId::Terminal(TerminalId::new(id)))
            .unwrap()
            .render_geom()
    };
    assert_eq!((geom(1).x, geom(1).cols.as_usize()), (18, 82));
    assert_eq!((geom(3).x, geom(3).cols.as_usize()), (100, 82));
    assert_eq!((geom(4).x, geom(4).cols.as_usize()), (100, 82));
    for id in [0, 2] {
        assert_eq!((geom(id).x, geom(id).cols.as_usize()), (0, 201));
    }
    for x in [18, 99] {
        assert_eq!(
            crate::tab::mouse_handler::MouseHandler::get_pane_at(
                &mut tab,
                &Position::new(3, x),
                true,
            )
            .unwrap()
            .unwrap()
            .pid(),
            PaneId::Terminal(TerminalId::new(1))
        );
    }
    assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
        &mut tab,
        &Position::new(3, 0),
        true,
    )
    .unwrap()
    .is_none());
    tab.max_agent_width = 0;
    draw(&mut tab);
    for id in [1, 3, 4] {
        let pane = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(id)))
            .unwrap();
        assert_eq!(pane.render_geom(), pane.current_geom());
    }
}

#[test]
fn pane_surround_sole_pane_theme_fallback_resize_and_disable() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, ModeInfo::default());
    let id = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, id);
    let mut grid = host_grid(size);
    render_into(&mut tab, &mut grid);
    assert_surround(&tab, id, &grid);
    for background in [(40, 42, 54), (248, 249, 250)] {
        tab.update_terminal_theme(Some(yinz_types::style::TerminalTheme {
            foreground: (255, 255, 255),
            background,
            ansi: [(0, 0, 0); 16],
        }));
        render_into(&mut tab, &mut grid);
        assert_surround(&tab, id, &grid);
    }
    tab.resize_whole_tab(Size { cols: 60, rows: 20 }).unwrap();
    render_into(&mut tab, &mut grid);
    assert_ne!(
        grid.get_character_at(0, 3).unwrap().styles.background,
        Some(tab.style.pane_surround().into())
    );
    tab.resize_whole_tab(size).unwrap();
    render_into(&mut tab, &mut grid);
    assert_surround(&tab, id, &grid);
    tab.max_agent_width = 0;
    render_into(&mut tab, &mut grid);
    for y in 0..size.rows {
        for x in 0..size.cols {
            assert_ne!(
                grid.get_character_at(x, y).unwrap().styles.background,
                Some(tab.style.pane_surround().into())
            );
        }
    }
}

#[test]
fn centered_agent_detection_restores_tiled_and_floating_shells() {
    let client = ClientId::new(1);
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Floating(None),
    ] {
        let mut tab = create_new_tab(
            Size {
                cols: 201,
                rows: 24,
            },
            ModeInfo::default(),
        );
        let id = PaneId::Terminal(TerminalId::new(2));
        tab.new_pane(id, None, None, false, true, placement, Some(client), None)
            .unwrap();
        tab.max_agent_width = 80;
        draw(&mut tab);
        let original = tab.get_pane_with_id(id).unwrap().render_geom();
        tab.get_pane_with_id_mut(id)
            .unwrap()
            .set_detected_agent(true);
        draw(&mut tab);
        let pane = tab.get_pane_with_id(id).unwrap();
        assert_eq!(pane.get_content_columns(), 80);
        assert!(pane.agent_info().is_none());
        assert!(!pane.was_an_agent());
        tab.get_pane_with_id_mut(id)
            .unwrap()
            .set_detected_agent(false);
        draw(&mut tab);
        assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom(), original);
    }
}

#[test]
fn centered_agent_linked_panels_stay_adjacent_in_both_directions() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        let mut tab = create_new_tab(
            Size {
                cols: 601,
                rows: 24,
            },
            ModeInfo::default(),
        );
        let client = ClientId::new(1);
        let ids = [
            PaneId::Terminal(TerminalId::new(2)),
            PaneId::Terminal(TerminalId::new(3)),
        ];
        for id in ids {
            tab.new_pane(
                id,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(client),
                None,
            )
            .unwrap();
            mark_agent(&mut tab, id);
        }
        tab.create_multipane(
            ids,
            MultipaneSplit {
                direction,
                ratio: 7000,
            },
            24,
            OverlayPlacement::dismissible(None),
        )
        .unwrap();
        tab.max_agent_width = 80;
        for fullscreen in [false, true] {
            if fullscreen {
                tab.toggle_pane_fullscreen(ids[0]);
            }
            draw(&mut tab);
            for id in ids {
                assert_eq!(tab.get_pane_with_id(id).unwrap().get_content_columns(), 80);
            }
            let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
            assert_eq!(
                outer.cols.as_usize(),
                if direction == MultipaneDirection::LeftRight {
                    163
                } else {
                    82
                }
            );
            if fullscreen {
                assert_eq!(outer.x, (601 - outer.cols.as_usize()) / 2);
            }
            let first = tab.get_pane_with_id(ids[0]).unwrap().render_geom();
            let second = tab.get_pane_with_id(ids[1]).unwrap().render_geom();
            if direction == MultipaneDirection::LeftRight {
                assert_eq!(first.x + first.cols.as_usize() + 1, second.x);
            } else {
                assert_eq!(first.x, second.x);
            }
        }
    }
}

#[test]
fn centered_agent_stack_headers_receive_clicks_at_their_rendered_rows() {
    let size = Size {
        cols: 241,
        rows: 30,
    };
    let client = ClientId::new(1);
    let expanded = PaneId::Terminal(TerminalId::new(0));
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        for limit in [0, 80] {
            let mut tab = create_new_tab_with_layout(
                size,
                ModeInfo::default(),
                r#"
                layout {
                    pane stacked=true {
                        pane focus=true
                        pane name="First collapsed"
                        pane name="Second collapsed"
                    }
                    pane
                }
                "#,
            );
            tab.set_pane_frames(style);
            tab.max_agent_width = limit;
            for id in [expanded, PaneId::Terminal(TerminalId::new(1))] {
                mark_agent(&mut tab, id);
            }
            for (id, title) in [(2, "First collapsed"), (3, "Second collapsed")] {
                let collapsed = PaneId::Terminal(TerminalId::new(id));
                for right_edge in [false, true] {
                    tab.tiled_panes.focus_pane(expanded, client);
                    tab.set_force_render();
                    let snapshot =
                        take_snapshot(&draw(&mut tab), size.rows, size.cols, Palette::default());
                    let row = snapshot
                        .lines()
                        .position(|line| line.contains(title))
                        .unwrap();
                    let pane = tab.get_pane_with_id(collapsed).unwrap();
                    let geom = pane.render_geom();
                    assert!(geom.rows.is_fixed());
                    assert_eq!(pane.max_content_width(), 0);
                    if limit == 80 {
                        assert!(pane.render_width().is_some());
                        assert!(geom.x > 0);
                        assert!(geom.x + geom.cols.as_usize() < size.cols);
                        for x in [geom.x - 1, geom.x + geom.cols.as_usize()] {
                            assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
                                &mut tab,
                                &Position::new(row as i32, x as u16),
                                false,
                            )
                            .unwrap()
                            .is_none());
                        }
                    }
                    let x = if right_edge {
                        geom.x + geom.cols.as_usize() - 1
                    } else {
                        geom.x
                    };
                    tab.handle_mouse_event(
                        &MouseEvent::new_left_press_event(Position::new(row as i32, x as u16)),
                        client,
                    )
                    .unwrap();
                    tab.handle_mouse_event(
                        &MouseEvent::new_left_release_event(Position::new(row as i32, x as u16)),
                        client,
                    )
                    .unwrap();
                    assert_eq!(
                        tab.get_active_pane_id(client),
                        Some(collapsed),
                        "style={style:?}, limit={limit}, title={title}, right_edge={right_edge}",
                    );
                }
            }
        }
    }
}

#[test]
fn centered_agents_without_full_frames_keep_text_and_dividers_inside_the_grid() {
    for style in [PaneFrameStyle::Titles, PaneFrameStyle::None] {
        let size = Size {
            cols: 241,
            rows: 20,
        };
        let mut tab = create_new_tab_with_layout(
            size,
            ModeInfo::default(),
            r#"
            layout {
                pane split_direction="vertical" {
                    pane
                    pane
                }
            }
        "#,
        );
        tab.set_pane_frames(style);
        tab.max_agent_width = 80;
        for id in [0, 1] {
            mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(id)));
        }
        draw(&mut tab);
        for (id, content) in [(0, b"AAAAAAAAAAAAAAAAAAAA"), (1, b"BBBBBBBBBBBBBBBBBBBB")] {
            tab.handle_pty_bytes(TerminalId::new(id), content.repeat(4))
                .unwrap();
        }
        tab.set_force_render();
        let snapshot = take_snapshot(&draw(&mut tab), size.rows, size.cols, Palette::default());
        assert!(
            snapshot.contains(&format!("{}│{}", "A".repeat(80), "B".repeat(80))),
            "{snapshot}"
        );
        let left = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(0)))
            .unwrap();
        let right = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
            .unwrap();
        assert_eq!(
            left.render_geom().x + left.render_geom().cols.as_usize(),
            right.render_geom().x
        );
    }
}

#[test]
fn centered_agents_focus_uses_rendered_spans_in_both_directions() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 400,
            rows: 24,
        },
        ModeInfo::default(),
        r#"layout {
            pane split_direction="vertical" { pane; pane; }
            pane split_direction="vertical" { pane; pane; }
        }"#,
    );
    let ids = [0, 1, 2, 3].map(|id| PaneId::Terminal(TerminalId::new(id)));
    // Layout allocation visits the two row containers before their children.
    let mut ids = ids;
    ids.sort_by_key(|id| {
        let g = tab.get_pane_with_id(*id).unwrap().current_geom();
        (g.y, g.x)
    });
    tab.max_agent_width = 80;
    mark_agent(&mut tab, ids[0]);
    mark_agent(&mut tab, ids[3]);
    draw(&mut tab);
    let requested: Vec<_> = ids
        .iter()
        .map(|id| tab.get_pane_with_id(*id).unwrap().current_geom())
        .collect();
    let client = ClientId::new(1);
    // Packing makes a recently focused neighbor overlap across the old column boundary.
    for (rank, id) in [ids[3], ids[2], ids[1]].into_iter().enumerate() {
        tab.focus_pane_with_id(id, false, false, client).unwrap();
        tab.get_pane_with_id_mut(id)
            .unwrap()
            .set_active_at(std::time::Instant::now() + std::time::Duration::from_secs(rank as u64));
    }
    assert!(tab.move_focus_down(client).unwrap());
    assert_eq!(tab.get_active_pane_id(client), Some(ids[2]));
    for (rank, id) in [ids[0], ids[1], ids[2]].into_iter().enumerate() {
        tab.focus_pane_with_id(id, false, false, client).unwrap();
        tab.get_pane_with_id_mut(id)
            .unwrap()
            .set_active_at(std::time::Instant::now() + std::time::Duration::from_secs(rank as u64));
    }
    assert!(tab.move_focus_up(client).unwrap());
    assert_eq!(tab.get_active_pane_id(client), Some(ids[1]));
    for (id, geom) in ids.iter().zip(requested) {
        assert_eq!(tab.get_pane_with_id(*id).unwrap().current_geom(), geom);
    }
}

#[test]
fn centered_agents_partial_row_ui_falls_back_and_invalidates_cached_packing() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 400,
            rows: 24,
        },
        ModeInfo::default(),
        r#"layout { pane; pane split_direction="vertical" { pane; pane; }; }"#,
    );
    let agent = PaneId::Terminal(TerminalId::new(2));
    let ui = PaneId::Terminal(TerminalId::new(1));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(0)));
    draw(&mut tab);
    let packed = tab.get_pane_with_id(agent).unwrap().render_geom();
    let requested = tab.get_pane_with_id(agent).unwrap().current_geom();
    let ui_geom = tab.get_pane_with_id(ui).unwrap().current_geom();
    tab.get_pane_with_id_mut(ui).unwrap().set_selectable(false);
    draw(&mut tab);
    let fallback = tab.get_pane_with_id(agent).unwrap().render_geom();
    assert_eq!(
        fallback.x,
        requested.x + (requested.cols.as_usize() - 82) / 2
    );
    assert!(fallback.x >= ui_geom.x + ui_geom.cols.as_usize());
    assert_eq!(tab.get_pane_with_id(ui).unwrap().render_geom(), ui_geom);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().current_geom(),
        requested
    );
    // Only the excluded UI geometry changes: it now occupies a separate row.
    let mut separate = ui_geom;
    separate.y = requested.y + requested.rows.as_usize();
    tab.get_pane_with_id_mut(ui).unwrap().set_geom(separate);
    draw(&mut tab);
    assert!(!tab.tiled_panes.agent_widths().unwrap().is_empty());
    assert_ne!(tab.get_pane_with_id(agent).unwrap().render_geom(), fallback);
    tab.get_pane_with_id_mut(ui).unwrap().set_geom(ui_geom);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(agent).unwrap().render_geom(), fallback);
    tab.get_pane_with_id_mut(ui).unwrap().set_selectable(true);
    draw(&mut tab);
    assert_eq!(tab.get_pane_with_id(agent).unwrap().render_geom(), packed);
}

#[test]
fn centered_agents_infeasible_grid_renders_in_original_slots_and_recovers() {
    // One capped pane cannot share a packed width with 83 panes requiring at least one column each.
    let layout = format!(
        "layout {{ pane; pane split_direction=\"vertical\" {{ {} }}; }}",
        "pane; ".repeat(83)
    );
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 1660,
            rows: 24,
        },
        ModeInfo::default(),
        &layout,
    );
    let agent = PaneId::Terminal(TerminalId::new(0));
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    let requested: Vec<_> = tab
        .tiled_panes
        .get_panes()
        .map(|(id, pane)| (*id, pane.current_geom()))
        .collect();
    draw(&mut tab);
    assert!(tab.tiled_panes.agent_widths().unwrap().is_empty());
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        80
    );
    for (id, geom) in requested {
        assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), geom);
        if id != agent {
            assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom(), geom);
        }
    }
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    assert!(!tab.tiled_panes.agent_widths().unwrap().is_empty());
    tab.unset_fullscreen();
    draw(&mut tab);
    assert!(tab.tiled_panes.agent_widths().unwrap().is_empty());
}

#[test]
fn centered_agent_threshold_and_live_resize_apply_in_every_layer() {
    for initial in [0, 1, 79, 80, 81] {
        for placement in [
            NewPanePlacement::default(),
            NewPanePlacement::Floating(None),
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        ] {
            let (sender, receiver) = channels::unbounded();
            let mut tab = create_new_tab_with_mock_pty_writer(
                Size {
                    cols: 300,
                    rows: 24,
                },
                ModeInfo::default(),
                SenderWithContext::new(sender),
            );
            let terminal = TerminalId::new(2);
            let id = PaneId::Terminal(terminal);
            tab.new_pane(
                id,
                None,
                None,
                false,
                true,
                placement,
                Some(ClientId::new(1)),
                None,
            )
            .unwrap();
            let requested = tab.get_pane_with_id(id).unwrap().current_geom();
            let unrestricted = tab.get_pane_with_id(id).unwrap().get_content_columns();
            mark_agent(&mut tab, id);
            for limit in [initial, 80, 79, 81, 0] {
                let before = tab.get_pane_with_id(id).unwrap().get_content_columns();
                receiver.try_iter().for_each(drop);
                tab.max_agent_width = limit;
                draw(&mut tab);
                let expected = if limit < 80 { unrestricted } else { limit };
                let pane = tab.get_pane_with_id(id).unwrap();
                assert_eq!(pane.current_geom(), requested);
                assert_eq!(pane.get_content_columns(), expected);
                if limit < 80 {
                    assert_eq!(pane.render_geom(), requested);
                }
                if before != expected {
                    assert!(receiver.try_iter().any(|(instruction, _)| matches!(instruction,
                        PtyWriteInstruction::ResizePty(pid, cols, _, _, _) if pid == terminal && usize::from(cols) == expected
                    )));
                }
            }
        }
    }
}

#[test]
fn centered_agent_margin_damage_respects_client_override_and_is_consumed() {
    use crate::ui::damage::CellRect;
    let size = Size {
        cols: 300,
        rows: 24,
    };
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.max_agent_width = 80;
    mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(1)));
    draw(&mut tab);
    let watcher = ClientId::new(2);
    tab.force_render_panes_intersecting(&[CellRect {
        x: 0,
        y: 3,
        cols: 5,
        rows: 1,
    }]);
    let mut output = Output::default();
    tab.render(&mut output, Some(watcher)).unwrap();
    let rendered = output.serialize().unwrap();
    let initial = "\u{1b}[4;1HXXXXX";
    let watcher_snapshot = take_snapshot(
        &format!("{initial}{}", rendered[&watcher]),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert!(!watcher_snapshot.contains("XXXXX"));
    let regular = rendered.get(&ClientId::new(1)).cloned().unwrap_or_default();
    let regular_snapshot = take_snapshot(
        &format!("{initial}{regular}"),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert!(regular_snapshot.contains("XXXXX"));
    assert!(tab.pending_render_damage.is_empty());
}

#[test]
fn centered_agent_packing_keeps_flexible_shell_content_visible() {
    let size = Size {
        cols: 400,
        rows: 24,
    };
    let mut tab = create_new_tab_with_layout(
        size,
        ModeInfo::default(),
        r#"layout { pane; pane split_direction="vertical" { pane size=80; pane; }; }"#,
    );
    tab.max_agent_width = 80;
    mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(0)));
    draw(&mut tab);
    let shell = PaneId::Terminal(TerminalId::new(2));
    let pane = tab.get_pane_with_id(shell).unwrap();
    assert!(pane.render_geom().cols.as_usize() >= pane.min_width());
    assert!(pane.get_content_columns() > 0);
    tab.handle_pty_bytes(TerminalId::new(2), b"XYZ".to_vec())
        .unwrap();
    assert!(
        take_snapshot(&draw(&mut tab), size.rows, size.cols, Palette::default()).contains("XYZ")
    );
    for (offset, expected_minimum) in [
        (yinz_types::pane_size::Offset::frame(3), 7),
        (yinz_types::pane_size::Offset::default(), 5),
    ] {
        tab.get_pane_with_id_mut(shell)
            .unwrap()
            .set_content_offset(offset);
        let widths = tab.tiled_panes.agent_widths().unwrap();
        assert_eq!(widths[&shell].1, expected_minimum);
    }
}

#[test]
fn capped_agent_undirected_resize_still_grows_height() {
    for by_id in [true, false] {
        let layout = yinz_config::layout::from_str(
            r#"layout { pane split_direction="vertical" { pane focus=true; pane; }; pane; }"#,
            "layout.kdl".into(),
            None,
            None,
        )
        .unwrap();
        let (tiled, floating) = layout.new_tab();
        let mut tab = create_new_tab_with_swap_layouts(
            Size {
                cols: 400,
                rows: 40,
            },
            ModeInfo::default(),
            (vec![], vec![]),
            Some((
                tiled,
                floating,
                (0..3).map(|id| (TerminalId::new(id), None)).collect(),
                vec![],
                HashMap::new(),
            )),
            true,
            false,
        );
        let agent = PaneId::Terminal(TerminalId::new(0));
        tab.max_agent_width = 80;
        mark_agent(&mut tab, agent);
        draw(&mut tab);
        let before = tab.get_pane_with_id(agent).unwrap().current_geom();
        assert_eq!(
            tab.get_pane_with_id(agent).unwrap().get_content_columns(),
            80
        );
        let strategy = ResizeStrategy::new(Resize::Increase, None);
        if by_id {
            tab.resize_pane_with_id(strategy, agent).unwrap();
        } else {
            tab.resize(ClientId::new(1), strategy).unwrap();
        }
        draw(&mut tab);
        let pane = tab.get_pane_with_id(agent).unwrap();
        assert_eq!(
            pane.current_geom().rows.as_usize(),
            before.rows.as_usize() + 2
        );
        assert_eq!(pane.current_geom().cols, before.cols);
        assert_eq!(pane.get_content_columns(), 80);
    }
}

#[test]
fn capped_agent_stacked_resize_falls_back_to_stacking() {
    let client = ClientId::new(1);
    for (agent_id, direction) in [(0, Direction::Right), (1, Direction::Left)] {
        let mut tab = create_new_tab_with_layout(
            Size {
                cols: 400,
                rows: 40,
            },
            ModeInfo::default(),
            r#"layout split_direction="vertical" { pane; pane; }"#,
        );
        let agent = PaneId::Terminal(TerminalId::new(agent_id));
        let neighbor = PaneId::Terminal(TerminalId::new(1 - agent_id));
        tab.max_agent_width = 80;
        mark_agent(&mut tab, agent);
        tab.focus_pane_with_id(agent, false, false, client).unwrap();
        draw(&mut tab);
        assert_eq!(
            tab.get_pane_with_id(agent).unwrap().get_content_columns(),
            80
        );
        let before = [agent, neighbor].map(|id| tab.get_pane_with_id(id).unwrap().current_geom());
        assert!(!tab
            .tiled_panes
            .resize_pane_with_id(
                ResizeStrategy::new(Resize::Increase, Some(direction)),
                agent,
                None,
            )
            .unwrap());
        for (id, geom) in [agent, neighbor].into_iter().zip(before) {
            assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), geom);
        }

        tab.resize(client, ResizeStrategy::new(Resize::Increase, None))
            .unwrap();
        let agent_geom = tab.get_pane_with_id(agent).unwrap().current_geom();
        let neighbor_geom = tab.get_pane_with_id(neighbor).unwrap().current_geom();
        assert!(agent_geom.is_stacked());
        assert_eq!(agent_geom.stacked, neighbor_geom.stacked);
        assert_eq!(agent_geom.cols.as_usize(), 400);
    }
}

#[test]
fn horizontal_resize_ignores_unchanged_undersized_pane_after_terminal_shrink() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 800,
            rows: 40,
        },
        ModeInfo::default(),
        r#"layout {
            pane split_direction="vertical" { pane size="1%"; pane; }
            pane split_direction="vertical" { pane; pane; }
        }"#,
    );
    tab.resize_whole_tab(Size {
        cols: 400,
        rows: 40,
    })
    .unwrap();
    let narrow = PaneId::Terminal(TerminalId::new(0));
    let before = tab.get_pane_with_id(narrow).unwrap().current_geom();
    assert_eq!(before.cols.as_usize(), 4);
    let resizing = PaneId::Terminal(TerminalId::new(1));
    assert_eq!(
        tab.get_pane_with_id(resizing)
            .unwrap()
            .current_geom()
            .cols
            .as_usize(),
        200
    );
    tab.resize_pane_with_id(
        ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        resizing,
    )
    .unwrap();
    assert_eq!(
        tab.get_pane_with_id(resizing)
            .unwrap()
            .current_geom()
            .cols
            .as_usize(),
        220
    );
    assert_eq!(tab.get_pane_with_id(narrow).unwrap().current_geom(), before);
    assert_eq!(
        tab.get_pane_with_id(PaneId::Terminal(TerminalId::new(3)))
            .unwrap()
            .current_geom()
            .cols
            .as_usize(),
        180,
    );
}

#[test]
fn capped_agent_resize_with_side_chrome_uses_displayed_columns() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 800,
            rows: 20,
        },
        ModeInfo::default(),
        "layout {\n pane split_direction=\"vertical\" { pane; pane; pane size=40; }\n}",
    );
    let agent = PaneId::Terminal(TerminalId::new(0));
    tab.set_pane_selectable(PaneId::Terminal(TerminalId::new(2)), false);
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    let pane = tab.get_pane_with_id(agent).unwrap();
    assert!(pane.render_width().is_none());
    assert_eq!(pane.get_content_columns(), 80);
    let before = pane.current_geom();
    let strategy = ResizeStrategy::new(Resize::Decrease, Some(Direction::Right));
    assert!(!tab
        .tiled_panes
        .resize_pane_with_strategies(agent, &[strategy], (0.01, 0.0))
        .unwrap());
    assert_eq!(tab.get_pane_with_id(agent).unwrap().current_geom(), before);
    let movement = tab.viewport.borrow().cols / 20;
    tab.resize_pane_with_id(strategy, agent).unwrap();
    for _ in 0..2 {
        draw(&mut tab);
        assert_eq!(
            tab.get_pane_with_id(agent).unwrap().get_content_columns(),
            80 - movement
        );
    }
}

#[test]
fn packed_agent_resize_preserves_stacks_and_frame_offsets() {
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let mut tab = create_new_tab_with_layout(
            Size { cols: 400, rows: 40 }, ModeInfo::default(),
            "layout {\n pane\n pane split_direction=\"vertical\" {\n pane stacked=true { pane; pane; }\n pane\n }\n}",
        );
        tab.set_pane_frames(style);
        tab.max_agent_width = 80;
        for id in tab.get_all_pane_ids() {
            mark_agent(&mut tab, id);
        }
        draw(&mut tab);
        let stack: Vec<_> = tab
            .tiled_panes
            .get_panes()
            .filter(|(_, pane)| pane.current_geom().is_stacked())
            .map(|(id, _)| *id)
            .collect();
        let before: Vec<_> = stack
            .iter()
            .map(|id| {
                let pane = tab.get_pane_with_id(*id).unwrap();
                (pane.current_geom(), pane.get_content_columns())
            })
            .collect();
        for (resize, delta) in [(Resize::Decrease, 20), (Resize::Increase, 0)] {
            tab.resize_pane_with_id(
                ResizeStrategy::new(resize, Some(Direction::Right)),
                stack[0],
            )
            .unwrap();
            for _ in 0..2 {
                draw(&mut tab);
                for (id, (geom, cols)) in stack.iter().zip(&before) {
                    let pane = tab.get_pane_with_id(*id).unwrap();
                    assert_eq!(pane.get_content_columns(), cols - delta);
                    assert_eq!(pane.current_geom().stacked, geom.stacked);
                    assert_eq!(
                        (pane.current_geom().y, pane.current_geom().rows),
                        (geom.y, geom.rows)
                    );
                }
                let first = tab.get_pane_with_id(stack[0]).unwrap().render_geom();
                let second = tab.get_pane_with_id(stack[1]).unwrap().render_geom();
                assert_eq!((first.x, first.cols), (second.x, second.cols));
            }
        }
    }
}

#[test]
fn blocked_tiled_drags_preserve_automatic_layouts() {
    let client = ClientId::new(1);
    for (layout, capped) in [
        ("layout { pane; }", false),
        (
            "layout {\n pane split_direction=\"vertical\" { pane; pane size=40; }\n}",
            false,
        ),
        (
            "layout {\n pane split_direction=\"vertical\" { pane; pane; pane size=350; }\n}",
            false,
        ),
        (
            "layout {\n pane split_direction=\"vertical\" { pane; pane; }\n}",
            true,
        ),
    ] {
        let mut tab = create_new_tab_with_layout(
            Size {
                cols: 400,
                rows: 40,
            },
            ModeInfo::default(),
            layout,
        );
        let agent = PaneId::Terminal(TerminalId::new(0));
        if capped {
            tab.max_agent_width = 80;
            for id in tab.get_all_pane_ids() {
                mark_agent(&mut tab, id);
            }
        }
        draw(&mut tab);
        let before: Vec<_> = tab
            .tiled_panes
            .get_panes()
            .map(|(id, pane)| (*id, pane.current_geom()))
            .collect();
        let geom = tab.get_pane_with_id(agent).unwrap().render_geom();
        let from = Position::new(5, (geom.x + geom.cols.as_usize() - 1) as u16);
        tab.handle_mouse_event(&MouseEvent::new_left_press_with_ctrl_event(from), client)
            .unwrap();
        for dx in [0, 1, if capped { 2 } else { 0 }] {
            let to = Position::new(5, (from.column() + dx) as u16);
            tab.handle_mouse_event(&MouseEvent::new_left_motion_with_ctrl_event(to), client)
                .unwrap();
            let state = tab.pane_being_resized_with_mouse.as_ref().unwrap();
            assert_eq!(state.start_position, to);
            assert!(!tab.swap_layouts.is_tiled_damaged(), "{layout}");
            assert_eq!(
                tab.tiled_panes
                    .get_panes()
                    .map(|(id, pane)| (*id, pane.current_geom()))
                    .collect::<Vec<_>>(),
                before
            );
        }
        let last = tab
            .pane_being_resized_with_mouse
            .as_ref()
            .unwrap()
            .start_position;
        tab.handle_mouse_event(&MouseEvent::new_left_release_with_ctrl_event(last), client)
            .unwrap();
        let swaps = yinz_config::kdl::layout::from_kdl(
            "layout {\n swap_tiled_layout {\n tab {\n pane { children; }\n }\n }\n}",
            None,
            None,
            None,
        )
        .unwrap();
        tab.swap_layouts
            .set_swap_tiled_layouts(swaps.swap_tiled_layouts);
        let new = PaneId::Terminal(TerminalId::new(9));
        tab.new_pane(
            new,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
        assert!(!tab.swap_layouts.is_tiled_damaged());
        assert!(tab
            .tiled_panes
            .get_panes()
            .all(|(_, pane)| pane.current_geom().cols.as_usize() == 400));
        tab.close_pane(new, false, None);
        assert!(!tab.swap_layouts.is_tiled_damaged());
        assert!(tab
            .tiled_panes
            .get_panes()
            .all(|(_, pane)| pane.current_geom().cols.as_usize() == 400));
    }
}

#[test]
fn packed_agent_resize_tracks_displayed_columns() {
    for below in [false, true] {
        for right in [false, true] {
            for other_is_agent in [false, true] {
                let (sender, receiver) = channels::unbounded();
                let mut tab = create_new_tab_with_mock_pty_writer(
                    Size {
                        cols: 400,
                        rows: 40,
                    },
                    ModeInfo::default(),
                    SenderWithContext::new(sender),
                );
                let client = ClientId::new(1);
                let first = PaneId::Terminal(TerminalId::new(1));
                let second = PaneId::Terminal(TerminalId::new(2));
                let third = PaneId::Terminal(TerminalId::new(3));
                tab.horizontal_split(second, None, None, client, None, None)
                    .unwrap();
                let left = if below { second } else { first };
                tab.focus_pane_with_id(left, false, false, client).unwrap();
                tab.vertical_split(third, None, None, client, None, None)
                    .unwrap();
                let agent = if right { third } else { left };
                let neighbor = if right { left } else { third };
                tab.max_agent_width = 80;
                for id in [first, second, third] {
                    if id != neighbor || other_is_agent {
                        mark_agent(&mut tab, id);
                    }
                }
                draw(&mut tab);
                let pane = tab.get_pane_with_id(agent).unwrap();
                assert_eq!(pane.get_content_columns(), 39);
                let geom = pane.render_geom();
                let x = if right {
                    geom.x
                } else {
                    geom.x + geom.cols.as_usize() - 1
                };
                let mut position = Position::new((geom.y + 5) as i32, x as u16);
                tab.handle_mouse_event(
                    &MouseEvent::new_left_press_with_ctrl_event(position),
                    client,
                )
                .unwrap();
                let mut cols: usize = 39;
                for delta in [-1, -4, 2, 3, 1] {
                    position = Position::new(
                        position.line() as i32,
                        position
                            .column()
                            .checked_add_signed(if right { -delta } else { delta })
                            .unwrap() as u16,
                    );
                    cols = cols.checked_add_signed(delta).unwrap();
                    tab.handle_mouse_event(
                        &MouseEvent::new_left_motion_with_ctrl_event(position),
                        client,
                    )
                    .unwrap();
                    assert!(tab.pane_being_resized_with_mouse.is_some());
                    let PaneId::Terminal(terminal) = agent else {
                        unreachable!()
                    };
                    assert_resized_agent(&mut tab, &receiver, terminal, cols, 18);
                    let left_geom = tab.get_pane_with_id(left).unwrap().render_geom();
                    let right_geom = tab.get_pane_with_id(third).unwrap().render_geom();
                    assert_eq!(left_geom.x + left_geom.cols.as_usize(), right_geom.x);
                    assert_eq!(left_geom.cols.as_usize() + right_geom.cols.as_usize(), 82);
                }
                tab.handle_mouse_event(
                    &MouseEvent::new_left_release_with_ctrl_event(position),
                    client,
                )
                .unwrap();
                let direction = if right {
                    Direction::Left
                } else {
                    Direction::Right
                };
                for (resize, expected) in [(Resize::Decrease, 20), (Resize::Increase, 40)] {
                    tab.resize_pane_with_id(ResizeStrategy::new(resize, Some(direction)), agent)
                        .unwrap();
                    let PaneId::Terminal(terminal) = agent else {
                        unreachable!()
                    };
                    assert_resized_agent(&mut tab, &receiver, terminal, expected, 18);
                }
            }
        }
    }
}

#[test]
fn centered_agent_resize_uses_rendered_edges() {
    use yinz_panes::pane::PaneEdge;
    let (sender, receiver) = channels::unbounded();
    let mut tab = create_new_tab_with_mock_pty_writer(
        Size {
            cols: 400,
            rows: 20,
        },
        ModeInfo::default(),
        SenderWithContext::new(sender),
    );
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(1));
    tab.vertical_split(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        client,
        None,
        None,
    )
    .unwrap();
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    let pane = tab.get_pane_with_id(agent).unwrap();
    let geom = pane.render_geom();
    let right = geom.x + geom.cols.as_usize() - 1;
    for (position, edge) in [
        (Position::new(0, geom.x as u16), PaneEdge::TopLeft),
        (Position::new(0, right as u16), PaneEdge::TopRight),
        (Position::new(19, geom.x as u16), PaneEdge::BottomLeft),
        (Position::new(19, right as u16), PaneEdge::BottomRight),
    ] {
        assert_eq!(pane.get_edge_at_position(&position), Some(edge));
    }
    let position = Position::new(5, right as u16);
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(position),
        client,
    )
    .unwrap();
    // One drag: shrink immediately, shrink further, reverse, return to the cap,
    // then hit the minimum and reverse without losing the drag.
    let mut column = right as i32;
    for (delta, expected_cols) in [(-1, 79), (-4, 75), (2, 77), (3, 80), (-60, 20)] {
        column += delta;
        let moved = Position::new(5, column as u16);
        tab.handle_mouse_event(&MouseEvent::new_left_motion_with_ctrl_event(moved), client)
            .unwrap();
        assert!(tab.pane_being_resized_with_mouse.is_some());
        assert_resized_agent(&mut tab, &receiver, TerminalId::new(1), expected_cols, 18);
        let left = tab.get_pane_with_id(agent).unwrap().render_geom();
        let neighbor = tab
            .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
            .unwrap();
        assert_eq!(left.x + left.cols.as_usize(), neighbor.render_geom().x);
        assert!(neighbor.render_geom().cols.as_usize() >= neighbor.min_width());
    }
    column -= 4;
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_with_ctrl_event(Position::new(5, column as u16)),
        client,
    )
    .unwrap();
    draw(&mut tab);
    assert_eq!(
        tab.get_pane_with_id(agent).unwrap().get_content_columns(),
        20
    );
    assert!(tab.pane_being_resized_with_mouse.is_some());
    column += 2;
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_with_ctrl_event(Position::new(5, column as u16)),
        client,
    )
    .unwrap();
    assert_resized_agent(&mut tab, &receiver, TerminalId::new(1), 22, 18);
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_with_ctrl_event(Position::new(5, column as u16)),
        client,
    )
    .unwrap();
    assert!(tab.pane_being_resized_with_mouse.is_none());
}

fn assert_resized_agent(
    tab: &mut Tab,
    receiver: &Receiver<(PtyWriteInstruction, ErrorContext)>,
    terminal: TerminalId,
    cols: usize,
    rows: usize,
) {
    for render in [false, true, true] {
        if render {
            draw(tab);
        }
        let pane = tab.get_pane_with_id(PaneId::Terminal(terminal)).unwrap();
        assert_eq!(
            (pane.get_content_columns(), pane.get_content_rows()),
            (cols, rows)
        );
        assert_eq!(
            (
                pane.render_geom().cols.as_usize(),
                pane.render_geom().rows.as_usize()
            ),
            (cols + 2, rows + 2)
        );
    }
    let last_size = receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::ResizePty(id, cols, rows, _, _) if id == terminal => {
                Some((cols as usize, rows as usize))
            },
            _ => None,
        })
        .last();
    assert_eq!(last_size, Some((cols, rows)));
}

#[test]
fn centered_agent_directional_resize_preserves_visible_width() {
    for limit in [0, 80] {
        for other_is_agent in [false, true] {
            for direction in [Direction::Left, Direction::Right] {
                let (sender, receiver) = channels::unbounded();
                let mut tab = create_new_tab_with_mock_pty_writer(
                    Size {
                        cols: 400,
                        rows: 20,
                    },
                    ModeInfo::default(),
                    SenderWithContext::new(sender),
                );
                let client = ClientId::new(1);
                tab.vertical_split(
                    PaneId::Terminal(TerminalId::new(2)),
                    None,
                    None,
                    client,
                    None,
                    None,
                )
                .unwrap();
                let terminal = TerminalId::new(if direction == Direction::Right { 1 } else { 2 });
                let agent = PaneId::Terminal(terminal);
                tab.max_agent_width = limit;
                mark_agent(&mut tab, agent);
                if other_is_agent {
                    mark_agent(
                        &mut tab,
                        PaneId::Terminal(TerminalId::new(if terminal == TerminalId::new(1) {
                            2
                        } else {
                            1
                        })),
                    );
                }
                draw(&mut tab);
                let initial = tab.get_pane_with_id(agent).unwrap().get_content_columns();
                for (resize, expected) in [
                    (Resize::Decrease, initial - 20),
                    (Resize::Decrease, initial - 40),
                    (Resize::Increase, initial - 20),
                    (Resize::Increase, initial),
                ] {
                    tab.resize_pane_with_id(ResizeStrategy::new(resize, Some(direction)), agent)
                        .unwrap();
                    assert_resized_agent(&mut tab, &receiver, terminal, expected, 18);
                    let left = tab
                        .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
                        .unwrap()
                        .render_geom();
                    let right = tab
                        .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
                        .unwrap()
                        .render_geom();
                    assert_eq!(left.x + left.cols.as_usize(), right.x);
                    if expected == initial - 40 {
                        tab.resize_whole_tab(Size {
                            cols: 800,
                            rows: 20,
                        })
                        .unwrap();
                        let doubled = (expected + 2) * 2 - 2;
                        let widened = if limit == 80 {
                            doubled.min(80)
                        } else {
                            doubled
                        };
                        draw(&mut tab);
                        assert_resized_agent(&mut tab, &receiver, terminal, widened, 18);
                        tab.resize_whole_tab(Size {
                            cols: 400,
                            rows: 20,
                        })
                        .unwrap();
                        draw(&mut tab);
                        assert_resized_agent(&mut tab, &receiver, terminal, expected, 18);
                    }
                }
            }
        }
    }
}

fn centered_agent_vertical_drag(diagonal: bool) {
    for limit in [0, 80] {
        for below in [false, true] {
            for other_is_agent in [false, true] {
                let (sender, receiver) = channels::unbounded();
                let mut tab = create_new_tab_with_mock_pty_writer(
                    Size {
                        cols: 400,
                        rows: 40,
                    },
                    ModeInfo::default(),
                    SenderWithContext::new(sender),
                );
                let client = ClientId::new(1);
                tab.horizontal_split(
                    PaneId::Terminal(TerminalId::new(2)),
                    None,
                    None,
                    client,
                    None,
                    None,
                )
                .unwrap();
                let terminal = TerminalId::new(if below { 2 } else { 1 });
                let agent = PaneId::Terminal(terminal);
                tab.max_agent_width = limit;
                mark_agent(&mut tab, agent);
                if other_is_agent {
                    mark_agent(
                        &mut tab,
                        PaneId::Terminal(TerminalId::new(if below { 1 } else { 2 })),
                    );
                }
                draw(&mut tab);
                let geom = tab.get_pane_with_id(agent).unwrap().render_geom();
                let row = if below {
                    geom.y
                } else {
                    geom.y + geom.rows.as_usize() - 1
                };
                let column = geom.x
                    + if diagonal {
                        geom.cols.as_usize() - 1
                    } else {
                        geom.cols.as_usize() / 2
                    };
                let mut position = Position::new(row as i32, column as u16);
                tab.handle_mouse_event(
                    &MouseEvent::new_left_press_with_ctrl_event(position),
                    client,
                )
                .unwrap();
                for delta in [-2, 4, -2] {
                    position = Position::new(
                        position.line() as i32 + delta,
                        (position.column() + usize::from(diagonal)) as u16,
                    );
                    tab.handle_mouse_event(
                        &MouseEvent::new_left_motion_with_ctrl_event(position),
                        client,
                    )
                    .unwrap();
                    assert!(tab.pane_being_resized_with_mouse.is_some());
                    let movement = position.line() - row as isize;
                    let rows = (18 + if below { -movement } else { movement }) as usize;
                    assert_resized_agent(
                        &mut tab,
                        &receiver,
                        terminal,
                        geom.cols.as_usize() - 2,
                        rows,
                    );
                    let top = tab
                        .get_pane_with_id(PaneId::Terminal(TerminalId::new(1)))
                        .unwrap()
                        .render_geom();
                    let bottom = tab
                        .get_pane_with_id(PaneId::Terminal(TerminalId::new(2)))
                        .unwrap()
                        .render_geom();
                    assert_eq!(top.y + top.rows.as_usize(), bottom.y);
                    assert_eq!(top.rows.as_usize() + bottom.rows.as_usize(), 40);
                }
                tab.handle_mouse_event(
                    &MouseEvent::new_left_release_with_ctrl_event(position),
                    client,
                )
                .unwrap();
                assert!(tab.pane_being_resized_with_mouse.is_none());
            }
        }
    }
}

#[test]
fn centered_agent_drag_resizes_height() {
    centered_agent_vertical_drag(false);
}

#[test]
fn centered_agent_diagonal_drag_keeps_resizing_height_when_width_is_blocked() {
    centered_agent_vertical_drag(true);
}

#[test]
fn centered_agent_drag_preserves_fixed_neighbors_and_reports_unexpected_errors() {
    let mut tab = create_new_tab_with_layout(
        Size { cols: 400, rows: 40 },
        ModeInfo::default(),
        "layout {\n pane split_direction=\"vertical\" {\n pane { pane; pane; }\n pane size=40\n }\n}",
    );
    let agent = PaneId::Terminal(TerminalId::new(0));
    let fixed = tab
        .tiled_panes
        .get_panes()
        .find_map(|(id, pane)| pane.current_geom().cols.is_fixed().then_some(*id))
        .unwrap();
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    draw(&mut tab);
    let fixed_geom = tab.get_pane_with_id(fixed).unwrap().current_geom();
    let geom = tab.get_pane_with_id(agent).unwrap().render_geom();
    let client = ClientId::new(1);
    assert!(!tab.swap_layouts.is_tiled_damaged());
    let x = geom.x + geom.cols.as_usize() - 1;
    let y = geom.y + geom.rows.as_usize() - 1;
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_with_ctrl_event(Position::new(y as i32, x as u16)),
        client,
    )
    .unwrap();
    for (dx, dy) in [(1, 0), (2, -2)] {
        tab.handle_mouse_event(
            &MouseEvent::new_left_motion_with_ctrl_event(Position::new(
                y as i32 + dy,
                (x + dx) as u16,
            )),
            client,
        )
        .unwrap();
        assert!(tab.pane_being_resized_with_mouse.is_some());
        assert_eq!(tab.swap_layouts.is_tiled_damaged(), dy != 0);
        assert_eq!(
            tab.get_pane_with_id(fixed).unwrap().current_geom(),
            fixed_geom
        );
    }
    assert_eq!(tab.get_pane_with_id(agent).unwrap().get_content_rows(), 16);
    let error = tab
        .tiled_panes
        .resize_pane_with_strategies(
            PaneId::Terminal(TerminalId::new(999)),
            &[ResizeStrategy::new(
                Resize::Decrease,
                Some(Direction::Right),
            )],
            (1.0, 1.0),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("no tiled pane with ID"));
}

#[test]
fn centered_agent_linked_panel_mouse_hits_include_frame_and_divider() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        for capped in [0, 1] {
            let mut tab = create_new_tab(
                Size {
                    cols: 600,
                    rows: 30,
                },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let ids = [
                PaneId::Terminal(TerminalId::new(2)),
                PaneId::Terminal(TerminalId::new(3)),
            ];
            for id in ids {
                tab.new_pane(
                    id,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                    Some(client),
                    None,
                )
                .unwrap();
            }
            tab.create_multipane(
                ids,
                MultipaneSplit {
                    direction,
                    ratio: 5000,
                },
                0,
                OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(20)),
                    y: Some(PercentOrFixed::Fixed(3)),
                    width: Some(PercentOrFixed::Fixed(300)),
                    height: Some(PercentOrFixed::Fixed(20)),
                    ..Default::default()
                })),
            )
            .unwrap();
            tab.max_agent_width = 80;
            mark_agent(&mut tab, ids[capped]);
            draw(&mut tab);
            let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
            let frame = Position::new((outer.y + 1) as i32, outer.x as u16);
            tab.handle_mouse_event(&MouseEvent::new_left_press_event(frame), client)
                .unwrap();
            assert!(tab.overlay_panes.pane_is_being_moved_with_mouse());
            let moved = Position::new((frame.line() + 1) as i32, (frame.column() + 5) as u16);
            tab.handle_mouse_event(&MouseEvent::new_left_motion_event(moved), client)
                .unwrap();
            tab.handle_mouse_event(&MouseEvent::new_left_release_event(moved), client)
                .unwrap();
            draw(&mut tab);
            assert_ne!(tab.overlay_panes.outer_geom(ids[0]).unwrap(), outer);
            let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
            let corner =
                Position::new(outer.y as i32, (outer.x + outer.cols.as_usize() - 1) as u16);
            tab.handle_mouse_event(&MouseEvent::new_left_press_with_ctrl_event(corner), client)
                .unwrap();
            assert!(tab.pane_being_resized_with_mouse.is_some());
            tab.handle_mouse_event(
                &MouseEvent::new_left_release_with_ctrl_event(corner),
                client,
            )
            .unwrap();
            let first = tab.get_pane_with_id(ids[0]).unwrap().render_geom();
            let seam = match direction {
                MultipaneDirection::LeftRight => {
                    Position::new(first.y as i32, (first.x + first.cols.as_usize()) as u16)
                },
                MultipaneDirection::TopBottom => {
                    Position::new((first.y + first.rows.as_usize()) as i32, first.x as u16)
                },
            };
            assert_eq!(tab.overlay_panes.seam_at(&seam), Some((ids[0], ids[1])));
            tab.handle_mouse_event(&MouseEvent::new_left_press_event(seam), client)
                .unwrap();
            assert!(tab.overlay_panes.seam_is_being_dragged());
            let moved = match direction {
                MultipaneDirection::LeftRight => {
                    let column = if capped == 0 {
                        seam.column() - 5
                    } else {
                        seam.column() + 5
                    };
                    Position::new(seam.line() as i32, column as u16)
                },
                MultipaneDirection::TopBottom => {
                    Position::new((seam.line() - 2) as i32, seam.column() as u16)
                },
            };
            assert!(
                tab.handle_mouse_event(&MouseEvent::new_left_motion_event(moved), client)
                    .unwrap()
                    .state_changed
            );
            tab.handle_mouse_event(&MouseEvent::new_left_release_event(moved), client)
                .unwrap();
            assert!(!tab.overlay_panes.seam_is_being_dragged());
            draw(&mut tab);
            let first = tab.get_pane_with_id(ids[0]).unwrap().render_geom();
            let covered_seam = match direction {
                MultipaneDirection::LeftRight => {
                    Position::new(first.y as i32, (first.x + first.cols.as_usize()) as u16)
                },
                MultipaneDirection::TopBottom => {
                    Position::new((first.y + first.rows.as_usize()) as i32, first.x as u16)
                },
            };
            let covering_agent = PaneId::Terminal(TerminalId::new(4));
            tab.new_pane(
                covering_agent,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                Some(client),
                None,
            )
            .unwrap();
            mark_agent(&mut tab, covering_agent);
            tab.toggle_pane_fullscreen(covering_agent);
            draw(&mut tab);
            assert_eq!(
                tab.overlay_panes.seam_at(&covered_seam),
                Some((ids[0], ids[1]))
            );
            assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
                &mut tab,
                &covered_seam,
                false
            )
            .unwrap()
            .is_none());
            tab.close_pane(covering_agent, false, None);
            tab.toggle_pane_fullscreen(ids[0]);
            draw(&mut tab);
            let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
            if outer.x > 0 {
                assert!(crate::tab::mouse_handler::MouseHandler::get_pane_at(
                    &mut tab,
                    &Position::new(5, 0),
                    false
                )
                .unwrap()
                .is_none());
            }
        }
    }
}

#[test]
fn centered_agent_fullscreen_margin_damage_repaints_at_its_layer() {
    use crate::ui::damage::CellRect;
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    let size = Size {
        cols: 400,
        rows: 24,
    };
    for overlay in [false, true] {
        for linked in [false, true] {
            if linked && !overlay {
                continue;
            }
            for no_ui in [false, true] {
                let mut tab = create_new_tab(size, ModeInfo::default());
                let client = ClientId::new(1);
                let watcher = ClientId::new(2);
                let ids = [
                    PaneId::Terminal(TerminalId::new(2)),
                    PaneId::Terminal(TerminalId::new(3)),
                ];
                tab.max_agent_width = 80;
                for id in ids.into_iter().take(if linked { 2 } else { 1 }) {
                    let placement = if overlay {
                        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None))
                    } else {
                        NewPanePlacement::Floating(None)
                    };
                    tab.new_pane(id, None, None, false, true, placement, Some(client), None)
                        .unwrap();
                    mark_agent(&mut tab, id);
                }
                if linked {
                    tab.create_multipane(
                        ids,
                        MultipaneSplit {
                            direction: MultipaneDirection::LeftRight,
                            ratio: 5000,
                        },
                        0,
                        OverlayPlacement::dismissible(None),
                    )
                    .unwrap();
                }
                if no_ui {
                    tab.toggle_pane_no_ui_fullscreen(ids[0]);
                } else {
                    tab.toggle_pane_fullscreen(ids[0]);
                }
                draw(&mut tab);
                let initial = "\u{1b}[4;1HXXXXX\u{1b}[4;396HXXXXX";
                tab.force_render_panes_intersecting(&[
                    CellRect {
                        x: 0,
                        y: 3,
                        cols: 5,
                        rows: 1,
                    },
                    CellRect {
                        x: 395,
                        y: 3,
                        cols: 5,
                        rows: 1,
                    },
                ]);
                let mut output = Output::default();
                tab.render(&mut output, Some(watcher)).unwrap();
                let rendered = output.serialize().unwrap();
                let snapshot = take_snapshot(
                    &format!(
                        "{initial}{}",
                        rendered.get(&watcher).cloned().unwrap_or_default()
                    ),
                    size.rows,
                    size.cols,
                    Palette::default(),
                );
                assert!(
                    !snapshot.contains("XXXXX"),
                    "overlay={overlay}, linked={linked}, no_ui={no_ui}"
                );
                let regular = rendered.get(&client).cloned().unwrap_or_default();
                assert!(take_snapshot(
                    &format!("{initial}{regular}"),
                    size.rows,
                    size.cols,
                    Palette::default()
                )
                .contains("XXXXX"));
                assert!(tab.pending_render_damage.is_empty());
            }
        }
    }
}

#[test]
fn centered_agent_fullscreen_margin_damage_preserves_higher_overlays() {
    use crate::ui::damage::CellRect;
    let size = Size {
        cols: 400,
        rows: 24,
    };
    let mut tab = create_new_tab(size, ModeInfo::default());
    let client = ClientId::new(1);
    let agent = PaneId::Terminal(TerminalId::new(2));
    tab.new_pane(
        agent,
        None,
        None,
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(client),
        None,
    )
    .unwrap();
    tab.max_agent_width = 80;
    mark_agent(&mut tab, agent);
    tab.toggle_pane_fullscreen(agent);
    draw(&mut tab);
    let overlay = PaneId::Terminal(TerminalId::new(3));
    tab.new_pane(
        overlay,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(0)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                ..Default::default()
            },
        ))),
        Some(client),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(TerminalId::new(3), b"HIGHER".to_vec())
        .unwrap();
    let initial = draw(&mut tab);
    assert!(take_snapshot(&initial, size.rows, size.cols, Palette::default()).contains("HIGHER"));
    tab.force_render_panes_intersecting(&[CellRect {
        x: 0,
        y: 3,
        cols: 40,
        rows: 1,
    }]);
    let repaint = draw(&mut tab);
    let snapshot = take_snapshot(
        &format!("{initial}\u{1b}[4;31HXXXXX{repaint}"),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert!(snapshot.contains("HIGHER"));
    assert!(!snapshot.contains("XXXXX"));
}

#[test]
fn centered_agent_over_chrome_keeps_displayed_anchor_on_resize() {
    let client = ClientId::new(1);
    let id = PaneId::Terminal(TerminalId::new(3));
    for bottom in [false, true] {
        for limit in [0, 80] {
            let mut tab = create_new_tab_with_layout(
                Size {
                    cols: 400,
                    rows: 30,
                },
                ModeInfo::default(),
                r#"
                layout {
                    pane size=1 borderless=true
                    pane
                    pane size=1 borderless=true
                }
                "#,
            );
            for chrome in [0, 2] {
                tab.set_pane_selectable(PaneId::Terminal(TerminalId::new(chrome)), false);
            }
            assert_eq!(tab.viewport.borrow().y, 1);
            assert_eq!(tab.viewport.borrow().rows, 28);
            tab.new_pane(
                id,
                None,
                None,
                false,
                true,
                NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(100)),
                    y: Some(PercentOrFixed::Fixed(3)),
                    width: Some(PercentOrFixed::Fixed(200)),
                    height: Some(PercentOrFixed::Fixed(10)),
                    ..Default::default()
                })),
                Some(client),
                None,
            )
            .unwrap();
            tab.max_agent_width = limit;
            mark_agent(&mut tab, id);
            draw(&mut tab);

            let geom = tab.get_pane_with_id(id).unwrap().render_geom();
            let from = Position::new((geom.y + 1) as i32, geom.x as u16);
            let to = Position::new(if bottom { 21 } else { 1 }, 399);
            tab.handle_mouse_event(&MouseEvent::new_left_press_event(from), client)
                .unwrap();
            tab.handle_mouse_event(&MouseEvent::new_left_motion_event(to), client)
                .unwrap();
            tab.handle_mouse_event(&MouseEvent::new_left_release_event(to), client)
                .unwrap();
            draw(&mut tab);
            let width = if limit == 0 { 200 } else { 82 };
            let geom = tab.get_pane_with_id(id).unwrap().render_geom();
            assert_eq!((geom.x, geom.y), (400 - width, if bottom { 20 } else { 0 }));
            assert!(tab.floating_panes.has_panes_outside_viewport());

            for (cols, rows) in [(400, 29), (400, 29), (350, 28), (350, 28)] {
                tab.resize_whole_tab(Size { cols, rows }).unwrap();
                for _ in 0..2 {
                    draw(&mut tab);
                    let pane = tab.get_pane_with_id(id).unwrap();
                    let geom = pane.render_geom();
                    assert_eq!(
                        (geom.x, geom.y, geom.cols.as_usize()),
                        (cols - width, if bottom { rows - 10 } else { 0 }, width),
                        "bottom={bottom}, limit={limit}, cols={cols}, rows={rows}"
                    );
                    assert_eq!(pane.current_geom().cols.as_usize(), 200);
                }
            }
            tab.max_agent_width = 0;
            draw(&mut tab);
            let geom = tab.get_pane_with_id(id).unwrap().render_geom();
            assert_eq!((geom.x, geom.cols.as_usize()), (150, 200));
        }
    }
}

#[test]
fn centered_agent_movement_reaches_displayed_edges_without_losing_requested_width() {
    let client = ClientId::new(1);
    let id = PaneId::Terminal(TerminalId::new(2));
    for overlay in [false, true] {
        for keyboard in [false, true] {
            for limit in [0, 80] {
                let mut tab = create_new_tab(
                    Size {
                        cols: 400,
                        rows: 30,
                    },
                    ModeInfo::default(),
                );
                let coordinates = Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(100)),
                    y: Some(PercentOrFixed::Fixed(3)),
                    width: Some(PercentOrFixed::Fixed(200)),
                    height: Some(PercentOrFixed::Fixed(20)),
                    ..Default::default()
                });
                let placement = if overlay {
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(coordinates))
                } else {
                    NewPanePlacement::Floating(coordinates)
                };
                tab.new_pane(id, None, None, false, true, placement, Some(client), None)
                    .unwrap();
                tab.max_agent_width = limit;
                mark_agent(&mut tab, id);
                draw(&mut tab);
                let width = if limit == 0 { 200 } else { 82 };
                for right in [false, true, false] {
                    if keyboard {
                        for _ in 0..50 {
                            if right {
                                tab.move_pane_right(id);
                            } else {
                                tab.move_pane_left(id);
                            }
                            draw(&mut tab);
                        }
                    } else {
                        let geom = tab.get_pane_with_id(id).unwrap().render_geom();
                        let from = Position::new((geom.y + 1) as i32, geom.x as u16);
                        let to = Position::new((geom.y + 1) as i32, if right { 399 } else { 0 });
                        tab.handle_mouse_event(&MouseEvent::new_left_press_event(from), client)
                            .unwrap();
                        tab.handle_mouse_event(&MouseEvent::new_left_motion_event(to), client)
                            .unwrap();
                        tab.handle_mouse_event(&MouseEvent::new_left_release_event(to), client)
                            .unwrap();
                    }
                    for _ in 0..2 {
                        draw(&mut tab);
                        let pane = tab.get_pane_with_id(id).unwrap();
                        assert_eq!(
                            pane.render_geom().x,
                            if right { 400 - width } else { 0 },
                            "overlay={overlay}, keyboard={keyboard}, limit={limit}"
                        );
                        assert_eq!(pane.render_geom().cols.as_usize(), width);
                        assert_eq!(pane.current_geom().cols.as_usize(), 200);
                    }
                    if right {
                        for columns in [350, 400] {
                            tab.resize_whole_tab(Size {
                                cols: columns,
                                rows: 30,
                            })
                            .unwrap();
                            draw(&mut tab);
                            let pane = tab.get_pane_with_id(id).unwrap();
                            assert_eq!(
                                pane.render_geom().x,
                                columns - width,
                                "resize: overlay={overlay}, keyboard={keyboard}, limit={limit}"
                            );
                            assert_eq!(pane.current_geom().cols.as_usize(), 200);
                        }
                    }
                }
                tab.toggle_pane_fullscreen(id);
                draw(&mut tab);
                tab.unset_fullscreen();
                draw(&mut tab);
                assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom().x, 0);
                tab.resize_whole_tab(Size {
                    cols: 450,
                    rows: 30,
                })
                .unwrap();
                draw(&mut tab);
                assert_eq!(tab.get_pane_with_id(id).unwrap().render_geom().x, 0);
                tab.max_agent_width = 0;
                draw(&mut tab);
                let pane = tab.get_pane_with_id(id).unwrap();
                assert_eq!(pane.render_geom().x, 0);
                assert_eq!(pane.render_geom().cols.as_usize(), 200);
            }
        }
    }
}

#[test]
fn centered_agent_waiting_pulse_uses_packed_geometry() {
    for style in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let mut tab = create_new_tab_with_layout(
            Size {
                cols: 600,
                rows: 20,
            },
            ModeInfo::default(),
            r#"
            layout {
                pane split_direction="vertical" {
                    pane
                    pane
                    pane
                }
            }
            "#,
        );
        tab.set_pane_frames(style);
        let first = PaneId::Terminal(TerminalId::new(0));
        let waiting = PaneId::Terminal(TerminalId::new(1));
        tab.max_agent_width = 80;
        mark_agent(&mut tab, first);
        mark_agent(&mut tab, waiting);
        let mut info = tab
            .get_pane_with_id(waiting)
            .unwrap()
            .agent_info()
            .unwrap()
            .clone();
        info.state = AgentState::Waiting;
        info.wait_needs_a_person = true;
        tab.set_pane_agent(waiting, Some(info));
        draw(&mut tab);
        let pane = tab.get_pane_with_id(waiting).unwrap();
        let displayed = pane.render_geom();
        let original = pane.current_geom();
        assert!(displayed.x + displayed.cols.as_usize() < original.x);
        assert!(tab.has_visible_waiting_pulse(None));
        let cover = PaneId::Terminal(TerminalId::new(3));
        tab.new_pane(
            cover,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(original.x)),
                    y: Some(PercentOrFixed::Fixed(0)),
                    width: Some(PercentOrFixed::Fixed(original.cols.as_usize())),
                    height: Some(PercentOrFixed::Fixed(20)),
                    ..Default::default()
                },
            ))),
            Some(ClientId::new(1)),
            None,
        )
        .unwrap();
        draw(&mut tab);
        assert!(
            tab.has_visible_waiting_pulse(None),
            "{style:?}: original slot is covered, displayed border is visible"
        );
        tab.overlay_panes
            .change_pane_coordinates(
                cover,
                FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(displayed.x.saturating_sub(1))),
                    y: Some(PercentOrFixed::Fixed(0)),
                    width: Some(PercentOrFixed::Fixed(displayed.cols.as_usize() + 2)),
                    height: Some(PercentOrFixed::Fixed(20)),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        draw(&mut tab);
        assert!(
            !tab.has_visible_waiting_pulse(None),
            "{style:?}: displayed border is covered"
        );
    }
}

#[test]
fn centered_agent_multipane_movement_keeps_requested_size() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        for keyboard in [false, true] {
            let mut tab = create_new_tab(
                Size {
                    cols: 600,
                    rows: 30,
                },
                ModeInfo::default(),
            );
            let client = ClientId::new(1);
            let ids = [
                PaneId::Terminal(TerminalId::new(2)),
                PaneId::Terminal(TerminalId::new(3)),
            ];
            for id in ids {
                tab.new_pane(
                    id,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                    Some(client),
                    None,
                )
                .unwrap();
            }
            tab.create_multipane(
                ids,
                MultipaneSplit {
                    direction,
                    ratio: 5000,
                },
                0,
                OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Fixed(100)),
                    y: Some(PercentOrFixed::Fixed(3)),
                    width: Some(PercentOrFixed::Fixed(300)),
                    height: Some(PercentOrFixed::Fixed(20)),
                    ..Default::default()
                })),
            )
            .unwrap();
            tab.max_agent_width = 80;
            for id in ids {
                mark_agent(&mut tab, id);
            }
            draw(&mut tab);
            let width = tab
                .overlay_panes
                .outer_geom(ids[0])
                .unwrap()
                .cols
                .as_usize();
            assert!(width < 300);
            let requested = ids.map(|id| tab.get_pane_with_id(id).unwrap().current_geom().cols);
            for right in [false, true, false] {
                if keyboard {
                    for _ in 0..60 {
                        if right {
                            tab.move_pane_right(ids[1]);
                        } else {
                            tab.move_pane_left(ids[1]);
                        }
                    }
                } else {
                    let geom = tab.overlay_panes.outer_geom(ids[0]).unwrap();
                    let from = Position::new((geom.y + 1) as i32, geom.x as u16);
                    let to = Position::new((geom.y + 1) as i32, if right { 599 } else { 0 });
                    tab.handle_mouse_event(&MouseEvent::new_left_press_event(from), client)
                        .unwrap();
                    tab.handle_mouse_event(&MouseEvent::new_left_motion_event(to), client)
                        .unwrap();
                    tab.handle_mouse_event(&MouseEvent::new_left_release_event(to), client)
                        .unwrap();
                }
                draw(&mut tab);
                let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
                assert_eq!(outer.x, if right { 600 - width } else { 0 });
                assert_eq!(outer.cols.as_usize(), width);
                assert_eq!(
                    ids.map(|id| tab.get_pane_with_id(id).unwrap().current_geom().cols),
                    requested
                );
            }
            tab.max_agent_width = 0;
            draw(&mut tab);
            let outer = tab.overlay_panes.outer_geom(ids[0]).unwrap();
            assert_eq!(outer.x, 0);
            assert_eq!(outer.cols.as_usize(), 300);
        }
    }
}

#[test]
fn centered_agent_waiting_pulse_has_no_boundary_in_the_packed_margin() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 600,
            rows: 20,
        },
        ModeInfo::default(),
        r#"
        layout {
            pane split_direction="vertical" {
                pane
                pane
                pane
            }
        }
    "#,
    );
    tab.set_pane_frames(PaneFrameStyle::None);
    tab.max_agent_width = 80;
    for i in 0..3 {
        mark_agent(&mut tab, PaneId::Terminal(TerminalId::new(i)));
    }
    let waiting = PaneId::Terminal(TerminalId::new(0));
    let mut info = tab
        .get_pane_with_id(waiting)
        .unwrap()
        .agent_info()
        .unwrap()
        .clone();
    info.state = AgentState::Waiting;
    info.wait_needs_a_person = true;
    tab.set_pane_agent(waiting, Some(info));
    draw(&mut tab);
    let geom = tab.get_pane_with_id(waiting).unwrap().render_geom();
    assert!(geom.x > 0);
    assert!(tab.has_visible_waiting_pulse(None));
    let x = geom.x + geom.cols.as_usize() - 1;
    tab.new_pane(
        PaneId::Terminal(TerminalId::new(3)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(x)),
                y: Some(PercentOrFixed::Fixed(0)),
                width: Some(PercentOrFixed::Fixed(600 - x)),
                height: Some(PercentOrFixed::Fixed(20)),
                ..Default::default()
            },
        ))),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    draw(&mut tab);
    assert!(
        !tab.has_visible_waiting_pulse(None),
        "the only shared boundary is covered; the left margin has no border"
    );
}
