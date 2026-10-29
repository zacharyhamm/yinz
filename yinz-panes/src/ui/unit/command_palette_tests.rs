use super::*;

#[test]
fn wheel_moves_list_selection_and_clamps_without_editing_query() {
    let mut palette = CommandPalette::default();
    palette.set_entries(
        (0..12)
            .map(|i| {
                entry(
                    &format!("item-{i}"),
                    &format!("Item {i:02}"),
                    EntryKind::Command,
                )
            })
            .collect(),
    );
    palette.insert_text("Item");
    palette.scroll(true);
    assert_eq!(palette.selected, 3);
    palette.busy = true;
    palette.scroll(true);
    assert_eq!(palette.selected, 3);
    palette.busy = false;
    for _ in 0..10 {
        palette.scroll(true);
    }
    assert_eq!(palette.selected, 11);
    for _ in 0..10 {
        palette.scroll(false);
    }
    assert_eq!(palette.selected, 0);
    assert_eq!(palette.query(), "Item");
    palette.insert_text("missing");
    palette.scroll(true);
    palette.scroll(false);
    assert!(palette.selected_entry().is_none());
}

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

fn footer(palette: &CommandPalette, layout: &CommandPaletteLayout) -> String {
    rendered_lines(palette, layout)[layout.footer_y.unwrap() - layout.rect.y].clone()
}

fn overflow_palette(count: usize) -> CommandPalette {
    let mut palette = CommandPalette::default();
    palette.set_entries(
        (0..count)
            .map(|i| entry(&i.to_string(), &format!("Item {i:02}"), EntryKind::Command))
            .collect(),
    );
    palette.set_query("Item".into());
    palette
}

#[test]
fn overflow_footer_tracks_navigation_in_root_picker_and_nested_lists() {
    let surface = Viewport {
        cols: 80,
        rows: 20,
        ..Default::default()
    };
    for (view, hidden) in [("root", 13), ("picker", 13), ("nested", 14)] {
        let mut palette = overflow_palette(20);
        if view == "picker" {
            let mut entries = palette.native_entries.clone();
            for entry in &mut entries {
                entry.kind = EntryKind::Pane;
            }
            palette.set_entries(entries);
            palette.open_picker(EntryKind::Pane);
        } else if view == "nested" {
            let mut entries = palette.native_entries.clone();
            for entry in &mut entries {
                entry.kind = EntryKind::Plugin;
            }
            palette.push_view(42, "Choose item".into());
            palette.set_sections(vec![PluginSection {
                title: String::new(),
                entries,
                semantic: true,
                status: None,
            }]);
        }
        let layout = palette.layout(surface);
        assert_eq!(layout.options_rows, 7);
        assert!(footer(&palette, &layout).contains(&format!("↑ 0 above · ↓ {hidden} below · ↑↓")));
        key(&mut palette, BareKey::PageDown);
        let layout = palette.layout(surface);
        assert!(footer(&palette, &layout).contains(&format!("↑ 1 above · ↓ {} below", hidden - 1)));
        palette.scroll(true);
        let layout = palette.layout(surface);
        assert!(footer(&palette, &layout).contains(&format!("↑ 4 above · ↓ {} below", hidden - 4)));
        palette.select_id("19");
        let layout = palette.layout(surface);
        assert!(footer(&palette, &layout).contains(&format!("↑ {hidden} above · ↓ 0 below")));
        palette.select_id("0");
        let layout = palette.layout(surface);
        assert!(footer(&palette, &layout).contains(&format!("↑ 0 above · ↓ {hidden} below")));
    }
}

#[test]
fn overflow_footer_refreshes_after_filter_updates_and_resize() {
    let mut palette = overflow_palette(20);
    let mut surface = Viewport {
        cols: 80,
        rows: 20,
        ..Default::default()
    };
    let mut entries = palette.native_entries.clone();
    for entry in &mut entries[..10] {
        entry.aliases.push("keep".into());
    }
    palette.set_entries(entries.clone());
    palette.set_query("keep".into());
    let layout = palette.layout(surface);
    assert!(footer(&palette, &layout).contains("↑ 0 above · ↓ 3 below"));
    surface.rows = 40;
    let layout = palette.layout(surface);
    assert_eq!(layout.result_overflow, None);
    assert!(!footer(&palette, &layout).contains("above"));
    surface.rows = 20;
    palette.set_entries(entries[..3].to_vec());
    assert_eq!(palette.layout(surface).result_overflow, None);
    palette.set_entries(entries);
    let layout = palette.layout(surface);
    assert!(footer(&palette, &layout).contains("↑ 0 above · ↓ 3 below"));
    palette.set_query("missing".into());
    assert_eq!(palette.layout(surface).result_overflow, None);
}

#[test]
fn overflow_footer_counts_wrapped_and_partial_entries_once() {
    let mut palette = wrapped_palette(&["A long title that wraps onto two lines"; 6]);
    let mut surface = Viewport {
        cols: 32,
        rows: 19,
        ..Default::default()
    };
    let layout = palette.layout(surface);
    assert_eq!(layout.options_rows, 6);
    assert!(matches!(
        layout.result_rows.last(),
        Some(ResultRow::Entry(2, 0))
    ));
    assert!(footer(&palette, &layout).contains("↑ 0 above · ↓ 3 below"));
    palette.select_id("5");
    let layout = palette.layout(surface);
    assert!(matches!(
        layout.result_rows.first(),
        Some(ResultRow::Heading(_))
    ));
    assert!(footer(&palette, &layout).contains("↑ 4 above · ↓ 0 below"));
    palette.select_id("0");
    surface.cols = 80;
    let layout = palette.layout(surface);
    assert!(footer(&palette, &layout).contains("↑ 0 above · ↓ 1 below"));
    surface.rows = 24;
    assert_eq!(palette.layout(surface).result_overflow, None);
}

#[test]
fn overflow_footer_excludes_headings_and_provider_statuses() {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Providers".into());
    palette.set_sections(
        (0..8)
            .map(|i| PluginSection {
                title: format!("Provider {i}"),
                entries: vec![entry(&i.to_string(), "Item", EntryKind::Plugin)],
                semantic: true,
                status: Some("Loading…".into()),
            })
            .collect(),
    );
    let surface = Viewport {
        cols: 80,
        rows: 20,
        ..Default::default()
    };
    let layout = palette.layout(surface);
    // The trailing heading is suppressed; only two entries are visible.
    assert_eq!(layout.result_rows.len(), 6);
    assert!(footer(&palette, &layout).contains("↑ 0 above · ↓ 6 below"));
    palette.selected = 4;
    let layout = palette.layout(surface);
    assert!(footer(&palette, &layout).contains("↑ 3 above · ↓ 3 below"));
    palette.selected_status = Some(7);
    let layout = palette.layout(Viewport { rows: 9, ..surface });
    assert!(layout
        .result_rows
        .iter()
        .all(|row| !matches!(row, ResultRow::Entry(_, _))));
    assert_eq!(layout.result_overflow, None);
    assert!(!footer(&palette, &layout).contains("above"));
}

#[test]
fn overflow_footer_prioritizes_counts_then_description_and_keyboard_hints() {
    let mut palette = overflow_palette(20);
    palette.description = "Detail  \n".repeat(20);
    let surface = Viewport {
        cols: 110,
        rows: 20,
        ..Default::default()
    };
    let layout = palette.layout(surface);
    assert!(
        footer(&palette, &layout).contains("↑ 0 above · ↓ 17 below · Alt+PgUp/Dn Desc 1–4/20 · ↑↓")
    );
    let narrow = palette.layout(Viewport {
        cols: 24,
        ..surface
    });
    assert!(footer(&palette, &narrow).contains("↑ 0 above · ↓ 17 below"));
    assert!(!footer(&palette, &narrow).contains("Alt+"));
    let narrower = palette.layout(Viewport {
        cols: 12,
        ..surface
    });
    assert!(footer(&palette, &narrower).contains("↑ 0 above"));
    palette.busy = true;
    assert!(footer(&palette, &layout).contains("Working…  Esc Cancel"));
    assert!(!footer(&palette, &layout).contains("above"));
    palette.set_error("Validation error".into());
    assert!(footer(&palette, &layout).contains("Validation error"));
    assert!(!footer(&palette, &layout).contains("Working"));
    assert!(!footer(&palette, &layout).contains("above"));
}

#[test]
fn overflow_footer_is_absent_without_a_footer_and_in_forms_and_text_readers() {
    let mut palette = overflow_palette(20);
    let surface = Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    for rows in 0..9 {
        let layout = palette.layout(Viewport { rows, ..surface });
        assert_eq!(layout.footer_y, None);
        assert_eq!(layout.result_overflow, None);
    }
    palette.open_form(Form::new(
        "form".into(),
        "Form".into(),
        vec![],
        "Submit",
        false,
    ));
    let layout = palette.layout(surface);
    assert_eq!(layout.result_overflow, None);
    assert!(!footer(&palette, &layout).contains("above"));
    palette.form = None;
    palette.text = Some("Text  \n".repeat(40));
    let layout = palette.layout(surface);
    assert_eq!(layout.result_overflow, None);
    assert!(!footer(&palette, &layout).contains("above"));
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
                        Some(ResultRow::Entry(_, _))
                    ));
                    palette.click(
                        &layout,
                        Position::new((layout.options_y + row) as i32, layout.rect.x as u16 + 2),
                    );
                    assert_eq!(palette.selected, selected);
                },
                ResultRow::Status(_, _) => panic!("native entries have no plugin status"),
                ResultRow::Divider => panic!("non-agent entries have no divider"),
                ResultRow::Entry(index, _) => {
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
            if let ResultRow::Entry(index, _) = result {
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
                        |row| matches!(row, ResultRow::Entry(index, _) if *index == palette.selected)
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
fn agent_search_prioritizes_exact_projects_and_worktrees_over_fuzzy_scores() {
    for picker in [None, Some(EntryKind::Agent)] {
        let mut palette = CommandPalette::default();
        let mut entries = Vec::new();
        for (id, title, project, directory) in [
            ("weak", "s---w---a---m---p", "yinz", "/work/yinz"),
            ("title", "swamp", "yinz", "/work/yinz"),
            ("project", "Project agent", "SWAMP", "/work/other"),
            ("worktree", "Worktree agent", "yinz", "/work/swamp/"),
            ("path", "Path agent", "yinz", "swamp"),
            ("branch", "Branch agent", "yinz", "/work/other"),
            (
                "display-project",
                "Display project agent",
                "yinz",
                "/work/other",
            ),
        ] {
            let mut entry = entry(id, title, EntryKind::Agent);
            entry.aliases = vec![format!("{project} {directory}")];
            entries.push(entry);
            palette.agent_metadata.insert(
                id.into(),
                std::collections::BTreeMap::from([
                    ("project".into(), project.into()),
                    ("directory".into(), directory.into()),
                    ("state".into(), "idle".into()),
                ]),
            );
        }
        for (id, project, branch) in [
            ("branch", "yinz", "swamp"),
            ("display-project", "swamp", "feature"),
        ] {
            palette.agent_presentations.insert(
                id.into(),
                AgentPresentation {
                    project: project.into(),
                    branch: branch.into(),
                    provider: "codex".into(),
                    state: yinz_types::agent::AgentState::Idle,
                    host: None,
                    pane_id: id.into(),
                    activity: String::new(),
                    hook_flash: false,
                },
            );
        }
        palette.set_entries(entries);
        palette.picker = picker;
        for query in ["swamp", " SwAmP ", "swamp state:idle"] {
            palette.set_query(query.into());
            let found = ids(&palette);
            assert_eq!(found.len(), 7);
            for id in ["project", "worktree", "path", "branch", "display-project"] {
                assert!(found[..5].contains(&id), "{query}: {found:?}");
            }
            assert_eq!(&found[5..], ["title", "weak"]);
        }
        // Full paths also qualify, and structured filters still restrict eligibility.
        palette.set_query("/work/swamp/".into());
        assert_eq!(ids(&palette).first(), Some(&"worktree"));
        palette.set_query("swamp state:busy".into());
        assert!(ids(&palette).is_empty());
        // Without free text, keep the panel order, even with structured filters.
        for query in ["", "state:idle"] {
            palette.set_query(query.into());
            assert_eq!(
                ids(&palette),
                [
                    "weak",
                    "title",
                    "project",
                    "worktree",
                    "path",
                    "branch",
                    "display-project"
                ]
            );
        }
    }
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
fn clearing_searches_preserves_forms_and_navigation_without_restoring_old_queries() {
    let mut palette = palette();
    palette.set_query("new".into());
    palette.select_id("tab");
    palette.open_picker(EntryKind::Pane);
    palette.set_query("console".into());
    palette.open_picker(EntryKind::Command);
    palette.set_query("tab".into());
    palette.push_view(1, "Form".into());
    palette.set_query("form search".into());
    palette.open_form(Form::new(
        "submit".into(),
        "Form".into(),
        vec![
            Field::text("Name", "retained"),
            Field::choice("Placement", &["Tiled", "Floating"]),
        ],
        "Submit",
        false,
    ));
    key(&mut palette, BareKey::Tab);
    key(&mut palette, BareKey::Right);
    palette.push_view(2, "Results".into());
    palette.set_sections(vec![PluginSection {
        title: "Results".into(),
        entries: vec![
            entry("first", "First", EntryKind::Plugin),
            entry("second", "Second", EntryKind::Plugin),
        ],
        semantic: false,
        status: None,
    }]);
    palette.set_query("second".into());
    palette.select_id("second");

    palette.clear_searches();
    assert_eq!(palette.query(), "");
    assert_eq!(ids(&palette), ["first", "second"]);
    assert_eq!(palette.selected_entry().unwrap().id, "first");
    assert!(!palette.selection_explicit);
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.view, 1);
    assert_eq!(palette.query(), "");
    assert_eq!(
        palette.form_values(),
        Some(("submit", vec!["retained".into(), "Floating".into()]))
    );
    assert_eq!(palette.form.as_ref().unwrap().selected, 1);
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.view, 0);
    assert_eq!(palette.query(), "");
    assert_eq!(ids(&palette), ["new", "tab"]);
    assert_eq!(palette.selected_entry().unwrap().id, "new");
    key(&mut palette, BareKey::Esc);
    assert_eq!(palette.query(), "");
    assert_eq!(ids(&palette), ["pane"]);
    key(&mut palette, BareKey::Esc);
    assert!(palette.is_root());
    assert_eq!(palette.query(), "");
    assert_eq!(ids(&palette), ["new", "tab", "pane"]);
    assert_eq!(palette.selected, 0);
    assert!(!palette.selection_explicit);
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
    palette.set_query(" ".into());
    let surface = Viewport {
        cols: 30,
        rows: 10,
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
    assert_eq!(palette.cursor(&layout), Some((10, 3)));
    palette.handle_key(KeyWithModifier::new(BareKey::Char('n')).with_alt_modifier());
    assert_eq!(palette.input.get_text(), "日界  text ");
}

#[test]
fn compact_root_ignores_activation_until_navigation_reveals_results() {
    for navigation in [
        KeyWithModifier::new(BareKey::Up),
        KeyWithModifier::new(BareKey::Down),
        KeyWithModifier::new(BareKey::PageUp),
        KeyWithModifier::new(BareKey::PageDown),
        KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier(),
        KeyWithModifier::new(BareKey::Char('p')).with_ctrl_modifier(),
    ] {
        let mut palette = palette();
        let surface = Viewport {
            cols: 100,
            rows: 40,
            ..Default::default()
        };
        let compact = palette.layout(surface);
        key(&mut palette, BareKey::Enter);
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier()),
            Outcome::None
        );
        assert_eq!(palette.layout(surface).rect, compact.rect);
        assert_eq!(palette.handle_key(navigation), Outcome::None);
        let expanded = palette.layout(surface);
        assert_eq!(expanded.rect.rows, 9);
        assert_eq!(expanded.input_y, compact.input_y);
        assert!(rendered_lines(&palette, &expanded)
            .iter()
            .any(|line| line.contains("New pane")));
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate("new".into())
        );
        key(&mut palette, BareKey::Down);
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate("tab".into())
        );
    }
}

#[test]
fn compact_root_ignores_agent_focus_until_results_are_visible() {
    let mut palette = CommandPalette::default();
    palette
        .agent_metadata
        .insert("agent".into(), Default::default());
    palette.set_entries(vec![entry("agent", "Agent", EntryKind::Agent)]);
    let focus = KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier();
    assert_eq!(palette.handle_key(focus.clone()), Outcome::None);
    key(&mut palette, BareKey::Down);
    assert_eq!(
        palette.handle_key(focus),
        Outcome::FocusAgent("agent".into())
    );
}

#[test]
fn root_browsing_survives_picker_query_edits() {
    for edit in [
        |palette: &mut CommandPalette| key(palette, BareKey::Char('t')),
        |palette: &mut CommandPalette| palette.insert_text("t"),
        |palette: &mut CommandPalette| palette.set_query("t".into()),
    ] {
        let mut palette = palette();
        key(&mut palette, BareKey::Down);
        key(&mut palette, BareKey::Down);
        assert_eq!(palette.selected_entry().unwrap().id, "tab");
        palette.open_picker(EntryKind::Tab);
        edit(&mut palette);
        assert_eq!(palette.query(), "t");
        key(&mut palette, BareKey::Esc);
        assert!(palette.is_root());
        assert_eq!(palette.query(), "");
        assert_eq!(palette.selected_entry().unwrap().id, "tab");
        assert!(!palette.is_compact());
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate("tab".into())
        );
    }
}

#[test]
fn root_browsing_survives_refresh_but_resets_when_query_is_cleared() {
    let mut palette = palette();
    key(&mut palette, BareKey::Down);
    palette.set_entries(vec![entry("tab", "New tab", EntryKind::Command)]);
    assert!(!palette.is_compact());
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("tab".into())
    );
    key(&mut palette, BareKey::Char('t'));
    key(&mut palette, BareKey::Backspace);
    assert!(palette.is_compact());
    key(&mut palette, BareKey::Enter);
}

#[test]
fn compact_root_can_start_browsing_before_entries_arrive() {
    let mut palette = CommandPalette::default();
    key(&mut palette, BareKey::Down);
    assert!(!palette.is_compact());
    key(&mut palette, BareKey::Enter);
    palette.set_entries(vec![entry("new", "New pane", EntryKind::Command)]);
    assert!(!palette.is_compact());
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Activate("new".into())
    );
}

#[test]
fn empty_root_keeps_only_the_framed_input_and_expands_downward() {
    let mut palette = palette();
    let surface = Viewport {
        x: 2,
        y: 3,
        cols: 100,
        rows: 40,
    };
    let compact = palette.layout(surface);
    assert_eq!(
        compact.rect,
        CellRect {
            x: 15,
            y: 7,
            cols: 74,
            rows: 3
        }
    );
    assert_eq!(compact.border, 1);
    assert_eq!(compact.options_rows, 0);
    assert_eq!(compact.footer_y, None);
    let lines = rendered_lines(&palette, &compact);
    assert!(lines[0].starts_with('╭') && lines[2].starts_with('╰'));
    for (query, height) in [("New", 7), ("no matching command", 6)] {
        palette.set_query(query.into());
        let expanded = palette.layout(surface);
        assert_eq!(expanded.rect.rows, height);
        assert_eq!(expanded.rect.y, compact.rect.y);
        assert_eq!(expanded.input_y, compact.input_y);
    }
    palette.set_query(String::new());
    assert_eq!(palette.layout(surface).rect, compact.rect);
    palette.open_picker(EntryKind::Pane);
    assert_eq!(palette.layout(surface).rect.rows, 6);
}

#[test]
fn compact_three_row_palette_keeps_borders() {
    let palette = palette();
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 3,
        ..Default::default()
    });
    assert_eq!(layout.border, 1);
    assert_eq!(layout.options_rows, 0);
    let lines = rendered_lines(&palette, &layout);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with('╭'));
    assert!(lines[1].starts_with("│> "));
    assert!(lines[2].starts_with('╰'));
}

#[test]
fn expanded_palettes_with_three_rows_keep_selected_results_visible() {
    for prepare in [
        |palette: &mut CommandPalette| palette.set_query("new".into()),
        |palette: &mut CommandPalette| key(palette, BareKey::Down),
        |palette: &mut CommandPalette| palette.open_picker(EntryKind::Pane),
    ] {
        let mut palette = palette();
        prepare(&mut palette);
        for _ in 0..palette.matches.len() {
            let layout = palette.layout(Viewport {
                cols: 80,
                rows: 5,
                ..Default::default()
            });
            assert_eq!(layout.border, 0);
            assert_eq!(layout.options_rows, 2);
            let lines = rendered_lines(&palette, &layout);
            let selected = format!("› {}", palette.selected_entry().unwrap().label);
            assert!(
                lines.iter().any(|line| line.contains(&selected)),
                "{selected:?} missing from {lines:?}"
            );
            key(&mut palette, BareKey::Down);
        }
    }
}

#[test]
fn three_row_forms_keep_fields_and_submit_visible() {
    let mut palette = palette();
    palette.open_form(Form::new(
        "form".into(),
        "New pane".into(),
        vec![Field::text("Name", "original")],
        "Create",
        false,
    ));
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 3,
        ..Default::default()
    });
    assert_eq!(layout.border, 0);
    assert_eq!(layout.options_rows, 2);
    let lines = rendered_lines(&palette, &layout);
    assert_eq!(lines[0].trim_end(), "New pane");
    assert_eq!(lines[1].trim_end(), "Name: original");
    assert_eq!(lines[2].trim_end(), "Create");
    assert!(palette.cursor(&layout).is_some());
}

#[test]
fn layout_and_cursor_fit_even_the_smallest_surfaces() {
    let mut palette = CommandPalette::default();
    for query in ["", "界e\u{301}"] {
        palette.set_query(query.repeat(80));
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
                if !query.is_empty() {
                    assert!(layout.rect.rows <= (rows * 3 / 5).max(1).min(rows));
                }
                for chunk in palette.chunks(&layout, &Style::default()) {
                    assert_eq!(chunk.width(), layout.rect.cols);
                    assert!(chunk.y < surface.y + rows);
                }
                if let Some((x, y)) = palette.cursor(&layout) {
                    assert!(layout.rect.contains(x, y));
                }
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
            x: 13,
            y: 3,
            cols: 74,
            rows: 6
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
    palette.set_query(" ".into());
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
    palette.set_query(" ".into());
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
                        characters[layout.border + 2].styles.background
                    );
                }
            }
        }
    }
}

#[test]
fn semantic_spaces_have_a_background_but_alignment_padding_does_not() {
    let mut palette = palette();
    palette.set_query(" ".into());
    for entry in &mut palette.entries {
        entry.shortcut = "Ctrl n".into();
        entry.description = "界".repeat(80);
    }
    for selected in ["new", "pane"] {
        palette.select_id(selected);
        for cols in [50, 80] {
            let layout = palette.layout(Viewport {
                cols,
                rows: 24,
                ..Default::default()
            });
            for chunk in palette.chunks(&layout, &Style::default()) {
                let cells = &chunk.terminal_characters;
                let text: String = cells.iter().map(|cell| cell.character).collect();
                for semantic in [
                    "New pane",
                    "日本語 terminal",
                    "Ctrl n",
                    "Do · Project: None",
                    "Select  Enter",
                ] {
                    if let Some(offset) = text.find(semantic) {
                        let start = text[..offset].chars().count();
                        for cell in &cells[start..start + semantic.chars().count()] {
                            assert!(
                                !matches!(cell.styles.background, None | Some(AnsiCode::Reset)),
                                "{text:?}"
                            );
                        }
                    }
                }
                if !text.contains('›') && text.contains("Ctrl n") {
                    let end = text.find("Ctrl n").unwrap();
                    let padding = text[..end].chars().count() - 1;
                    assert_eq!(cells[padding].character, ' ');
                    assert_eq!(cells[padding].styles.background, Some(AnsiCode::Reset));
                }
            }
        }
    }
    palette.set_query("日本語 ".into());
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let chunks = palette.chunks(&layout, &Style::default());
    let input = &chunks[layout.input_y - layout.rect.y].terminal_characters;
    for cell in &input[layout.border..layout.border + "> 日本語 ".chars().count()] {
        assert!(!matches!(
            cell.styles.background,
            None | Some(AnsiCode::Reset)
        ));
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
                ResultRow::Entry(index, _) => {
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
                ResultRow::Divider => panic!("session entries have no divider"),
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
    assert_eq!(palette.selected_entry().unwrap().id, "inserted");
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_entry().unwrap().id, "unrelated");
    palette.insert_text("pasted");
    assert_eq!(palette.selected_entry().unwrap().id, "inserted");
    palette.push_view(1, "Static".into());
    let mut static_section = section(&["apple", "banana"]);
    static_section.semantic = false;
    palette.set_sections(vec![static_section]);
    palette.insert_text("ban");
    assert_eq!(ids(&palette), ["banana"]);
    palette.back();
    assert_eq!(palette.query(), "newxpasted");
    assert_eq!(palette.selected_entry().unwrap().id, "inserted");
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
            palette.set_query(" ".into());
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
    assert_eq!(palette.selected_entry().unwrap().id, "first");
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
            ResultRow::Entry(index, _) => {
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate(palette.entries[palette.matches[*index]].id.clone())
                );
            },
            ResultRow::Status(section, _) => {
                assert_eq!(palette.selected_status, Some(*section));
                key(&mut palette, BareKey::Enter);
            },
            ResultRow::Heading(_) | ResultRow::Divider => {},
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
    assert_eq!(palette.selected_entry().unwrap().id, "second");
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_entry().unwrap().id, "inserted");
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected_entry().unwrap().id, "first");
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
    assert_eq!(palette.selected_entry().unwrap().id, "event:1");
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
                ResultRow::Entry(index, _) => palette.entries[palette.matches[*index]].id == id,
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
        palette.set_query("provideronly".into());
        assert_eq!(ids(&palette), if nested { vec![] } else { vec!["pdf"] });
        for query in ["unrelated", "descriptiononly", "reader document"] {
            palette.set_query(query.into());
            assert!(ids(&palette).is_empty(), "{query}");
            assert!(palette.result_rows(10).is_empty());
        }
    }
}

#[test]
fn provider_titles_rank_static_and_dynamic_results_with_commands() {
    for semantic in [false, true] {
        let mut palette = CommandPalette::default();
        palette.set_entries(vec![
            entry("weak", "n--------p", EntryKind::Command),
            entry("equal", "np", EntryKind::Command),
        ]);
        palette.set_sections(vec![PluginSection {
            title: "np".into(),
            entries: vec![
                entry("first", "Unrelated", EntryKind::Plugin),
                entry("second", "Other", EntryKind::Plugin),
            ],
            semantic,
            status: None,
        }]);
        palette.set_query("np".into());
        assert_eq!(ids(&palette), ["equal", "first", "second", "weak"]);
        palette.set_query("other".into());
        assert_eq!(
            ids(&palette),
            if semantic {
                vec!["second", "first"]
            } else {
                vec!["second"]
            }
        );
    }
}

#[test]
fn plugin_palette_scores_interleave_with_native_and_preserve_ties_and_geometry() {
    for (aliases, semantic) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut palette = CommandPalette::default();
        palette.set_entries(vec![
            entry("native-low", "n--------p", EntryKind::Command),
            entry("native-high", "np", EntryKind::Command),
        ]);
        let plugin_entry = |id, label| {
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
                semantic,
                status: None,
                entries: vec![
                    plugin_entry("a-low", "n--------p"),
                    plugin_entry("a-high", "np"),
                ],
            },
            PluginSection {
                title: "B".into(),
                semantic,
                status: Some("Loading…".into()),
                entries: vec![
                    plugin_entry("b-high", "np"),
                    plugin_entry("b-low", "n--------p"),
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
                if let ResultRow::Entry(i, _) = r {
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
                    .any(|r| matches!(r, ResultRow::Entry(i, _) if *i == selected)));
                let mut heading = None;
                for (row, result) in layout.result_rows.iter().enumerate() {
                    match result {
                        ResultRow::Heading(title) => heading = Some(title.as_str()),
                        ResultRow::Entry(index, _) => {
                            if let Some(section) = palette.entry_sections[palette.matches[*index]]
                                && layout.result_rows.len() > 1
                            {
                                assert_eq!(heading, Some(palette.sections[section].title.as_str()));
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
        if semantic {
            assert_eq!(
                ids(&palette),
                [
                    "a-low",
                    "a-high",
                    "b-high",
                    "b-low",
                    "dynamic-z",
                    "dynamic-a",
                    "dynamic-new"
                ]
            );
        } else {
            assert_eq!(ids(&palette), ["dynamic-z", "dynamic-a", "dynamic-new"]);
        }
        assert_eq!(
            palette
                .result_rows(20)
                .iter()
                .any(|r| matches!(r, ResultRow::Heading(t) if t == "A")),
            semantic
        );
    }
}

#[test]
fn theme_picker_filters_selects_and_previews_with_mouse() {
    let mut palette = CommandPalette::default();
    palette.set_entries(vec![
        entry("command", "Change theme…", EntryKind::Command),
        entry("theme:a", "alpha", EntryKind::Theme),
        entry("theme:b", "beta", EntryKind::Theme),
    ]);
    assert_eq!(ids(&palette), vec!["command"]);
    palette.open_picker(EntryKind::Theme);
    assert_eq!(ids(&palette), vec!["theme:a", "theme:b"]);
    palette.select_id("theme:b");
    assert_eq!(palette.selected_entry().unwrap().label, "beta");
    key(&mut palette, BareKey::Up);
    assert_eq!(palette.selected_entry().unwrap().label, "alpha");
    let layout = palette.layout(Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 24,
    });
    assert!(rendered_lines(&palette, &layout)
        .iter()
        .any(|line| line.contains("↑↓ Preview · Enter Save · Esc Back")));
    palette.click(
        &layout,
        Position::new((layout.options_y + 1) as i32, (layout.rect.x + 2) as u16),
    );
    assert_eq!(palette.selected_entry().unwrap().label, "beta");
    palette.set_query("alpha".into());
    assert_eq!(palette.selected_entry().unwrap().label, "alpha");
    palette.set_query("no-such-theme".into());
    assert!(palette.selected_entry().is_none());
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::None
    );
    key(&mut palette, BareKey::Esc);
    assert!(palette.is_root());
}

fn wrapped_palette(labels: &[&str]) -> CommandPalette {
    let mut palette = CommandPalette::default();
    palette.push_view(42, "Search documents".into());
    palette.set_sections(vec![PluginSection {
        title: "Documents".into(),
        semantic: true,
        status: None,
        entries: labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let mut entry = entry(&index.to_string(), label, EntryKind::Plugin);
                entry.description = "body snippet".into();
                entry
            })
            .collect(),
    }]);
    palette
}

#[test]
fn entry_labels_wrap_twice_and_only_complete_titles_get_snippets() {
    for (label, expected) in [
        ("#1 Short", vec!["#1 Short — body snippet"]),
        ("#2 Short title", vec!["#2 Short", "  title — body snippet"]),
        ("#3 abcdefghijklmnop", vec!["#3", "  abcde…"]),
        ("#4 日本語日本語", vec!["#4", "  日本…"]),
        ("#5 e\u{301} caf", vec!["#5 e\u{301} caf — body snippet"]),
        ("#6 [archived]", vec!["#6", "  [arch…"]),
        ("#7 12345678", vec!["#7", "  12345…"]),
        (
            "#8 Short e\u{301} caf",
            vec!["#8 Short", "  e\u{301} caf — body snippet"],
        ),
    ] {
        let palette = wrapped_palette(&[label]);
        assert_eq!(texts(&palette.entry_lines(0, 10, 2)), expected, "{label}");
    }
    let palette = wrapped_palette(&["#1 Short title"]);
    assert_eq!(texts(&palette.entry_lines(0, 10, 1)), vec!["#1 Shor…"]);
}

#[test]
fn continuation_indent_leaves_text_space_in_narrow_palettes() {
    let palette = wrapped_palette(&["abcdefghi"]);
    for (width, expected) in [
        (0, vec![""]),
        (1, vec!["a", "…"]),
        (2, vec!["a", "…"]),
        (3, vec!["a", "…"]),
        (4, vec!["ab", " …"]),
        (5, vec!["abc", "  …"]),
        (6, vec!["abcd", "  e…"]),
    ] {
        let lines = texts(&palette.entry_lines(0, width, 2));
        assert_eq!(lines, expected, "width {width}");
        assert!(lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= width));
    }
}

#[test]
fn wrapped_results_share_selection_clicks_and_scroll_geometry() {
    let mut palette = wrapped_palette(&[
        "#1 Short",
        "#2 A title that wraps onto two lines",
        "#3 A much longer title that needs more than two lines and must be truncated",
        "#4 日本語の長いタイトルとさらに続く文字列",
        "#5 Last",
    ]);
    for cols in [3, 8, 24, 64] {
        for rows in [4, 5, 6, 7, 8, 11] {
            let surface = Viewport {
                cols,
                rows,
                ..Default::default()
            };
            for selected in 0..5 {
                palette.selected = selected;
                let layout = palette.layout(surface);
                assert!(layout.result_rows.len() <= layout.options_rows);
                if layout.options_rows == 0 {
                    continue;
                }
                assert!(layout
                    .result_rows
                    .iter()
                    .any(|row| palette.row_selected(row)));
                let lines = rendered_lines(&palette, &layout);
                let chunks = palette.chunks(&layout, &Style::default());
                let mut first_style = None;
                for (row, result) in layout.result_rows.iter().enumerate() {
                    if let ResultRow::Entry(index, line) = result {
                        let y = layout.options_y + row;
                        let painted = &lines[y - layout.rect.y];
                        assert!(unicode_width::UnicodeWidthStr::width(painted.as_str()) <= cols);
                        if *index == selected {
                            let style = chunks[y - layout.rect.y].terminal_characters
                                [layout.border]
                                .styles
                                .clone();
                            if let Some(first) = first_style {
                                assert_eq!(style, first);
                            }
                            first_style = Some(style);
                        }
                        if *line > 0 {
                            if cols >= 8 {
                                assert_eq!(
                                    painted
                                        .chars()
                                        .skip(layout.border)
                                        .take(4)
                                        .collect::<String>(),
                                    "    "
                                );
                                let characters = &chunks[y - layout.rect.y].terminal_characters;
                                for offset in 1..4 {
                                    assert_eq!(
                                        characters[layout.border + offset].styles,
                                        characters[layout.border].styles
                                    );
                                }
                            }
                            assert!(
                                matches!(layout.result_rows.get(row - 1), Some(ResultRow::Entry(previous, 0)) if previous == index)
                            );
                        }
                        palette.click(
                            &layout,
                            Position::new(y as i32, (layout.rect.x + layout.border) as u16),
                        );
                        assert_eq!(palette.selected, *index);
                        assert_eq!(
                            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                            Outcome::Activate(index.to_string())
                        );
                        if layout.options_rows == 1 && *index == 2 && cols > 3 {
                            assert!(painted.contains('…'));
                            assert!(!painted.contains("body snippet"));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn page_navigation_counts_wrapped_rows() {
    let mut palette = wrapped_palette(&[
        "#1 Long title here",
        "#2 Long title here",
        "#3 Long title here",
        "#4 Long title here",
    ]);
    let surface = Viewport {
        cols: 16,
        rows: 15,
        ..Default::default()
    };
    let layout = palette.layout(surface);
    assert_eq!(layout.options_rows, 4);
    key(&mut palette, BareKey::Down);
    assert_eq!(palette.selected, 1);
    key(&mut palette, BareKey::Up);
    assert_eq!(palette.selected, 0);
    key(&mut palette, BareKey::PageDown);
    assert_eq!(palette.selected, 1);
    key(&mut palette, BareKey::PageUp);
    assert_eq!(palette.selected, 0);
    key(&mut palette, BareKey::PageUp);
    assert_eq!(palette.selected, 0);
    key(&mut palette, BareKey::PageDown);
    assert_eq!(palette.selected, 1);
    key(&mut palette, BareKey::PageDown);
    assert_eq!(palette.selected, 2);
    key(&mut palette, BareKey::PageUp);
    assert_eq!(palette.selected, 0);
}

#[test]
fn all_result_kinds_wrap_in_root_and_picker_views() {
    for kind in [
        EntryKind::Command,
        EntryKind::Plugin,
        EntryKind::Pane,
        EntryKind::Tab,
        EntryKind::Session,
        EntryKind::ExitedSession,
        EntryKind::Project,
        EntryKind::Theme,
        EntryKind::Agent,
        EntryKind::WarpTabSession,
        EntryKind::WarpPaneSession,
        EntryKind::WarpTab,
        EntryKind::WarpPane,
    ] {
        let mut palette = CommandPalette::default();
        palette.set_query(" ".into());
        palette
            .agent_metadata
            .insert("result".into(), Default::default());
        let result = entry(
            "result",
            "A long title with 日本語 that needs more than two rows of space",
            kind,
        );
        if kind == EntryKind::Plugin {
            palette.set_sections(vec![PluginSection {
                title: "Provider".into(),
                entries: vec![result],
                semantic: true,
                status: None,
            }]);
        } else {
            palette.set_entries(vec![result]);
        }
        for picker in [None, Some(kind)] {
            if kind == EntryKind::Plugin && picker.is_some() {
                let sections = palette.sections.clone();
                palette.push_view(1, "Provider".into());
                palette.set_sections(sections);
            } else {
                palette.picker = picker;
                palette.rematch();
            }
            if palette.matches.is_empty() {
                assert!(picker.is_none(), "{kind:?}");
                continue;
            }
            for rows in [12, 19] {
                let layout = palette.layout(Viewport {
                    cols: 24,
                    rows,
                    ..Default::default()
                });
                let result_rows: Vec<_> = layout
                    .result_rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| matches!(row, ResultRow::Entry(_, _)))
                    .collect();
                let expected = if layout.options_rows == 2 && palette.entry_heading(0).is_some() {
                    1
                } else {
                    2
                };
                assert_eq!(result_rows.len(), expected, "{kind:?}, {picker:?}, {rows}");
                let lines = rendered_lines(&palette, &layout);
                let (last_row, _) = result_rows.last().unwrap();
                assert!(lines[layout.options_y + last_row - layout.rect.y].contains('…'));
                if expected == 2 {
                    assert!(lines[layout.options_y + last_row - layout.rect.y]
                        .chars()
                        .skip(layout.border)
                        .take(4)
                        .all(|c| c == ' '));
                }
                palette.click(
                    &layout,
                    Position::new(
                        (layout.options_y + last_row) as i32,
                        (layout.rect.x + layout.border) as u16,
                    ),
                );
                assert_eq!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate("result".into())
                );
            }
        }
    }
}

#[test]
fn wrapped_shortcuts_and_descriptions_share_the_available_columns() {
    let mut palette = CommandPalette::default();
    palette.set_query(" ".into());
    let mut result = entry(
        "result",
        "Alpha bravo charlie delta echo",
        EntryKind::Command,
    );
    result.shortcut = "Ctrl n".into();
    result.description = "body snippet".into();
    palette.set_entries(vec![result]);
    let layout = palette.layout(Viewport {
        cols: 26,
        rows: 19,
        ..Default::default()
    });
    assert_eq!(layout.rect.rows, 7);
    let lines = rendered_lines(&palette, &layout);
    let first = layout.options_y - layout.rect.y;
    assert!(lines[first].contains("› Alpha bravo"));
    assert!(lines[first].contains("Ctrl n"));
    assert!(lines[first + 1].contains("    charlie delt…"));
    assert!(!lines[first + 1].contains("Ctrl n"));
    assert!(!lines.iter().any(|line| line.contains("body snippet")));
    palette.entries[0].label = "Alpha bravo charlie".into();
    assert_eq!(
        texts(&palette.entry_lines(0, 24, 2)),
        vec!["Alpha bravo", "  charlie — body snippet"]
    );
}

#[test]
fn page_navigation_moves_past_two_line_results_without_a_section_heading() {
    let mut palette = CommandPalette::default();
    palette.set_query(" ".into());
    palette.set_entries(
        (0..3)
            .map(|i| {
                entry(
                    &i.to_string(),
                    "A long title that wraps",
                    EntryKind::Command,
                )
            })
            .collect(),
    );
    let surface = Viewport {
        cols: 16,
        rows: 12,
        ..Default::default()
    };
    assert_eq!(palette.layout(surface).options_rows, 2);
    for selected in [1, 2] {
        key(&mut palette, BareKey::PageDown);
        assert_eq!(palette.selected, selected);
        let layout = palette.layout(surface);
        assert_eq!(layout.result_rows.len(), 2);
        assert!(layout
            .result_rows
            .iter()
            .all(|row| palette.row_selected(row)));
    }
    key(&mut palette, BareKey::PageUp);
    assert_eq!(palette.selected, 1);
}

#[test]
fn wrapped_history_pages_stay_in_the_focused_region() {
    let mut palette = CommandPalette {
        metadata: Some("Run history".into()),
        ..Default::default()
    };
    palette.set_sections(vec![PluginSection {
        title: "Timeline".into(),
        semantic: true,
        status: None,
        entries: std::iter::once(entry("resume", "Resume", EntryKind::Plugin))
            .chain((0..5).map(|i| {
                entry(
                    &format!("event:{i}"),
                    "A long event title that wraps onto two rows",
                    EntryKind::Plugin,
                )
            }))
            .collect(),
    }]);
    key(&mut palette, BareKey::Tab);
    palette.layout(Viewport {
        cols: 24,
        rows: 13,
        ..Default::default()
    });
    key(&mut palette, BareKey::PageDown);
    assert_eq!(palette.selected_entry().unwrap().id, "event:1");
    key(&mut palette, BareKey::PageUp);
    assert_eq!(palette.selected_entry().unwrap().id, "event:0");
    key(&mut palette, BareKey::Tab);
    for direction in [BareKey::PageDown, BareKey::PageUp] {
        key(&mut palette, direction);
        assert_eq!(palette.selected_entry().unwrap().id, "resume");
    }
}

#[test]
fn secondary_form_actions_receive_edited_values_and_choices_are_searchable() {
    let mut palette = CommandPalette::default();
    let mut form = Form::new(
        "submit".into(),
        "Create".into(),
        vec![
            Field::text("Name", "initial"),
            Field::choice("Layout", &["default", "compact", "custom"]),
        ],
        "Create",
        false,
    );
    form.actions.push(("configure".into(), "Configure…".into()));
    palette.open_form(form);
    palette.insert_text(" edited");
    palette.handle_key(KeyWithModifier::new(BareKey::Enter));
    for c in "compact".chars() {
        palette.handle_key(KeyWithModifier::new(BareKey::Char(c)));
    }
    palette.handle_key(KeyWithModifier::new(BareKey::Enter));
    palette.handle_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Submit {
            id: "configure".into(),
            values: vec!["initial edited".into(), "compact".into()]
        }
    );
}

#[test]
fn late_results_select_best_until_navigation_and_reset_on_query_change() {
    for plugin_first in [false, true] {
        let mut palette = CommandPalette::default();
        palette.set_query("pdf".into());
        let native = vec![entry("native", "pdf", EntryKind::Command)];
        let sections = vec![PluginSection {
            title: "Plugin".into(),
            entries: vec![entry("plugin", "Pretty document finder", EntryKind::Plugin)],
            semantic: false,
            status: None,
        }];
        if plugin_first {
            palette.set_sections(sections.clone());
            palette.set_entries(native.clone());
        } else {
            palette.set_entries(vec![entry(
                "weak",
                "Pretty document finder",
                EntryKind::Command,
            )]);
            palette.set_sections(vec![PluginSection {
                entries: vec![entry("plugin", "pdf", EntryKind::Plugin)],
                ..sections[0].clone()
            }]);
        }
        assert_eq!(palette.selected_entry().unwrap().id, ids(&palette)[0]);
        key(&mut palette, BareKey::Down);
        let chosen = palette.selected_entry().unwrap().id.clone();
        key(&mut palette, BareKey::Left);
        let mut updated = palette.sections.clone();
        updated[0].status = Some("updated".into());
        palette.set_sections(updated);
        assert_eq!(palette.selected_entry().unwrap().id, chosen);
        palette.set_query("pdf".into());
        assert_eq!(palette.selected_entry().unwrap().id, ids(&palette)[0]);
    }
}

#[test]
fn late_results_restore_intent_and_resume_automatic_selection_after_removal() {
    for explicit in [false, true] {
        for picker in [false, true] {
            let mut palette = palette();
            palette.set_query("new".into());
            if explicit {
                let layout = palette.layout(Viewport {
                    cols: 80,
                    rows: 24,
                    ..Default::default()
                });
                palette.click(
                    &layout,
                    Position::new((layout.options_y + 1) as i32, (layout.rect.x + 2) as u16),
                );
                assert_eq!(palette.selected_entry().unwrap().id, "tab");
            }
            if picker {
                palette.open_picker(EntryKind::Pane);
            } else {
                palette.push_view(1, "Nested".into());
            }
            palette.back();
            let mut entries = palette.native_entries.clone();
            entries.insert(0, entry("best", "new", EntryKind::Command));
            palette.set_entries(entries.clone());
            assert_eq!(
                palette.selected_entry().unwrap().id,
                if explicit { "tab" } else { "best" }
            );
            entries.retain(|entry| entry.id != "tab");
            palette.set_entries(entries.clone());
            assert_eq!(palette.selected_entry().unwrap().id, "best");
            entries.insert(0, entry("new-best", "new", EntryKind::Command));
            palette.set_entries(entries);
            assert_eq!(palette.selected_entry().unwrap().id, "new-best");
        }
    }
}

#[test]
fn configuration_fields_preserve_multiline_capture_keys_and_validate_numbers() {
    let mut palette = palette();
    let mut field = Field::text("Command", "");
    field.input_options = Some(yinz_types::command_palette::InputOptions {
        kind: "multiline".into(),
        ..Default::default()
    });
    palette.open_form(Form::new(
        "form".into(),
        "Configuration".into(),
        vec![field],
        "Apply",
        false,
    ));
    palette.insert_text("one\ntwo\tthree");
    assert_eq!(
        palette.form.as_ref().unwrap().fields[0].text_value(),
        "one\ntwo\tthree"
    );
    palette.handle_key(KeyWithModifier::new(BareKey::Enter).with_shift_modifier());
    assert!(palette.form.as_ref().unwrap().fields[0]
        .text_value()
        .ends_with('\n'));
    let mut field = Field::text("Key", "");
    field.input_options = Some(yinz_types::command_palette::InputOptions {
        kind: "key".into(),
        ..Default::default()
    });
    palette.open_form(Form::new(
        "form".into(),
        "Configuration".into(),
        vec![field],
        "Apply",
        false,
    ));
    key(&mut palette, BareKey::F(2));
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Esc)),
        Outcome::None
    );
    assert!(!palette.form.as_ref().unwrap().fields[0]
        .text_value()
        .is_empty());
    let mut field = Field::text("Opacity", "101");
    field.input_options = Some(yinz_types::command_palette::InputOptions {
        kind: "number".into(),
        minimum: Some(0),
        maximum: Some(100),
        ..Default::default()
    });
    palette.open_form(Form::new(
        "form".into(),
        "Configuration".into(),
        vec![field],
        "Apply",
        false,
    ));
    key(&mut palette, BareKey::Enter);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::None
    );
    assert!(palette.error.as_ref().unwrap().contains("Opacity"));
}

#[test]
fn configuration_forms_keep_the_cursor_when_help_is_visible() {
    for kind in ["number", "key", "multiline"] {
        let mut palette = palette();
        let mut field = Field::text("Value", "1");
        field.input_options = Some(yinz_types::command_palette::InputOptions {
            kind: kind.into(),
            help: "Edit this value".into(),
            ..Default::default()
        });
        palette.open_form(Form::new(
            "form".into(),
            "Configuration".into(),
            vec![field],
            "Apply",
            false,
        ));
        let surface = Viewport {
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        assert!(palette.metadata.is_some());
        assert!(palette.cursor(&palette.layout(surface)).is_some(), "{kind}");
        palette.form = None;
        assert!(palette.cursor(&palette.layout(surface)).is_none());
    }
}

#[test]
fn view_descriptions_wrap_without_changing_list_navigation_or_leaking_on_back() {
    for form in [false, true] {
        for cols in [32, 80] {
            let mut palette = palette();
            palette.push_view(1, "Settings".into());
            palette.description = "Choose which corner of the screen shows notifications.".into();
            if form {
                let mut number = Field::text("Timeout", "10");
                number.input_options = Some(yinz_types::command_palette::InputOptions {
                    kind: "number".into(),
                    ..Default::default()
                });
                palette.open_form(Form::new(
                    "edit".into(),
                    "Notification position".into(),
                    vec![
                        Field::choice("Position", &["top-left", "bottom-right"]),
                        number,
                    ],
                    "Apply",
                    false,
                ));
            } else {
                palette.set_sections(vec![PluginSection {
                    title: "Settings".into(),
                    entries: vec![entry("edit", "Edit setting", EntryKind::Plugin)],
                    semantic: false,
                    status: None,
                }]);
                palette.insert_text("Edit");
            }
            assert!(
                palette.metadata.is_none(),
                "empty input help needs no details area"
            );
            let layout = palette.layout(Viewport {
                cols,
                rows: 24,
                ..Default::default()
            });
            let lines = rendered_lines(&palette, &layout);
            assert!(lines
                .iter()
                .any(|line| line.contains("Choose which corner")));
            assert!(lines.iter().any(|line| line.contains("notifications.")));
            assert!(!lines.iter().any(|line| line.contains("Agent info")));
            assert!(layout.options_rows > 0);
            if cols == 32 {
                assert!(layout.description_lines.len() > 1);
            }
            if !form {
                assert_eq!(ids(&palette), ["edit"]);
                assert!(matches!(
                    palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
                    Outcome::Activate(_)
                ));
            }
            palette.push_view(2, "Next".into());
            assert!(palette.description.is_empty());
            key(&mut palette, BareKey::Esc);
            assert!(palette.description.starts_with("Choose which corner"));
            key(&mut palette, BareKey::Esc);
            assert!(palette.description.is_empty());
        }
    }
}

#[test]
fn overflowing_view_descriptions_scroll_without_changing_actions_or_forms() {
    for form in [false, true] {
        let mut palette = palette();
        palette.push_view(1, "Clipboard access".into());
        palette.description = "Allow any pane program, including remote programs, to read the clipboard through OSC 52 without asking.".into();
        if form {
            palette.open_form(Form::new(
                "edit".into(),
                "Clipboard access".into(),
                vec![Field::text("Value", "unchanged")],
                "Apply",
                false,
            ));
        } else {
            palette.set_sections(vec![PluginSection {
                title: String::new(),
                entries: vec![
                    entry("edit", "Edit", EntryKind::Command),
                    entry("reset", "Reset", EntryKind::Command),
                ],
                semantic: false,
                status: None,
            }]);
        }
        let surface = Viewport {
            cols: 32,
            rows: if form { 10 } else { 17 },
            ..Default::default()
        };
        let first = rendered_lines(&palette, &palette.layout(surface)).join("\n");
        assert!(first.contains("Allow any pane program"));
        assert!(!first.contains("without asking."));
        if form {
            assert!(first.contains("Alt+PgUp/Dn Desc 1–3/4"));
        } else {
            assert!(first.contains("↑ 0 above · ↓ 1 below · Alt+"));
        }

        for _ in 0..3 {
            assert_eq!(
                palette.handle_key(KeyWithModifier::new(BareKey::PageDown).with_alt_modifier()),
                Outcome::None
            );
            let layout = palette.layout(surface);
            let text = rendered_lines(&palette, &layout).join("\n");
            assert!(text.contains("without asking."));
            if form {
                assert!(text.contains("Alt+PgUp/Dn Desc 2–4/4"));
            } else {
                assert!(text.contains("↑ 0 above · ↓ 1 below · Alt+"));
                assert_eq!(palette.description_offset.get(), 1);
            }
            assert!(layout.options_rows > 0);
        }
        if form {
            assert_eq!(palette.form_values().unwrap().1, ["unchanged"]);
            assert_eq!(palette.form.as_ref().unwrap().selected, 0);
        } else {
            assert_eq!(palette.selected_entry().unwrap().id, "edit");
        }

        palette.push_view(2, "Next".into());
        assert_eq!(palette.description_offset.get(), 0);
        key(&mut palette, BareKey::Esc);
        assert!(rendered_lines(&palette, &palette.layout(surface))
            .join("\n")
            .contains("without asking."));
        for _ in 0..3 {
            palette.handle_key(KeyWithModifier::new(BareKey::PageUp).with_alt_modifier());
            assert_eq!(
                rendered_lines(&palette, &palette.layout(surface)).join("\n"),
                first
            );
        }
        key(&mut palette, BareKey::Down);
        if form {
            assert_eq!(palette.form.as_ref().unwrap().selected, 1);
        } else {
            assert_eq!(palette.selected_entry().unwrap().id, "reset");
        }
        key(&mut palette, BareKey::Esc);
        assert!(palette.description.is_empty());
        assert_eq!(palette.description_offset.get(), 0);
    }
}

#[test]
fn short_surfaces_keep_controls_visible_with_descriptions() {
    for rows in [5, 6] {
        for form in [false, true] {
            let mut palette = palette();
            palette.description = "Choose a value.".into();
            let labels = if form {
                palette.open_form(Form::new(
                    "edit".into(),
                    "Configuration".into(),
                    vec![Field::text("Value", "unchanged")],
                    "Apply",
                    false,
                ));
                ["unchanged", "Apply"]
            } else {
                ["New pane", "New tab"]
            };
            let surface = Viewport {
                cols: 80,
                rows: if form { rows } else { (rows * 5).div_ceil(3) },
                ..Default::default()
            };
            for label in labels {
                let layout = palette.layout(surface);
                assert_eq!(layout.options_rows, 1, "{rows} rows, form={form}");
                assert!(layout.description_lines.is_empty());
                let lines = rendered_lines(&palette, &layout);
                assert!(
                    lines[layout.options_y - layout.rect.y].contains(label),
                    "{rows} rows, form={form}: {lines:?}"
                );
                key(&mut palette, BareKey::Down);
            }
        }
    }
}

#[test]
fn description_scrolling_clamps_on_resize_and_fits_short_surfaces() {
    let mut palette = palette();
    palette.description = "one  \ntwo  \nthree  \nfour  \nfive".into();
    for rows in 0..=10 {
        let surface = Viewport {
            x: 3,
            y: 2,
            cols: 32,
            rows,
        };
        let layout = palette.layout(surface);
        let available = layout
            .footer_y
            .unwrap_or(layout.rect.y + layout.rect.rows - layout.border);
        assert!(layout.description_lines.is_empty() || layout.metadata_y <= available);
        assert_eq!(
            palette.chunks(&layout, &Style::default()).len(),
            layout.rect.rows
        );
        if available.saturating_sub(layout.description_y) > 1 {
            assert!(!layout.description_lines.is_empty(), "{rows} rows");
            let mut seen = layout.description_lines.clone();
            for _ in 0..5 {
                palette.handle_key(KeyWithModifier::new(BareKey::PageDown).with_alt_modifier());
                seen.extend(palette.layout(surface).description_lines);
            }
            assert!(seen.iter().any(|line| line.text == "five"), "{rows} rows");
        } else {
            assert!(layout.description_lines.is_empty(), "{rows} rows");
        }
    }
    let large = Viewport {
        cols: 80,
        rows: 40,
        ..Default::default()
    };
    let layout = palette.layout(large);
    assert_eq!(
        texts(&layout.description_lines),
        ["one", "two", "three", "four", "five"]
    );
    assert_eq!(palette.description_offset.get(), 0);
    assert!(!rendered_lines(&palette, &layout)
        .join("\n")
        .contains("Alt+PgUp/Dn"));
    palette.handle_key(KeyWithModifier::new(BareKey::PageDown).with_alt_modifier());
    assert_eq!(palette.description_offset.get(), 0);

    palette.description = "A description that wraps across several lines at narrow widths.".into();
    let narrow = Viewport {
        cols: 12,
        rows: 10,
        ..Default::default()
    };
    palette.layout(narrow);
    for _ in 0..5 {
        palette.handle_key(KeyWithModifier::new(BareKey::PageDown).with_alt_modifier());
    }
    let layout = palette.layout(large);
    assert_eq!(
        texts(&layout.description_lines),
        [palette.description.clone()]
    );
    assert_eq!(palette.description_offset.get(), 0);
}

#[test]
fn description_scrolling_preserves_metadata_navigation_and_message_priority() {
    for form in [false, true] {
        let mut palette = palette();
        if form {
            palette.open_form(Form::new(
                "confirm".into(),
                "Confirm".into(),
                vec![],
                "Apply",
                true,
            ));
        }
        palette.description = (1..=12).map(|i| format!("Description {i}  \n")).collect();
        palette.metadata = Some((1..=12).map(|i| format!("Detail {i}  \n")).collect());
        let surface = Viewport {
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        palette.layout(surface);
        key(&mut palette, BareKey::PageDown);
        let metadata_offset = palette.metadata_offset;
        assert!(metadata_offset > 0);
        palette.handle_key(KeyWithModifier::new(BareKey::PageDown).with_alt_modifier());
        let layout = palette.layout(surface);
        assert_eq!(palette.metadata_offset, metadata_offset);
        assert!(palette.description_offset.get() > 0);
        let description_offset = palette.description_offset.get();
        key(&mut palette, BareKey::PageUp);
        assert_eq!(palette.metadata_offset, 0);
        assert_eq!(palette.description_offset.get(), description_offset);

        palette.busy = true;
        palette.handle_key(KeyWithModifier::new(BareKey::PageUp).with_alt_modifier());
        assert_eq!(palette.description_offset.get(), description_offset);
        let text = rendered_lines(&palette, &layout).join("\n");
        assert!(text.contains("Working…"));
        assert!(!text.contains("Alt+PgUp/Dn"));
        palette.busy = false;
        palette.set_error("Validation error".into());
        let text = rendered_lines(&palette, &layout).join("\n");
        assert!(text.contains("Validation error"));
        assert!(!text.contains("Alt+PgUp/Dn"));
    }
}

#[test]
fn description_scroll_shortcuts_remain_capturable_as_form_values() {
    let mut palette = palette();
    palette.description = "one\ntwo\nthree\nfour".into();
    let mut field = Field::text("Key", "");
    field.input_options = Some(yinz_types::command_palette::InputOptions {
        kind: "key".into(),
        ..Default::default()
    });
    palette.open_form(Form::new(
        "edit".into(),
        "Keybinding".into(),
        vec![field],
        "Apply",
        false,
    ));
    palette.layout(Viewport {
        cols: 32,
        rows: 10,
        ..Default::default()
    });
    key(&mut palette, BareKey::F(2));
    let shortcut = KeyWithModifier::new(BareKey::PageDown).with_alt_modifier();
    palette.handle_key(shortcut.clone());
    assert_eq!(palette.form_values().unwrap().1, [shortcut.to_string()]);
    assert_eq!(palette.description_offset.get(), 0);
}

#[test]
fn result_height_tracks_content_updates_and_viewport_cap() {
    let mut palette = CommandPalette::default();
    let mut surface = Viewport {
        x: 2,
        y: 3,
        cols: 100,
        rows: 40,
    };
    palette.set_query("result".into());
    assert_eq!(palette.layout(surface).rect.rows, 6);
    let entries: Vec<_> = (0..40)
        .map(|i| entry(&i.to_string(), &format!("result {i}"), EntryKind::Command))
        .collect();
    palette.set_entries(entries[..8].to_vec());
    let layout = palette.layout(surface);
    assert_eq!(layout.rect.rows, 13);
    assert_eq!(layout.options_rows, 8);
    assert_eq!(layout.rect.y, 7);
    palette.set_entries(entries);
    let layout = palette.layout(surface);
    assert_eq!(layout.rect.rows, 24);
    assert_eq!(layout.options_rows, 19);
    key(&mut palette, BareKey::PageDown);
    let layout = palette.layout(surface);
    assert!(layout
        .result_rows
        .iter()
        .any(|row| matches!(row, ResultRow::Entry(i, _) if *i == palette.selected)));
    surface.rows = 23;
    assert_eq!(palette.layout(surface).rect.rows, 13);
    palette.set_query("result 39".into());
    assert_eq!(palette.layout(surface).rect.rows, 6);
    palette.set_query("missing".into());
    let layout = palette.layout(surface);
    assert_eq!(layout.rect.rows, 6);
    assert!(rendered_lines(&palette, &layout)
        .iter()
        .any(|line| line.contains("No matches")));
    palette.set_query(String::new());
    assert_eq!(palette.layout(surface).rect.rows, 3);
}

#[test]
fn result_height_counts_headings_wrapping_and_details() {
    let mut palette = CommandPalette::default();
    palette.set_entries(vec![
        entry("one", &"result ".repeat(20), EntryKind::Pane),
        entry("two", "result two", EntryKind::Tab),
    ]);
    palette.set_query("result".into());
    let surface = Viewport {
        cols: 50,
        rows: 60,
        ..Default::default()
    };
    let layout = palette.layout(surface);
    assert_eq!(layout.rect.rows, 10);
    assert_eq!(layout.result_rows.len(), 5);
    assert_eq!(layout.options_rows, 5);
    palette.description = "Choose a **result**".into();
    palette.metadata = Some("**Details**".into());
    let layout = palette.layout(surface);
    assert_eq!(texts(&layout.description_lines), ["Choose a result"]);
    assert_eq!(texts(&layout.metadata_lines), ["Details"]);
    assert_eq!(layout.result_rows.len(), 5);
    assert!(layout.rect.rows <= 36);
    assert_eq!(layout.options_rows, layout.result_rows.len());
}

#[test]
fn result_height_includes_provider_statuses_and_shrinks_after_refresh() {
    let mut palette = CommandPalette::default();
    palette.set_query(" ".into());
    let surface = Viewport {
        cols: 80,
        rows: 40,
        ..Default::default()
    };
    palette.set_sections(
        (0..4)
            .map(|i| PluginSection {
                title: format!("Provider {i}"),
                entries: vec![],
                semantic: true,
                status: Some("Loading…".into()),
            })
            .collect(),
    );
    let layout = palette.layout(surface);
    assert_eq!(layout.rect.rows, 13);
    assert_eq!(layout.result_rows.len(), 8);
    assert_eq!(
        layout
            .result_rows
            .iter()
            .filter(|row| matches!(row, ResultRow::Status(_, _)))
            .count(),
        4
    );
    palette.set_sections(vec![]);
    assert_eq!(palette.layout(surface).rect.rows, 6);
}

fn texts(lines: &[MdLine]) -> Vec<String> {
    lines.iter().map(|line| line.text.clone()).collect()
}

#[test]
fn markdown_results_share_styles_search_and_activation_for_all_providers() {
    for plugin in [false, true] {
        let mut palette = CommandPalette::default();
        let mut item = entry(
            "stable-id",
            "**日本語** `command`",
            if plugin {
                EntryKind::Plugin
            } else {
                EntryKind::Command
            },
        );
        item.description = "~~old~~ **new**".into();
        if plugin {
            palette.set_sections(vec![PluginSection {
                title: "**Provider**".into(),
                entries: vec![item],
                semantic: false,
                status: None,
            }]);
        } else {
            palette.set_entries(vec![item]);
        }
        palette.insert_text("日本語 command");
        assert_eq!(ids(&palette), ["stable-id"]);
        assert_eq!(palette.action_label("stable-id"), "日本語 command");
        let layout = palette.layout(Viewport {
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        let lines = rendered_lines(&palette, &layout).join("\n");
        assert!(lines.contains("日本語 command — old new"), "{lines}");
        assert!(!lines.contains("**"));
        let chunks = palette.chunks(&layout, &Style::default());
        let japanese = chunks
            .iter()
            .skip(layout.options_y - layout.rect.y)
            .flat_map(|chunk| &chunk.terminal_characters)
            .find(|c| c.character == '日')
            .unwrap();
        assert_eq!(japanese.styles.bold, Some(AnsiCode::On));
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate("stable-id".into())
        );
    }
}

#[test]
fn markdown_form_labels_leave_values_literal_and_cursor_aligned() {
    let mut palette = CommandPalette {
        description: "# Instructions\n\nUse **care**.".into(),
        ..Default::default()
    };
    let mut field = Field::text("**名前**", "**literal**");
    field.input_options = Some(yinz_types::command_palette::InputOptions {
        kind: "text".into(),
        help: "Use `code`".into(),
        error: "**Required**".into(),
        ..Default::default()
    });
    palette.open_form(Form::new(
        "form".into(),
        "**Title**".into(),
        vec![field],
        "**Save**",
        false,
    ));
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let lines = rendered_lines(&palette, &layout).join("\n");
    assert!(lines.contains("名前: **literal**"), "{lines}");
    assert!(lines.contains("Use code Required"), "{lines}");
    assert!(lines.contains("Instructions"));
    let (x, _) = palette.cursor(&layout).unwrap();
    assert_eq!(x, layout.rect.x + layout.border + 6 + "**literal**".len());
    key(&mut palette, BareKey::Tab);
    assert_eq!(
        palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
        Outcome::Submit {
            id: "form".into(),
            values: vec!["**literal**".into()]
        }
    );
}

#[test]
fn markdown_text_body_wraps_and_keeps_character_styles() {
    let mut palette = CommandPalette::default();
    palette.open_text(
        "# Heading\n\n**Bold** and ~~old~~\n\n- item\n\n```\n**literal code**\n```".into(),
    );
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 30,
        ..Default::default()
    });
    let lines = rendered_lines(&palette, &layout).join("\n");
    assert!(lines.contains("Heading"));
    assert!(lines.contains("Bold and old"));
    assert!(lines.contains("**literal code**"));
    assert!(layout
        .text_lines
        .iter()
        .any(|line| line.styles.iter().any(|style| style.strike)));
    for cols in 0..8 {
        let layout = palette.layout(Viewport {
            cols,
            rows: 8,
            ..Default::default()
        });
        for chunk in palette.chunks(&layout, &Style::default()) {
            let text: String = chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect();
            assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= cols);
        }
    }
}

#[test]
fn inline_markdown_does_not_expand_table_padding_or_split_long_labels() {
    let header = "x".repeat(10_000);
    let table = format!("| **{header}** |\n| --- |\n{}", "| a |\n".repeat(100));
    let line = inline(&table);
    assert!(line.text.starts_with(&header));
    assert!(line.text.len() < table.len());
    assert!(line.styles[0].bold);
    assert_eq!(line.text.chars().count(), line.styles.len());
    assert_eq!(inline(&header).text, header);
}

#[test]
fn inline_markdown_keeps_styles_aligned_across_many_blocks() {
    let line = inline(&format!("{}**日**", "a\n\n".repeat(50_000)));
    assert_eq!(line.text, format!("{}日", "a ".repeat(50_000)));
    assert_eq!(line.styles.len(), line.text.chars().count());
    assert!(line.styles[..line.styles.len() - 1]
        .iter()
        .all(|style| *style == MdStyle::default()));
    assert!(line.styles.last().unwrap().bold);

    let mut plain = MdLine::plain("日本語");
    append(&mut plain, &inline("**bold**"));
    append(&mut plain, &MdLine::plain("!"));
    assert_eq!(plain.text, "日本語bold!");
    assert_eq!(plain.styles.len(), 8);
    assert!(plain.styles[..3]
        .iter()
        .all(|style| *style == MdStyle::default()));
    assert!(plain.styles[3..7].iter().all(|style| style.bold));
    assert_eq!(plain.styles[7], MdStyle::default());
}

#[test]
fn worktree_search_offers_creation_after_matches_and_restores_navigation() {
    let mut palette = CommandPalette::default();
    palette.push_view(1, "Worktrees".into());
    palette.set_sections(vec![PluginSection {
        title: "Worktrees".into(),
        entries: vec![
            entry("create", "Create new worktree…", EntryKind::Plugin),
            entry("main", "main", EntryKind::Plugin),
            entry("topic", "topic", EntryKind::Plugin),
            entry("project", "Change project…", EntryKind::Plugin),
        ],
        semantic: false,
        status: None,
    }]);
    palette.set_worktree_creation("create".into(), vec!["project".into()]);
    assert_eq!(ids(&palette), ["create", "main", "topic", "project"]);
    for query in ["mai", "main"] {
        palette.set_query(query.into());
        assert_eq!(ids(&palette), ["main", "create"]);
        assert_eq!(palette.selected_entry().unwrap().id, "main");
    }
    for query in ["new-branch", "project", " **literal** "] {
        palette.set_query(query.into());
        assert_eq!(ids(&palette), ["create"]);
        let selected = palette.selected_entry().unwrap();
        assert_eq!(
            inline(&selected.label).text,
            format!("Create worktree ‘{query}’")
        );
        assert_eq!(palette.worktree_creation_values(&selected.id), [query]);
        assert_eq!(
            palette.handle_key(KeyWithModifier::new(BareKey::Enter)),
            Outcome::Activate("create".into())
        );
    }
    palette.push_view(2, "Other view".into());
    assert!(palette.worktree_creation.is_none());
    assert_eq!(palette.back(), Outcome::None);
    assert_eq!(ids(&palette), ["create"]);
    assert_eq!(
        palette.worktree_creation_values("create"),
        [" **literal** "]
    );
    for query in ["", "   "] {
        palette.set_query(query.into());
        assert_eq!(ids(&palette), ["create", "main", "topic", "project"]);
        assert_eq!(
            palette.selected_entry().unwrap().label,
            "Create new worktree…"
        );
        assert!(palette.worktree_creation_values("create").is_empty());
    }
    assert_eq!(palette.back(), Outcome::None);
    assert!(palette.worktree_creation.is_none());
}
