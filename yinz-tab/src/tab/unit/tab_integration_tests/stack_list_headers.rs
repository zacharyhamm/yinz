use super::*;
use yinz_terminal::panes::terminal_character::AnsiCode;
use yinz_types::style::TerminalTheme;

const CLIENT: ClientId = ClientId::new(1);

#[test]
fn selectable_panes_except_target_matches_stack_list_promotion() {
    for floating in [false, true] {
        for rank in 0..3 {
            let mut tab = stack_tab(Size { cols: 80, rows: 24 }, floating, PaneFrameStyle::Full);
            let members = tab.stack_lists.values().next().unwrap().members.clone();
            let target = members[rank];
            let promoted = members[if rank == 2 { 1 } else { rank + 1 }];
            tab.focus_pane_with_id(target, false, false, CLIENT)
                .unwrap();
            for selectable in [false, true] {
                tab.get_pane_with_id_mut(promoted)
                    .unwrap()
                    .set_selectable(selectable);
                assert_eq!(tab.has_selectable_panes_except(target), selectable);
            }
            tab.suppress_pane(target, None);
            assert!(tab.get_pane_info(target).unwrap().is_suppressed);
            assert!(!tab.get_pane_info(promoted).unwrap().is_suppressed);
            assert_eq!(tab.get_pane_info(promoted).unwrap().is_floating, floating);
            assert!(tab.has_selectable_panes());
            assert_eq!(tab.get_all_pane_ids().len(), members.len());
            tab.assert_invariants();
        }
    }
}

#[test]
fn stack_list_focus_cycles_in_both_directions_without_neighbors() {
    for floating in [false, true] {
        let mut tab = stack_tab(Size { cols: 80, rows: 24 }, floating, PaneFrameStyle::Full);
        let members = tab.stack_lists.values().next().unwrap().members.clone();
        tab.focus_pane_with_id(members[0], false, false, CLIENT)
            .unwrap();
        for expected in [members[2], members[1], members[0], members[2]] {
            assert!(tab.move_focus_up(CLIENT).unwrap());
            assert_eq!(tab.get_active_pane_id(CLIENT), Some(expected));
            assert_eq!(tab.stack_lists.values().next().unwrap().visible, expected);
        }
        for expected in [members[0], members[1], members[2], members[0]] {
            assert!(tab.move_focus_down(CLIENT).unwrap());
            assert_eq!(tab.get_active_pane_id(CLIENT), Some(expected));
            assert_eq!(tab.stack_lists.values().next().unwrap().visible, expected);
        }
    }
}

#[test]
fn stack_list_focus_prefers_neighbors_above_and_below_over_wrapping() {
    for floating in [false, true] {
        let layout = r#"
            layout {
                pane
                pane stacked=true {
                    pane
                    pane
                    pane
                }
                pane
            }
        "#;
        let (layout, _) = yinz_config::kdl::layout::from_kdl(layout, None, None, None)
            .unwrap()
            .template
            .unwrap();
        let ids = (1..=5).map(|id| (TerminalId::new(id), None)).collect();
        let mut tab = create_new_tab_with_stacked_pane_list(
            Size { cols: 80, rows: 60 },
            ModeInfo::default(),
            true,
            Some((layout, ids)),
        );
        tab.sync_stacked_pane_list_mode();
        let list = tab.stack_lists.values().next().unwrap();
        let members = list.members.clone();
        let above = PaneId::Terminal(TerminalId::new(1));
        let below = PaneId::Terminal(TerminalId::new(3));
        if floating {
            for id in [above, list.visible, below] {
                let pane = tab.tiled_panes.extract_pane(id).unwrap();
                let geom = pane.position_and_size();
                tab.add_floating_pane(
                    pane,
                    id,
                    Some(FloatingPaneCoordinates {
                        x: Some(PercentOrFixed::Fixed(geom.x)),
                        y: Some(PercentOrFixed::Fixed(geom.y)),
                        width: Some(PercentOrFixed::Fixed(geom.cols.as_usize())),
                        height: Some(PercentOrFixed::Fixed(geom.rows.as_usize())),
                        ..Default::default()
                    }),
                    true,
                    Some(CLIENT),
                )
                .unwrap();
            }
            tab.toggle_floating_panes(Some(CLIENT), None, None).unwrap();
        }
        tab.focus_pane_with_id(members[1], false, false, CLIENT)
            .unwrap();
        assert!(tab.move_focus_up(CLIENT).unwrap());
        assert_eq!(tab.get_active_pane_id(CLIENT), Some(members[0]));
        assert!(tab.move_focus_up(CLIENT).unwrap());
        assert_eq!(tab.get_active_pane_id(CLIENT), Some(above));

        tab.focus_pane_with_id(members[1], false, false, CLIENT)
            .unwrap();
        assert!(tab.move_focus_down(CLIENT).unwrap());
        assert_eq!(tab.get_active_pane_id(CLIENT), Some(members[2]));
        assert!(tab.move_focus_down(CLIENT).unwrap());
        assert_eq!(tab.get_active_pane_id(CLIENT), Some(below));
    }
}

#[test]
fn same_worktree_stack_labels_follow_focus_in_tiled_and_floating_lists() {
    let size = Size { cols: 80, rows: 24 };
    for floating in [false, true] {
        let mut tab = stack_tab(size, floating, PaneFrameStyle::Full);
        let members = tab.stack_lists.values().next().unwrap().members.clone();
        for id in &members {
            tab.agent_worktrees.insert(*id, PathBuf::from("/checkout"));
        }
        let mut grid = host_grid(size);
        for focused in &members {
            tab.focus_pane_with_id(*focused, false, false, CLIENT)
                .unwrap();
            let mut output = Output::default();
            tab.render(&mut output, None).unwrap();
            let serialized = output.serialize().unwrap();
            let mut parser = vte::Parser::new();
            parser.advance(&mut grid, serialized[&CLIENT].as_bytes());
            let text = format!("{grid:?}");
            for id in &members {
                let PaneId::Terminal(id) = id else {
                    unreachable!()
                };
                assert_eq!(
                    text.contains(&format!("👯 [{id}]")),
                    PaneId::Terminal(*id) != *focused,
                    "{text}"
                );
            }
            assert_eq!(text.matches('👯').count(), members.len() - 1);
        }
        tab.agent_worktrees.clear();
        let mut output = Output::default();
        tab.render(&mut output, None).unwrap();
        let serialized = output.serialize().unwrap();
        vte::Parser::new().advance(&mut grid, serialized[&CLIENT].as_bytes());
        assert!(!format!("{grid:?}").contains('👯'));
    }
}

#[test]
fn floating_stack_headers_survive_unchanged_frame_repaints() {
    let size = Size { cols: 80, rows: 24 };
    for frames in [
        PaneFrameStyle::Full,
        PaneFrameStyle::Titles,
        PaneFrameStyle::None,
    ] {
        let mut tab = stack_tab(size, true, frames);
        let members = tab.stack_lists.values().next().unwrap().members.clone();
        let mut grid = host_grid(size);
        for member in members {
            tab.focus_pane_with_id(member, false, false, CLIENT)
                .unwrap();
            render_into(&mut tab, &mut grid);
            assert_header_cells(&tab, &grid, false);
            // Unchanged hidden headers may be cached while the visible pane draws its frame again.
            render_into(&mut tab, &mut grid);
            assert_header_cells(&tab, &grid, false);
        }
    }
}

#[test]
fn full_client_repaints_restore_cached_stack_headers_without_invalidating_other_clients() {
    let size = Size {
        cols: 120,
        rows: 30,
    };
    let other = ClientId::new(2);
    for floating in [false, true] {
        let mut tab = stack_tab(size, floating, PaneFrameStyle::Full);
        tab.connected_clients_in_app.borrow_mut().insert(other);
        tab.add_client(other, None).unwrap();
        let list = tab.stack_lists.values().next().unwrap().clone();
        for (rank, member) in list.members.iter().enumerate() {
            tab.get_pane_with_id_mut(*member)
                .unwrap()
                .set_title(format!("stack-caption-{rank}"));
        }
        // Warm both clients' caches before clearing only one client's display.
        let mut output = Output::default();
        tab.render(&mut output, None).unwrap();
        tab.prepare_full_render_for_client(CLIENT);

        let mut output = Output::default();
        tab.render(&mut output, Some(other)).unwrap();
        let serialized = output.serialize().unwrap();
        for (rank, member) in list.members.iter().enumerate() {
            if *member != list.visible {
                assert!(!serialized[&other].contains(&format!("stack-caption-{rank}")));
            }
        }

        let mut output = Output::default();
        tab.render(&mut output, Some(CLIENT)).unwrap();
        let serialized = output.serialize().unwrap();
        assert!(!serialized.contains_key(&other));
        // A fresh host grid models a display clear: cached text cannot hide missing output.
        let mut grid = host_grid(size);
        vte::Parser::new().advance(&mut grid, serialized[&CLIENT].as_bytes());
        let text = format!("{grid:?}");
        for rank in 0..list.members.len() {
            assert!(text.contains(&format!("stack-caption-{rank}")), "{text}");
        }
    }
}

#[test]
fn targeted_stack_headers_preserve_all_clients_focus_and_emphasis() {
    let size = Size {
        cols: 160,
        rows: 40,
    };
    let other = ClientId::new(2);
    for floating in [false, true] {
        let mut tab = stack_tab(size, floating, PaneFrameStyle::Full);
        tab.update_session_is_mirrored(false);
        tab.connected_clients_in_app.borrow_mut().insert(other);
        tab.add_client(other, None).unwrap();
        let visible = tab.stack_lists.values().next().unwrap().visible;
        tab.focus_pane_with_id(visible, false, false, other)
            .unwrap();
        let mut focus_targets = vec![visible];
        // Floating panes share focus; tiled panes also allow only the other client on the stack.
        if !floating {
            let neighbor = PaneId::Terminal(TerminalId::new(4));
            tab.new_pane(
                neighbor,
                None,
                None,
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: Some(Direction::Down),
                    borderless: None,
                },
                Some(CLIENT),
                None,
            )
            .unwrap();
            focus_targets.push(neighbor);
        }
        for target in focus_targets {
            let shared_focus = target == visible;
            tab.focus_pane_with_id(target, false, false, CLIENT)
                .unwrap();
            tab.set_force_render();
            let mut output = Output::default();
            tab.render(&mut output, None).unwrap();
            let serialized = output.serialize().unwrap();
            let mut regular_grid = host_grid(size);
            vte::Parser::new().advance(&mut regular_grid, serialized[&CLIENT].as_bytes());
            if shared_focus {
                assert!(format!("{regular_grid:?}").contains("MY FOCUS AND:"));
            } else {
                assert!(format!("{regular_grid:?}").contains("FOCUSED USER:"));
            }

            tab.prepare_full_render_for_client(CLIENT);
            let mut output = Output::default();
            tab.render(&mut output, Some(CLIENT)).unwrap();
            let serialized = output.serialize().unwrap();
            assert!(!serialized.contains_key(&other));
            let mut targeted_grid = host_grid(size);
            vte::Parser::new().advance(&mut targeted_grid, serialized[&CLIENT].as_bytes());
            let list = tab.stack_lists.values().next().unwrap();
            let rect = tab.get_pane_with_id(visible).unwrap().render_geom();
            for y in rect.y..rect.y + list.members.len() {
                for x in rect.x..rect.x + rect.cols.as_usize() {
                    assert_eq!(
                        targeted_grid.get_character_at(x, y),
                        regular_grid.get_character_at(x, y),
                        "floating={floating}, shared_focus={shared_focus}, ({x}, {y})"
                    );
                }
            }
        }
    }
}

fn stack_tab(size: Size, floating: bool, frames: PaneFrameStyle) -> Tab {
    let mut tab = create_new_tab_with_stacked_pane_list(size, ModeInfo::default(), true, None);
    // The default test palette includes unreadable ribbons. Use readable pairs so
    // repaint/geometry checks exercise declared colors rather than contrast fallback.
    let mut colors = tab.style.colors;
    colors.ribbon_selected.base = yinz_types::data::PaletteColor::Rgb((255, 255, 255));
    colors.ribbon_selected.background = yinz_types::data::PaletteColor::Rgb((30, 60, 90));
    colors.ribbon_unselected.base = yinz_types::data::PaletteColor::Rgb((255, 255, 255));
    colors.ribbon_unselected.background = yinz_types::data::PaletteColor::Rgb((25, 25, 25));
    tab.update_theme(colors);
    tab.set_pane_frames(frames);
    for id in 2..=3 {
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(CLIENT),
            None,
        )
        .unwrap();
    }
    tab.sync_stacked_pane_list_mode();
    assert_eq!(tab.stack_lists.len(), 1);
    if floating {
        // Move the visible pane with its ledger intact, as a restored floating stack does.
        let visible = tab.stack_lists.values().next().unwrap().visible;
        let pane = tab.tiled_panes.extract_pane(visible).unwrap();
        tab.add_floating_pane(pane, visible, None, true, Some(CLIENT))
            .unwrap();
        tab.toggle_floating_panes(Some(CLIENT), None, None).unwrap();
    }
    tab
}

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
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    vte::Parser::new().advance(grid, output.serialize().unwrap()[&CLIENT].as_bytes());
}

fn assert_header_cells(tab: &Tab, grid: &Grid, shared_divider: bool) {
    let list = tab.stack_lists.values().next().unwrap();
    let pane = tab.get_pane_with_id(list.visible).unwrap();
    let rect = pane.position_and_size();
    let offset = pane.get_content_offset();
    let foreground = tab
        .style
        .terminal
        .map_or(AnsiCode::Reset, |theme| AnsiCode::RgbCode(theme.foreground));
    let background = tab
        .style
        .terminal
        .map_or(AnsiCode::Reset, |theme| AnsiCode::RgbCode(theme.background));
    let labels: Vec<_> = list
        .members
        .iter()
        .map(|id| {
            tab.get_pane_with_id(*id)
                .unwrap()
                .stack_list_entry_label(false)
        })
        .collect();
    let width = labels.iter().map(String::len).max().unwrap();
    let entry_x =
        rect.x + offset.left + (rect.cols.as_usize() - offset.left - offset.right - width - 6) / 2;
    for (rank, label) in labels.iter().map(String::as_str).chain([""]).enumerate() {
        let mut expected = vec![' '; rect.cols.as_usize()];
        let selected = list.members.get(rank) == Some(&list.visible);
        if rank < list.members.len() {
            let entry = format!("{} │ {:width$} │", if selected { '>' } else { ' ' }, label);
            for (i, character) in entry.chars().enumerate() {
                expected[entry_x - rect.x + i] = character;
            }
        }
        if selected {
            // Both columns of a wide glyph resolve to the same terminal character.
            expected[offset.left + 1] = '👇';
            expected[offset.left + 2] = '👇';
        }
        for (column, character) in expected.into_iter().enumerate() {
            let x = rect.x + column;
            let y = rect.y + rank;
            let cell = grid.get_character_at(x, y).unwrap();
            if shared_divider && column == rect.cols.as_usize() - 1 {
                assert_eq!(cell.character, '│', "shared divider at ({x}, {y})");
                continue;
            }
            assert_eq!(cell.character, character, "character at ({x}, {y})");
            let ribbon = (tab.pane_frame_style.draws_titles()
                && rank < list.members.len()
                && (offset.left..rect.cols.as_usize() - offset.right).contains(&column))
            .then_some(if selected {
                tab.style.colors.ribbon_selected
            } else {
                tab.style.colors.ribbon_unselected
            });
            assert_eq!(
                cell.styles.background,
                Some(ribbon.map_or(background, |colors| colors.background.into())),
                "background at ({x}, {y})"
            );
            let selected_text = selected && (entry_x + 4..entry_x + 4 + label.len()).contains(&x);
            let focus_marker = selected && (offset.left..offset.left + 4).contains(&column);
            let expected_foreground = if let Some(colors) = ribbon {
                colors.base.into()
            } else if selected_text || focus_marker {
                tab.style.colors.frame_selected.base.into()
            } else {
                foreground
            };
            assert_eq!(
                cell.styles.foreground,
                Some(expected_foreground),
                "foreground at ({x}, {y})"
            );
        }
    }
}

fn check_header_repaints(theme: Option<TerminalTheme>) {
    for floating in [false, true] {
        for frames in [
            PaneFrameStyle::Full,
            PaneFrameStyle::Titles,
            PaneFrameStyle::None,
        ] {
            let size = Size {
                cols: 100,
                rows: 30,
            };
            let mut tab = stack_tab(size, floating, frames);
            tab.update_terminal_theme(theme);
            let mut grid = host_grid(size);
            render_into(&mut tab, &mut grid);
            assert_header_cells(&tab, &grid, false);

            assert!(
                tab.focus_hidden_stack_list_member(PaneId::Terminal(TerminalId::new(1)), CLIENT)
            );
            render_into(&mut tab, &mut grid);
            assert_header_cells(&tab, &grid, false);

            for size in [
                Size { cols: 80, rows: 24 },
                Size {
                    cols: 120,
                    rows: 40,
                },
            ] {
                tab.resize_whole_tab(size).unwrap();
                grid.change_size(size.rows, size.cols);
                render_into(&mut tab, &mut grid);
                assert_header_cells(&tab, &grid, false);
            }
        }
    }
}

#[test]
fn light_stack_list_headers_paint_every_cell_after_selection_and_resize() {
    check_header_repaints(Some(TerminalTheme {
        foreground: (40, 45, 50),
        background: (245, 240, 230),
        ansi: [(0, 0, 0); 16],
    }));
}

#[test]
fn dark_stack_list_headers_paint_every_cell_after_selection_and_resize() {
    check_header_repaints(Some(TerminalTheme {
        foreground: (220, 225, 230),
        background: (20, 25, 30),
        ansi: [(0, 0, 0); 16],
    }));
}

#[test]
fn unthemed_stack_list_headers_preserve_host_defaults_after_selection_and_resize() {
    check_header_repaints(None);
}

#[test]
fn stack_list_headers_preserve_shared_divider_after_selection_and_resize() {
    for frames in [PaneFrameStyle::Titles, PaneFrameStyle::None] {
        let size = Size {
            cols: 100,
            rows: 30,
        };
        let mut tab = stack_tab(size, false, frames);
        let neighbor = PaneId::Terminal(TerminalId::new(4));
        tab.new_pane(
            neighbor,
            None,
            None,
            false,
            false,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Right),
                borderless: None,
            },
            Some(CLIENT),
            None,
        )
        .unwrap();
        tab.update_terminal_theme(Some(TerminalTheme {
            foreground: (40, 45, 50),
            background: (245, 240, 230),
            ansi: [(0, 0, 0); 16],
        }));
        let mut grid = host_grid(size);
        render_into(&mut tab, &mut grid);
        let list = tab.stack_lists.values().next().unwrap();
        let pane = tab.get_pane_with_id(list.visible).unwrap();
        let rect = pane.position_and_size();
        assert_eq!(pane.get_content_offset().right, 1);
        assert_eq!(
            tab.get_pane_with_id(neighbor)
                .unwrap()
                .position_and_size()
                .x,
            rect.x + rect.cols.as_usize()
        );
        assert_header_cells(&tab, &grid, true);

        assert!(tab.focus_hidden_stack_list_member(PaneId::Terminal(TerminalId::new(1)), CLIENT));
        render_into(&mut tab, &mut grid);
        assert_header_cells(&tab, &grid, true);

        for size in [
            Size { cols: 80, rows: 24 },
            Size {
                cols: 120,
                rows: 40,
            },
        ] {
            tab.resize_whole_tab(size).unwrap();
            grid.change_size(size.rows, size.cols);
            render_into(&mut tab, &mut grid);
            assert_header_cells(&tab, &grid, true);
        }
    }
}

#[test]
fn stack_list_header_padding_stays_below_higher_floating_panes() {
    for floating in [false, true] {
        let size = Size {
            cols: 100,
            rows: 30,
        };
        let mut tab = stack_tab(size, floating, PaneFrameStyle::Full);
        tab.update_terminal_theme(Some(TerminalTheme {
            foreground: (220, 225, 230),
            background: (20, 25, 30),
            ansi: [(0, 0, 0); 16],
        }));
        let mut grid = host_grid(size);
        render_into(&mut tab, &mut grid);
        let list = tab.stack_lists.values().next().unwrap();
        let rect = tab
            .get_pane_with_id(list.visible)
            .unwrap()
            .position_and_size();
        let header_rows = list.members.len() + 1;
        let terminal = TerminalId::new(4);
        tab.new_pane(
            PaneId::Terminal(terminal),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(rect.x)),
                y: Some(PercentOrFixed::Fixed(rect.y)),
                width: Some(PercentOrFixed::Fixed(rect.cols.as_usize())),
                height: Some(PercentOrFixed::Fixed(8)),
                borderless: Some(true),
                ..Default::default()
            })),
            Some(CLIENT),
            None,
        )
        .unwrap();
        let mut bytes = String::from("\x1b[?7l\x1b[48;2;1;2;3m");
        for row in 1..=header_rows {
            bytes.push_str(&format!(
                "\x1b[{row};1H{}",
                "X".repeat(rect.cols.as_usize())
            ));
        }
        tab.handle_pty_bytes(terminal, bytes.into_bytes()).unwrap();
        render_into(&mut tab, &mut grid);
        for y in rect.y..rect.y + header_rows {
            for x in rect.x..rect.x + rect.cols.as_usize() {
                let cell = grid.get_character_at(x, y).unwrap();
                assert_eq!(cell.character, 'X', "higher floating pane at ({x}, {y})");
                assert_eq!(cell.styles.background, Some(AnsiCode::RgbCode((1, 2, 3))));
            }
        }
    }
}

#[test]
fn catppuccin_stack_list_headers_use_ribbons_through_the_tab_caller() {
    let themes = yinz_config::theme::Themes::from_string(
        include_str!("../../../../../yinz-config/assets/themes/catppuccin-macchiato.kdl"),
        false,
    )
    .unwrap();
    let theme = themes.get_theme("catppuccin-macchiato").unwrap();
    for floating in [false, true] {
        let size = Size {
            cols: 100,
            rows: 30,
        };
        let mut tab = stack_tab(size, floating, PaneFrameStyle::Titles);
        tab.update_theme(theme.palette);
        tab.update_terminal_theme(theme.terminal);
        let mut grid = host_grid(size);
        render_into(&mut tab, &mut grid);
        assert_header_cells(&tab, &grid, false);
        assert!(tab.focus_hidden_stack_list_member(PaneId::Terminal(TerminalId::new(1)), CLIENT));
        render_into(&mut tab, &mut grid);
        assert_header_cells(&tab, &grid, false);
    }
}

#[test]
fn floating_titles_keep_frame_colors_until_fullscreen() {
    let themes = yinz_config::theme::Themes::from_string(
        include_str!("../../../../../yinz-config/assets/themes/catppuccin-macchiato.kdl"),
        false,
    )
    .unwrap();
    let theme = themes.get_theme("catppuccin-macchiato").unwrap();
    let size = Size {
        cols: 100,
        rows: 30,
    };
    let mut tab = setup_tab_with_focused_floating_pane(size, CLIENT, PaneFrameStyle::Titles);
    tab.update_theme(theme.palette);
    tab.update_terminal_theme(theme.terminal);
    tab.handle_pty_bytes(TerminalId::new(2), b"\x1b]2;color-test-pane\x07".to_vec())
        .unwrap();
    let mut grid = host_grid(size);
    for fullscreen in [false, true] {
        if fullscreen {
            tab.toggle_active_pane_fullscreen(CLIENT);
        }
        render_into(&mut tab, &mut grid);
        let id = tab.floating_panes.active_pane_id(CLIENT).unwrap();
        let rect = tab.get_pane_with_id(id).unwrap().render_geom();
        let row: Vec<_> = (rect.x..rect.x + rect.cols.as_usize())
            .map(|x| grid.get_character_at(x, rect.y).unwrap())
            .collect();
        let text: String = row.iter().map(|cell| cell.character).collect();
        let title_byte = text.find("color-test-pane").expect("floating pane title");
        let title_column = text[..title_byte].chars().count();
        let cell = &row[title_column];
        let (foreground, background) = if fullscreen {
            (
                theme.palette.ribbon_selected.base.into(),
                theme.palette.ribbon_selected.background.into(),
            )
        } else {
            (
                theme.palette.frame_selected.base.into(),
                AnsiCode::RgbCode(theme.terminal.unwrap().background),
            )
        };
        assert_eq!(cell.styles.foreground, Some(foreground));
        assert_eq!(cell.styles.background, Some(background));
    }
}
