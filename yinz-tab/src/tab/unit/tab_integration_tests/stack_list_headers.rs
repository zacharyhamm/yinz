use super::*;
use yinz_terminal::panes::terminal_character::AnsiCode;
use yinz_types::style::TerminalTheme;

const CLIENT: ClientId = ClientId::new(1);

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

fn stack_tab(size: Size, floating: bool, frames: PaneFrameStyle) -> Tab {
    let mut tab = create_new_tab_with_stacked_pane_list(size, ModeInfo::default(), true, None);
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
        for (column, character) in expected.into_iter().enumerate() {
            let x = rect.x + column;
            let y = rect.y + rank;
            let cell = grid.get_character_at(x, y).unwrap();
            if shared_divider && column == rect.cols.as_usize() - 1 {
                assert_eq!(cell.character, '│', "shared divider at ({x}, {y})");
                continue;
            }
            assert_eq!(cell.character, character, "character at ({x}, {y})");
            assert_eq!(
                cell.styles.background,
                Some(background),
                "background at ({x}, {y})"
            );
            let selected_text = selected && (entry_x + 4..entry_x + 4 + label.len()).contains(&x);
            let expected_foreground = if selected_text {
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
