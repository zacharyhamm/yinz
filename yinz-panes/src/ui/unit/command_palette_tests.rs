use super::*;

fn key(palette: &mut CommandPalette, key: BareKey) {
    assert_eq!(palette.handle_key(KeyWithModifier::new(key)), Outcome::None);
}

fn entry(id: &str, label: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        label: label.into(),
        description: String::new(),
        shortcut: String::new(),
        aliases: vec![],
        kind,
    }
}

fn palette() -> CommandPalette {
    let mut palette = CommandPalette::default();
    let mut pane = entry("pane", "日本語 terminal", EntryKind::Pane);
    pane.aliases.push("console".into());
    palette.set_entries(vec![
        entry("new", "New pane", EntryKind::Command),
        entry("tab", "New tab", EntryKind::Command),
        pane,
    ]);
    palette
}

fn ids(palette: &CommandPalette) -> Vec<&str> {
    palette
        .matches
        .iter()
        .map(|index| palette.entries[*index].id.as_str())
        .collect()
}

fn rendered_lines(palette: &CommandPalette, layout: &CommandPaletteLayout) -> Vec<String> {
    palette
        .chunks(layout, &Style::default())
        .iter()
        .map(|chunk| {
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect()
        })
        .collect()
}

#[test]
fn picker_headings_remain_visible_with_no_matches() {
    for (kind, label) in [
        (EntryKind::Pane, "Focus pane"),
        (EntryKind::Tab, "Switch tab"),
        (EntryKind::Session, "Switch session"),
    ] {
        let mut palette = palette();
        palette.open_picker(kind);
        for query in ["", "missing"] {
            palette.insert_text(query);
            let layout = palette.layout(Viewport {
                cols: 80,
                rows: 24,
                ..Default::default()
            });
            let lines = rendered_lines(&palette, &layout);
            assert!(lines[layout.picker_heading_y.unwrap() - layout.rect.y].contains(label));
            if query == "missing" {
                assert!(lines.iter().any(|line| line.contains("No matches")));
            }
            let chunks = palette.chunks(&layout, &Style::default());
            assert_eq!(
                chunks[layout.picker_heading_y.unwrap() - layout.rect.y].terminal_characters
                    [layout.border]
                    .styles
                    .dim,
                Some(AnsiCode::On)
            );
        }
        key(&mut palette, BareKey::Esc);
        assert!(palette.picker.is_none());
        assert_eq!(palette.input.get_text(), "");
    }
}

#[test]
fn grouped_results_preserve_order_and_share_scrolled_click_geometry() {
    let mut palette = CommandPalette::default();
    palette.set_entries(vec![
        entry("pane1", "match one", EntryKind::Pane),
        entry("pane2", "match two", EntryKind::Pane),
        entry("tab", "match three", EntryKind::Tab),
        entry("pane3", "match four", EntryKind::Pane),
        entry("command", "match five", EntryKind::Command),
        entry("session", "match six", EntryKind::Session),
    ]);
    palette.insert_text("match");
    assert_eq!(
        ids(&palette),
        ["pane1", "pane2", "tab", "pane3", "command", "session"]
    );
    let surface = Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    for selected in 0..palette.matches.len() {
        assert_eq!(palette.selected, selected);
        let layout = palette.layout(surface);
        let lines = rendered_lines(&palette, &layout);
        let mut parent = None;
        let mut visible = Vec::new();
        for (row, result) in layout.result_rows.iter().enumerate() {
            match result {
                ResultRow::Heading(label) => {
                    parent = Some(label.as_str());
                    assert!(lines[layout.options_y + row - layout.rect.y].contains(label));
                    assert!(matches!(
                        layout.result_rows.get(row + 1),
                        Some(ResultRow::Entry(_))
                    ));
                    palette.click(
                        &layout,
                        Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
                    );
                    assert_eq!(palette.selected, selected);
                },
                ResultRow::Status(_, _) => panic!("native entries have no plugin status"),
                ResultRow::Entry(index) => {
                    let kind = palette.entries[palette.matches[*index]].kind;
                    if let Some(label) = kind.parent_label() {
                        assert_eq!(parent, Some(label));
                    } else {
                        parent = None;
                    }
                    visible.push(*index);
                },
            }
        }
        assert!(visible.contains(&selected));
        assert!(visible.windows(2).all(|pair| pair[1] == pair[0] + 1));
        for (row, result) in layout.result_rows.iter().enumerate() {
            if let ResultRow::Entry(index) = result {
                palette.click(
                    &layout,
                    Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
                );
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate(palette.entries[palette.matches[*index]].id.clone())
                );
            }
        }
        palette.selected = selected;
        key(&mut palette, BareKey::Down);
    }
}

#[test]
fn headings_fit_small_surfaces_and_keep_the_selected_result_visible() {
    let mut palette = palette();
    palette.selected = 2;
    for picker in [None, Some(EntryKind::Pane)] {
        if let Some(kind) = picker {
            palette.open_picker(kind);
        }
        for rows in 0..12 {
            for cols in 0..20 {
                let layout = palette.layout(Viewport {
                    x: 2,
                    y: 3,
                    cols,
                    rows,
                });
                let chunks = palette.chunks(&layout, &Style::default());
                for chunk in chunks {
                    assert_eq!(chunk.width(), layout.rect.cols);
                    assert!(chunk.y < 3 + rows);
                }
                if layout.options_rows > 0 {
                    assert!(layout.result_rows.iter().any(
                        |row| matches!(row, ResultRow::Entry(index) if *index == palette.selected)
                    ));
                }
                if let Some((x, y)) = palette.cursor(&layout) {
                    assert!(layout.rect.contains(x, y));
                }
            }
        }
    }
}

#[test]
fn fuzzy_ranking_aliases_unicode_and_empty_queries() {
    let mut palette = palette();
    assert_eq!(ids(&palette), ["new", "tab", "pane"]);
    palette.insert_text("np");
    assert_eq!(ids(&palette), ["new"]);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("new".into())
    );
    palette.input.set_text("console".into());
    palette.rematch();
    assert_eq!(ids(&palette), ["pane"]);
    palette.input.set_text("日語".into());
    palette.rematch();
    assert_eq!(ids(&palette), ["pane"]);
    palette.input.set_text("unmatched".into());
    palette.rematch();
    assert!(ids(&palette).is_empty());
    key(&mut palette, BareKey::Enter);
    palette.set_entries(vec![
        entry("gap", "n--------p", EntryKind::Command),
        entry("exact", "np", EntryKind::Command),
        entry("tie", "np", EntryKind::Command),
    ]);
    palette.input.set_text("np".into());
    palette.rematch();
    assert_eq!(ids(&palette), ["exact", "tie", "gap"]);
}

#[test]
fn picker_restores_search_and_updates_preserve_stable_selection() {
    let mut palette = palette();
    palette.insert_text("new");
    key(&mut palette, BareKey::Down);
    palette.open_picker(EntryKind::Pane);
    assert_eq!(ids(&palette), ["pane"]);
    palette.insert_text("console");
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.input.get_text(), "new");
    assert_eq!(palette.selected_entry().unwrap().id, "tab");
    let mut entries = palette.entries.clone();
    entries.insert(0, entry("first", "New first", EntryKind::Command));
    palette.set_entries(entries);
    assert_eq!(palette.selected_entry().unwrap().id, "tab");
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Esc)),
        Outcome::Dismiss
    );
}

#[test]
fn forms_edit_navigate_preserve_search_and_only_submit_from_button() {
    let mut palette = palette();
    palette.insert_text("new");
    palette.open_form(Form::new(
        "form".into(),
        "New pane".into(),
        vec![
            Field::text("Name", "original"),
            Field::choice("Placement", &["Tiled", "Floating"]),
        ],
        "Create",
        false,
    ));
    key(&mut palette, BareKey::Left);
    key(&mut palette, BareKey::Char('!'));
    key(&mut palette, BareKey::Enter);
    key(&mut palette, BareKey::Right);
    key(&mut palette, BareKey::Enter);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Submit {
            id: "form".into(),
            values: vec!["origina!l".into(), "Floating".into()]
        }
    );
    palette.set_error("Try another name".into());
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let rendered: String = palette
        .chunks(&layout, &Style::default())
        .iter()
        .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
        .collect();
    assert!(rendered.contains("Try another name"));
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier()),
        Outcome::None
    );
    assert_eq!(palette.form.as_ref().unwrap().selected, 1);
    key(&mut palette, BareKey::Up);
    assert_eq!(palette.form.as_ref().unwrap().selected, 0);
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.input.get_text(), "new");
}

#[test]
fn confirmation_defaults_to_cancel_and_requires_explicit_selection() {
    let mut palette = palette();
    for confirm in [false, true] {
        palette.open_form(Form::new(
            "close".into(),
            "Close terminal 1?".into(),
            vec![],
            "Close pane",
            true,
        ));
        if confirm {
            key(&mut palette, BareKey::Down);
            assert_eq!(
                palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                Outcome::Submit {
                    id: "close".into(),
                    values: vec![]
                }
            );
        } else {
            key(&mut palette, BareKey::Enter);
            assert!(palette.form.is_none());
        }
    }
}

#[test]
fn clicking_uses_scrolled_geometry() {
    let mut palette = palette();
    let surface = Viewport {
        cols: 30,
        rows: 6,
        ..Default::default()
    };
    key(&mut palette, BareKey::Down);
    key(&mut palette, BareKey::Down);
    let layout = palette.layout(surface);
    assert_eq!(layout.first_option, 2);
    palette.click(
        &layout,
        Position::new(layout.options_y as i32, layout.rect.x as u16 + 2),
    );
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("pane".into())
    );
}

#[test]
fn editor_keys_unicode_and_paste() {
    let mut palette = CommandPalette::default();
    palette.insert_text("日本\n\ttext\u{2003}more\x1b");
    assert_eq!(palette.input.get_text(), "日本  text more");
    palette.handle_key(KeyWithModifier::new(BareKey::Char('w')).with_ctrl_modifier());
    assert_eq!(palette.input.get_text(), "日本  text ");
    key(&mut palette, BareKey::Home);
    key(&mut palette, BareKey::Right);
    key(&mut palette, BareKey::Delete);
    assert_eq!(palette.input.get_text(), "日  text ");
    key(&mut palette, BareKey::Char('界'));
    assert_eq!(palette.input.get_text(), "日界  text ");
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    assert_eq!(palette.cursor(&layout), Some((15, 4)));
    palette.handle_key(KeyWithModifier::new(BareKey::Char('n')).with_alt_modifier());
    assert_eq!(palette.input.get_text(), "日界  text ");
}

#[test]
fn layout_and_cursor_fit_even_the_smallest_surfaces() {
    let mut palette = CommandPalette::default();
    palette.insert_text("界e\u{301}".repeat(80).as_str());
    for cols in 0..90 {
        for rows in 0..35 {
            let surface = Viewport {
                x: 2,
                y: 3,
                cols,
                rows,
            };
            let layout = palette.layout(surface);
            assert!(layout.rect.x >= surface.x && layout.rect.y >= surface.y);
            assert!(layout.rect.x + layout.rect.cols <= surface.x + cols);
            assert!(layout.rect.y + layout.rect.rows <= surface.y + rows);
            for chunk in palette.chunks(&layout, &Style::default()) {
                assert_eq!(chunk.width(), layout.rect.cols);
                assert!(chunk.y < surface.y + rows);
            }
            if let Some((x, y)) = palette.cursor(&layout) {
                assert!(layout.rect.contains(x, y));
            }
        }
    }
    let layout = palette.layout(Viewport {
        cols: 100,
        rows: 30,
        ..Default::default()
    });
    assert_eq!(
        layout.rect,
        CellRect {
            x: 18,
            y: 5,
            cols: 64,
            rows: 11
        }
    );
}

#[test]
fn control_navigation_matches_arrows_in_results_and_forms() {
    let mut palette = palette();
    let next = KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier();
    let previous = KeyWithModifier::new(BareKey::Char('p')).with_ctrl_modifier();
    for (key, selected) in [
        (previous.clone(), 0),
        (next.clone(), 1),
        (next.clone(), 2),
        (next.clone(), 2),
        (previous.clone(), 1),
    ] {
        assert_eq!(palette.handle_key(key), Outcome::None);
        assert_eq!(palette.selected, selected);
        assert_eq!(palette.input.get_text(), "");
    }
    palette.open_form(Form::new(
        "form".into(),
        "Form".into(),
        vec![Field::text("Name", "original")],
        "Create",
        false,
    ));
    palette.handle_key(next);
    assert_eq!(palette.form.as_ref().unwrap().selected, 1);
    palette.handle_key(previous);
    assert_eq!(palette.form.as_ref().unwrap().selected, 0);
    assert_eq!(
        palette.form.as_ref().unwrap().fields[0].text_value(),
        "original"
    );
}

#[test]
fn shortcuts_remain_visible_beside_long_unicode_labels() {
    let mut palette = CommandPalette::default();
    let mut entry = entry("new", &"界".repeat(80), EntryKind::Command);
    entry.shortcut = "Ctrl g → Alt n".into();
    palette.set_entries(vec![entry]);
    for cols in 0..90 {
        let layout = palette.layout(Viewport {
            cols,
            rows: 24,
            ..Default::default()
        });
        for chunk in palette.chunks(&layout, &Style::default()) {
            assert_eq!(chunk.width(), layout.rect.cols);
        }
        if cols >= 30 {
            assert!(rendered_lines(&palette, &layout)
                .iter()
                .any(|line| line.contains("Ctrl g → Alt n")));
        }
    }
}

#[test]
fn shortcuts_are_subtle_on_selected_and_unselected_rows() {
    let mut palette = palette();
    for entry in &mut palette.entries {
        entry.shortcut = "Ctrl n".into();
    }
    for rows in [3, 24] {
        let layout = palette.layout(Viewport {
            cols: 60,
            rows,
            ..Default::default()
        });
        for chunk in palette.chunks(&layout, &Style::default()) {
            let text: String = chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect();
            if let Some(start) = text.find("Ctrl n") {
                let start = text[..start].chars().count();
                let characters = &chunk.terminal_characters;
                assert_ne!(characters[layout.border].styles.dim, Some(AnsiCode::On));
                for character in &characters[start..start + 6] {
                    assert_eq!(character.styles.dim, Some(AnsiCode::On));
                    assert_eq!(
                        character.styles.background,
                        characters[layout.border].styles.background
                    );
                }
            }
        }
    }
}

#[test]
fn session_picker_groups_search_results_and_keeps_selection_on_refresh() {
    let mut palette = CommandPalette::default();
    palette.set_entries(vec![
        entry("new-live", "n--------p", EntryKind::Session),
        entry("old-live", "np", EntryKind::Session),
        entry("new-exited", "n--------p", EntryKind::ExitedSession),
        entry("old-exited", "np", EntryKind::ExitedSession),
        entry("command", "np", EntryKind::Command),
    ]);
    palette.open_picker(EntryKind::Session);
    assert_eq!(
        ids(&palette),
        ["new-live", "old-live", "new-exited", "old-exited"]
    );
    palette.insert_text("np");
    assert_eq!(
        ids(&palette),
        ["old-live", "new-live", "old-exited", "new-exited"]
    );
    let surface = Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    for selected in 0..palette.matches.len() {
        palette.selected = selected;
        let layout = palette.layout(surface);
        let lines = rendered_lines(&palette, &layout);
        let mut group = None;
        for (row, result) in layout.result_rows.iter().enumerate() {
            match result {
                ResultRow::Heading(label) => {
                    group = Some(label.as_str());
                    assert!(lines[layout.options_y + row - layout.rect.y].contains(label));
                    palette.click(
                        &layout,
                        Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
                    );
                    assert_eq!(palette.selected, selected);
                },
                ResultRow::Status(_, _) => panic!("native entries have no plugin status"),
                ResultRow::Entry(index) => {
                    let expected =
                        if palette.entries[palette.matches[*index]].kind == EntryKind::Session {
                            "Live sessions"
                        } else {
                            "Exited sessions"
                        };
                    assert_eq!(group, Some(expected));
                    palette.click(
                        &layout,
                        Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
                    );
                    assert_eq!(
                        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                        Outcome::Activate(palette.entries[palette.matches[*index]].id.clone())
                    );
                    palette.selected = selected;
                },
            }
        }
    }
    let selected = palette.selected_entry().unwrap().id.clone();
    let mut entries = palette.entries.clone();
    entries.reverse();
    for entry in &mut entries {
        entry.description = "updated age".into();
    }
    palette.set_entries(entries);
    assert_eq!(palette.selected_entry().unwrap().id, selected);
    palette.set_entries(vec![entry("exited", "np", EntryKind::ExitedSession)]);
    let lines = rendered_lines(&palette, &palette.layout(surface)).join("\n");
    assert!(lines.contains("Exited sessions"));
    assert!(!lines.contains("Live sessions"));
    palette.set_entries(vec![]);
    let lines = rendered_lines(&palette, &palette.layout(surface)).join("\n");
    assert!(lines.contains("No matches"));
    assert!(!lines.contains("Exited sessions"));
}

#[test]
fn project_picker_restores_search_and_clips_unicode_in_every_view() {
    let mut palette = palette();
    let name = "日本語🦀".repeat(30);
    palette.current_project = Some(name.clone());
    let mut project = entry("project", &name, EntryKind::Project);
    project.description = "Current".into();
    let mut entries = palette.native_entries.clone();
    entries.push(project);
    palette.set_entries(entries);
    palette.insert_text("New");
    key(&mut palette, BareKey::Down);
    let selected = palette.selected_entry().unwrap().id.clone();
    palette.open_picker(EntryKind::Project);
    palette.insert_text("日本");
    assert_eq!(ids(&palette), vec!["project"]);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("project".into())
    );
    let surface = Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    let lines = rendered_lines(&palette, &palette.layout(surface));
    assert!(lines[0].contains("Project: 日本"));
    for line in &lines {
        assert!(unicode_width::UnicodeWidthStr::width(line.as_str()) <= 80);
    }
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.input.get_text(), "New");
    assert_eq!(palette.selected_entry().unwrap().id, selected);
    palette.open_picker(EntryKind::Project);
    assert_eq!(palette.back(), Outcome::None);
    assert_eq!(palette.input.get_text(), "New");
    palette.open_form(Form::new("form".into(), "Form".into(), vec![], "OK", false));
    assert!(rendered_lines(&palette, &palette.layout(surface))[0].contains("Project: 日本"));
    palette.back();
    palette.set_entries(vec![]);
    palette.open_picker(EntryKind::Project);
    assert!(rendered_lines(&palette, &palette.layout(surface))
        .iter()
        .any(|line| line.contains("No projects")));
}

#[test]
fn plugin_sections_keep_semantic_order_and_selection() {
    let mut palette = palette();
    palette.insert_text("new");
    let section = |ids: &[&str]| PluginSection {
        title: "Remote".into(),
        entries: ids
            .iter()
            .map(|id| entry(id, id, EntryKind::Plugin))
            .collect(),
        semantic: true,
        status: None,
    };
    palette.set_sections(vec![section(&["unrelated", "also unrelated"])]);
    assert_eq!(ids(&palette), ["new", "tab", "unrelated", "also unrelated"]);
    key(&mut palette, BareKey::Down);
    key(&mut palette, BareKey::Down);
    palette.set_sections(vec![section(&["inserted", "unrelated", "also unrelated"])]);
    assert_eq!(palette.selected_entry().unwrap().id, "unrelated");
    key(&mut palette, BareKey::Char('x'));
    assert_eq!(palette.selected_entry().unwrap().id, "unrelated");
    palette.insert_text("pasted");
    assert_eq!(palette.selected_entry().unwrap().id, "unrelated");
    palette.push_view(1, "Static".into());
    let mut static_section = section(&["apple", "banana"]);
    static_section.semantic = false;
    palette.set_sections(vec![static_section]);
    palette.insert_text("ban");
    assert_eq!(ids(&palette), ["banana"]);
    palette.back();
    assert_eq!(palette.query(), "newxpasted");
    assert_eq!(palette.selected_entry().unwrap().id, "unrelated");
    palette.set_sections(vec![section(&["replacement"])]);
    assert_eq!(palette.selected_entry().unwrap().id, "replacement");
}

#[test]
fn provider_statuses_are_reachable_and_never_activate_entries() {
    for (next, previous) in [
        (
            KeyWithModifier::new(BareKey::Down),
            KeyWithModifier::new(BareKey::Up),
        ),
        (
            KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('p')).with_ctrl_modifier(),
        ),
    ] {
        for entry_count in [0, 9] {
            let mut palette = CommandPalette::default();
            palette.set_entries(
                (0..entry_count)
                    .map(|i| entry(&i.to_string(), &format!("Command {i}"), EntryKind::Command))
                    .collect(),
            );
            let statuses = ["Loading…", "No actions", "Provider failed"];
            palette.set_sections(
                statuses
                    .iter()
                    .enumerate()
                    .map(|(i, status)| PluginSection {
                        title: format!("Provider {i}"),
                        entries: vec![],
                        semantic: true,
                        status: Some((*status).into()),
                    })
                    .collect(),
            );
            for _ in 0..entry_count {
                assert_eq!(palette.handle_key(next.clone()), Outcome::None);
            }
            for (section, key) in [
                (0, &next),
                (1, &next),
                (2, &next),
                (2, &previous),
                (1, &previous),
                (0, &previous),
            ] {
                assert_eq!(palette.selected_status, Some(section));
                assert!(palette.selected_entry().is_none());
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::None
                );
                for rows in [4, 7, 24] {
                    let layout = palette.layout(Viewport {
                        cols: 80,
                        rows,
                        ..Default::default()
                    });
                    let lines = rendered_lines(&palette, &layout);
                    let row = layout
                        .result_rows
                        .iter()
                        .position(|row| matches!(row, ResultRow::Status(i, _) if *i == section))
                        .expect("focused status must be visible");
                    let y = layout.options_y + row - layout.rect.y;
                    assert!(lines[y].contains(statuses[section]));
                    assert!(!lines.iter().any(|line| line.contains('›')));
                    if layout.options_rows >= 2 {
                        assert!(lines
                            .iter()
                            .any(|line| line.contains(&format!("Provider {section}"))));
                    }
                    let style = Style::default();
                    let chunks = palette.chunks(&layout, &style);
                    assert_eq!(
                        chunks[y].terminal_characters[layout.border]
                            .styles
                            .background,
                        Some(style.colors.text_selected.background.into())
                    );
                }
                assert_eq!(palette.handle_key(key.clone()), Outcome::None);
            }
            if entry_count == 0 {
                assert_eq!(palette.selected_status, Some(0));
            } else {
                assert_eq!(palette.selected_status, None);
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate((entry_count - 1).to_string())
                );
            }
        }
    }
}

#[test]
fn provider_status_focus_shares_mouse_geometry_and_survives_view_history() {
    let mut palette = CommandPalette::default();
    palette.set_sections(vec![
        PluginSection {
            title: "First".into(),
            entries: vec![entry("first", "First command", EntryKind::Plugin)],
            semantic: true,
            status: Some("Loading…".into()),
        },
        PluginSection {
            title: "Second".into(),
            entries: vec![entry("second", "Second command", EntryKind::Plugin)],
            semantic: true,
            status: Some("No more actions".into()),
        },
    ]);
    let surface = Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_status, Some(0));
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_entry().unwrap().id, "second");
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_status, Some(1));
    let layout = palette.layout(surface);
    for (row, result) in layout.result_rows.iter().enumerate() {
        palette.click(
            &layout,
            Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
        );
        match result {
            ResultRow::Entry(index) => {
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate(palette.entries[palette.matches[*index]].id.clone())
                );
            },
            ResultRow::Status(section, _) => {
                assert_eq!(palette.selected_status, Some(*section));
                key(&mut palette, BareKey::Enter);
            },
            ResultRow::Heading(_) => {},
        }
    }
    assert_eq!(palette.selected_status, Some(1));
    palette.set_sections(palette.sections.clone());
    assert_eq!(palette.selected_status, Some(1));
    palette.push_view(1, "Nested".into());
    palette.back();
    assert_eq!(palette.selected_status, Some(1));
    key(&mut palette, BareKey::Enter);

    let mut sections = palette.sections.clone();
    sections[0]
        .entries
        .insert(0, entry("inserted", "Inserted", EntryKind::Plugin));
    sections[1].status = None;
    palette.set_sections(sections);
    assert_eq!(palette.selected_status, None);
    assert_eq!(palette.selected_entry().unwrap().id, "second");
    key(&mut palette, BareKey::Up);
    assert_eq!(palette.selected_status, Some(0));
    palette.insert_text("Second");
    assert_eq!(palette.selected_status, None);
    key(&mut palette, BareKey::Down);
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_status, Some(0));
    palette.open_picker(EntryKind::Pane);
    assert_eq!(palette.selected_status, None);
    key(&mut palette, BareKey::Enter);
}

#[test]
fn plugin_form_history_busy_and_text_scroll() {
    let mut palette = palette();
    palette.push_view(1, "Form".into());
    palette.open_form(Form::new(
        "submit".into(),
        "Form".into(),
        vec![Field::text("Name", "")],
        "Submit",
        false,
    ));
    palette.insert_text("retained");
    key(&mut palette, BareKey::Tab);
    palette.busy = true;
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::None
    );
    palette.insert_text("ignored");
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Esc)),
        Outcome::Cancel
    );
    palette.busy = false;
    palette.push_view(2, "Output".into());
    palette.open_text((0..50).map(|i| format!("Row {i}\n")).collect());
    let surface = Viewport {
        x: 0,
        y: 0,
        cols: 80,
        rows: 24,
    };
    let layout = palette.layout(surface);
    assert!(rendered_lines(&palette, &layout)
        .iter()
        .any(|s| s.contains("Row 0")));
    key(&mut palette, BareKey::End);
    let layout = palette.layout(surface);
    assert!(rendered_lines(&palette, &layout)
        .iter()
        .any(|s| s.contains("Row 49")));
    key(&mut palette, BareKey::Up);
    assert!(palette.text_offset < 50);
    key(&mut palette, BareKey::Home);
    assert_eq!(palette.text_offset, 0);
    palette.back();
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Submit {
            id: "submit".into(),
            values: vec!["retained".into()]
        }
    );
}

#[test]
fn root_discovery_dismisses_but_nested_discovery_and_execution_cancel() {
    let mut palette = palette();
    palette.pending = true;
    assert_eq!(palette.back(), Outcome::Dismiss);
    palette.busy = true;
    assert_eq!(palette.back(), Outcome::Cancel);
    palette.busy = false;
    palette.push_view(1, "Nested".into());
    palette.pending = true;
    assert_eq!(palette.back(), Outcome::Cancel);
    palette.pending = false;
    assert_eq!(palette.back(), Outcome::None);
    assert!(palette.is_root());
}

#[test]
fn control_agent_pickers_are_scoped_and_preserve_navigation_and_project() {
    for (kind, empty) in [
        (EntryKind::ControlAgent, "No control agents"),
        (EntryKind::SetControlAgent, "No agent panes"),
    ] {
        let mut palette = palette();
        palette.current_project = Some("project".into());
        let mut entries = palette.native_entries.clone();
        entries.extend([
            entry("control:a:1", "agent one", kind),
            entry("control:b:1", "agent two", kind),
        ]);
        palette.set_entries(entries.clone());
        assert!(!ids(&palette).contains(&"control:a:1"));
        palette.insert_text("New");
        let selected = palette.selected_entry().unwrap().id.clone();
        palette.open_picker(kind);
        assert_eq!(ids(&palette), vec!["control:a:1", "control:b:1"]);
        palette.push_view(42, "Plugin view".into());
        palette.set_sections(vec![PluginSection {
            title: "Plugin".into(),
            entries: vec![entry("plugin-child", "child", EntryKind::Plugin)],
            semantic: false,
            status: None,
        }]);
        assert_eq!(ids(&palette), vec!["plugin-child"]);
        assert_eq!(palette.current_project.as_deref(), Some("project"));
        assert_eq!(palette.back(), Outcome::None);
        assert_eq!(ids(&palette), vec!["control:a:1", "control:b:1"]);
        key(&mut palette, BareKey::Down);
        entries.reverse();
        palette.set_entries(entries);
        assert_eq!(palette.selected_entry().unwrap().id, "control:b:1");
        let surface = Viewport {
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        palette.insert_text("absent");
        assert!(rendered_lines(&palette, &palette.layout(surface))
            .join("\n")
            .contains("No matches"));
        key(&mut palette, BareKey::Esc);
        assert_eq!(palette.query(), "New");
        assert_eq!(palette.selected_entry().unwrap().id, selected);
        palette.open_picker(kind);
        palette.set_entries(vec![entry("pane", "ordinary", EntryKind::Pane)]);
        let text = rendered_lines(&palette, &palette.layout(surface)).join("\n");
        assert!(text.contains(empty));
        assert!(text.contains("Project: project"));
        assert_eq!(palette.back(), Outcome::None);
        assert_eq!(palette.query(), "New");
    }
}

#[test]
fn nested_warp_pickers_restore_search_selection_and_hide_destinations_at_root() {
    for (sessions, targets) in [
        (EntryKind::WarpTabSession, EntryKind::WarpTab),
        (EntryKind::WarpPaneSession, EntryKind::WarpPane),
    ] {
        let mut palette = CommandPalette::default();
        let entries = vec![
            entry("warp", "Warp", EntryKind::Command),
            entry("source-a", "Alpha", sessions),
            entry("source-b", "Beta", sessions),
            entry("target-a", "First target", targets),
            entry("target-b", "Second target", targets),
        ];
        palette.set_entries(entries.clone());
        assert_eq!(ids(&palette), vec!["warp"]);
        palette.insert_text("wp");
        palette.open_picker(sessions);
        assert_eq!(ids(&palette), vec!["source-a", "source-b"]);
        palette.insert_text("bt");
        assert_eq!(ids(&palette), vec!["source-b"]);
        palette.open_picker(targets);
        palette.insert_text("sctg");
        assert_eq!(ids(&palette), vec!["target-b"]);
        // A discovery refresh preserves the surviving selection.
        palette.set_entries(
            entries
                .into_iter()
                .filter(|entry| entry.id != "target-a")
                .collect(),
        );
        assert_eq!(palette.selected_entry().unwrap().id, "target-b");
        key(&mut palette, BareKey::Esc);
        assert_eq!(palette.query(), "bt");
        assert_eq!(palette.selected_entry().unwrap().id, "source-b");
        key(&mut palette, BareKey::Esc);
        assert_eq!(palette.query(), "wp");
        assert_eq!(palette.selected_entry().unwrap().id, "warp");
        palette.open_picker(targets);
        palette.set_entries(vec![]);
        key(&mut palette, BareKey::Enter);
    }
}

#[test]
fn live_agent_filters_refresh_selection_and_control_enter() {
    let mut palette = CommandPalette {
        agent_metadata: std::collections::BTreeMap::from([
            (
                "one".into(),
                std::collections::BTreeMap::from([
                    ("state".into(), "idle".into()),
                    ("directory".into(), "/work/café space".into()),
                ]),
            ),
            (
                "two".into(),
                std::collections::BTreeMap::from([("state".into(), "busy".into())]),
            ),
        ]),
        ..Default::default()
    };
    palette.set_entries(vec![
        entry("one", "日本語", EntryKind::Agent),
        entry("two", "Other", EntryKind::Agent),
    ]);
    palette.insert_text("state:idle directory:\"café space\"");
    assert_eq!(ids(&palette), ["one"]);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier()),
        Outcome::FocusAgent("one".into())
    );
    palette.push_view(99, "Agent info".into());
    palette.metadata = Some((0..100).map(|n| format!("日本語 {n}\n")).collect());
    for cols in 0..15 {
        for rows in 0..15 {
            let layout = palette.layout(Viewport {
                x: 0,
                y: 0,
                cols,
                rows,
            });
            assert!(layout.rect.cols <= cols && layout.rect.rows <= rows);
            palette.chunks(&layout, &Style::default());
        }
    }
    key(&mut palette, BareKey::End);
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.query(), "state:idle directory:\"café space\"");
    assert_eq!(ids(&palette), ["one"]);
    palette
        .agent_metadata
        .get_mut("one")
        .unwrap()
        .insert("state".into(), "waiting".into());
    palette.set_entries(vec![
        entry("one", "Updated title", EntryKind::Agent),
        entry("two", "Other", EntryKind::Agent),
    ]);
    assert!(ids(&palette).is_empty());
}

#[test]
fn agent_info_empty_timeline_cannot_activate_resume() {
    let mut palette = CommandPalette {
        metadata: Some("Archived run".into()),
        ..Default::default()
    };
    palette.set_entries(vec![entry("resume", "Resume", EntryKind::Command)]);
    key(&mut palette, BareKey::Enter);
    key(&mut palette, BareKey::Tab);
    assert_eq!(palette.metadata_focus, 1);
    for key_code in [
        BareKey::Up,
        BareKey::Down,
        BareKey::PageUp,
        BareKey::PageDown,
        BareKey::Home,
        BareKey::End,
    ] {
        key(&mut palette, key_code);
        assert!(palette.selected_entry().is_none());
        assert_eq!(palette.selected, palette.matches.len());
        assert!(!palette
            .result_rows(10)
            .iter()
            .any(|row| palette.row_selected(row)));
        key(&mut palette, BareKey::Enter);
    }
    // Even a stale programmatic selection cannot cross the focus boundary.
    palette.select_id("resume");
    key(&mut palette, BareKey::Enter);
    assert!(palette.selected_entry().is_none());
    key(&mut palette, BareKey::Tab);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("resume".into())
    );
    key(&mut palette, BareKey::Tab);
    key(&mut palette, BareKey::Enter);
    palette.handle_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
    assert_eq!(palette.selected_entry().unwrap().id, "resume");
}

#[test]
fn agent_info_refresh_and_mouse_selection_respect_regions() {
    let mut palette = CommandPalette {
        metadata: Some("Archived run".into()),
        ..Default::default()
    };
    let sections = |events: &[&str]| {
        vec![PluginSection {
            title: "Timeline".into(),
            entries: std::iter::once(entry("resume", "Resume", EntryKind::Plugin))
                .chain(events.iter().map(|id| entry(id, id, EntryKind::Plugin)))
                .collect(),
            semantic: true,
            status: None,
        }]
    };
    palette.set_sections(sections(&["event:1", "event:2"]));
    key(&mut palette, BareKey::Tab);
    assert_eq!(palette.selected_entry().unwrap().id, "event:1");
    palette.select_id("event:2");
    palette.set_query("search".into());
    assert_eq!(palette.selected_entry().unwrap().id, "event:2");
    palette.set_sections(sections(&["event:1"]));
    assert_eq!(palette.selected_entry().unwrap().id, "event:1");
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("event:1".into())
    );
    palette.set_sections(sections(&[]));
    palette.set_query("another search".into());
    assert_eq!(palette.metadata_focus, 1);
    assert!(palette.selected_entry().is_none());
    key(&mut palette, BareKey::Enter);
    palette.set_sections(sections(&["event:3"]));
    assert_eq!(palette.selected_entry().unwrap().id, "event:3");
    // Non-semantic search can also remove all events while retaining an action.
    let mut filtered = sections(&["event:3"]);
    filtered[0].semantic = false;
    palette.set_sections(filtered);
    palette.set_query("Resume".into());
    assert!(palette.selected_entry().is_none());
    key(&mut palette, BareKey::Enter);
    palette.set_query(String::new());
    assert_eq!(palette.selected_entry().unwrap().id, "event:3");
    let layout = palette.layout(Viewport {
        cols: 100,
        rows: 36,
        ..Default::default()
    });
    for (id, focus) in [("resume", 2), ("event:3", 1)] {
        let row = layout
            .result_rows
            .iter()
            .position(|row| match row {
                ResultRow::Entry(index) => palette.entries[palette.matches[*index]].id == id,
                _ => false,
            })
            .unwrap();
        palette.click(
            &layout,
            Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
        );
        assert_eq!(palette.metadata_focus, focus);
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate(id.into())
        );
    }
}

#[test]
fn static_palette_aliases_use_native_matching_at_root_and_nested() {
    for nested in [false, true] {
        let mut palette = CommandPalette::default();
        if nested {
            palette.push_view(1, "Nested".into());
        }
        let mut pdf = entry("pdf", "Open PDF…", EntryKind::Plugin);
        pdf.aliases = vec![
            "reader".into(),
            "document".into(),
            "portable document".into(),
        ];
        pdf.description = "descriptiononly".into();
        palette.set_sections(vec![PluginSection {
            title: "provideronly".into(),
            entries: vec![pdf],
            semantic: false,
            status: None,
        }]);
        for query in [
            "pdf",
            "reader",
            "document",
            "PDF",
            "READER",
            "DOCUMENT",
            "opdf",
            "portable document",
        ] {
            palette.set_query(query.into());
            assert_eq!(ids(&palette), ["pdf"], "{query}");
        }
        for query in [
            "unrelated",
            "descriptiononly",
            "provideronly",
            "reader document",
        ] {
            palette.set_query(query.into());
            assert!(ids(&palette).is_empty(), "{query}");
            assert!(palette.result_rows(10).is_empty());
        }
    }
}

#[test]
fn static_palette_scores_interleave_with_native_and_preserve_ties_and_geometry() {
    for aliases in [false, true] {
        let mut palette = CommandPalette::default();
        palette.set_entries(vec![
            entry("native-low", "n--------p", EntryKind::Command),
            entry("native-high", "np", EntryKind::Command),
        ]);
        let static_entry = |id, label| {
            let mut value = entry(id, label, EntryKind::Plugin);
            if aliases {
                value.aliases.push(label.into());
                value.label = "Other".into();
            }
            value
        };
        palette.set_sections(vec![
            PluginSection {
                title: "A".into(),
                semantic: false,
                status: None,
                entries: vec![
                    static_entry("a-low", "n--------p"),
                    static_entry("a-high", "np"),
                ],
            },
            PluginSection {
                title: "B".into(),
                semantic: false,
                status: Some("Loading…".into()),
                entries: vec![
                    static_entry("b-high", "np"),
                    static_entry("b-low", "n--------p"),
                ],
            },
            PluginSection {
                title: "Dynamic".into(),
                semantic: true,
                status: None,
                entries: vec![
                    entry("dynamic-z", "zzz", EntryKind::Plugin),
                    entry("dynamic-a", "aaa", EntryKind::Plugin),
                ],
            },
        ]);
        for query in ["", "   "] {
            palette.set_query(query.into());
            assert_eq!(
                ids(&palette),
                [
                    "native-low",
                    "native-high",
                    "a-low",
                    "a-high",
                    "b-high",
                    "b-low",
                    "dynamic-z",
                    "dynamic-a"
                ]
            );
        }
        palette.set_query("np".into());
        assert_eq!(
            ids(&palette),
            [
                "native-high",
                "a-high",
                "b-high",
                "native-low",
                "a-low",
                "b-low",
                "dynamic-z",
                "dynamic-a"
            ]
        );
        let rows = palette.result_rows(30);
        let indices: Vec<_> = rows
            .iter()
            .filter_map(|r| {
                if let ResultRow::Entry(i) = r {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(indices, (0..palette.matches.len()).collect::<Vec<_>>());
        assert!(rows
            .iter()
            .any(|r| matches!(r, ResultRow::Status(1, status) if status == "Loading…")));
        for height in [8, 12, 24] {
            for selected in 0..palette.matches.len() {
                palette.selected = selected;
                palette.selected_status = None;
                let layout = palette.layout(Viewport {
                    cols: 80,
                    rows: height,
                    ..Default::default()
                });
                assert!(layout
                    .result_rows
                    .iter()
                    .any(|r| matches!(r, ResultRow::Entry(i) if *i == selected)));
                let mut heading = None;
                for (row, result) in layout.result_rows.iter().enumerate() {
                    match result {
                        ResultRow::Heading(title) => heading = Some(title.as_str()),
                        ResultRow::Entry(index) => {
                            if let Some(section) = palette.entry_sections[palette.matches[*index]] {
                                if layout.result_rows.len() > 1 {
                                    assert_eq!(
                                        heading,
                                        Some(palette.sections[section].title.as_str())
                                    );
                                }
                            }
                            palette.click(
                                &layout,
                                Position::new(
                                    (layout.options_y + row) as i32,
                                    layout.rect.x as u16 + 2,
                                ),
                            );
                            assert_eq!(
                                palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                                Outcome::Activate(
                                    palette.entries[palette.matches[*index]].id.clone()
                                )
                            );
                        },
                        _ => (),
                    }
                }
                palette.selected = selected;
                palette.selected_status = None;
                key(&mut palette, BareKey::Down);
            }
        }
        palette.select_id("a-high");
        let mut sections = palette.sections.clone();
        sections[2]
            .entries
            .push(entry("dynamic-new", "New result", EntryKind::Plugin));
        palette.set_sections(sections);
        assert_eq!(palette.selected_entry().unwrap().id, "a-high");
        palette.set_query("no-match".into());
        assert_eq!(ids(&palette), ["dynamic-z", "dynamic-a", "dynamic-new"]);
        assert!(!palette
            .result_rows(20)
            .iter()
            .any(|r| matches!(r, ResultRow::Heading(t) if t == "A")));
    }
}
