use super::*;

fn key(palette: &mut CommandPalette, key: BareKey) {
    assert_eq!(palette.handle_key(KeyWithModifier::new(key)), Outcome::None);
}

fn entry(id: &str, label: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        label: label.into(),
        description: String::new(),
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
                    parent = Some(*label);
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
