use super::{host, run};
use crate::plugin_tests::runtime_parity::SteelReference;
use agent_proto::agents;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{atomic::AtomicU64, Arc},
};
use yinz_terminal::panes::{
    grid::Grid,
    terminal_character::{AnsiCode, TerminalCharacter},
};
use yinz_types::{data::PaletteColor, style::Style};

fn plugin() -> SteelReference {
    plugin_with_clock(Arc::new(AtomicU64::new(90_000)))
}

fn plugin_with_clock(clock: Arc<AtomicU64>) -> SteelReference {
    let (_, responder) = host(&[], clock.clone());
    let mut plugin = SteelReference::with_clock(
        "agents",
        BTreeMap::from([("view".into(), "agents".into())]),
        Path::new("/work"),
        responder,
        Some(clock),
    );
    plugin
        .runtime
        .eval(
            r#"
      (a-reset!)
      (a-set! "rows" (list (hash "Project" "project") (hash "Branch" "  topic")
        (hash "Agent" (hash "pane_id" 7 "kind" "claude" "text" "7 CLA !" "waiting" #t))
        (hash "Run" (hash "run_id" "archived" "kind" "claude" "worktree" "topic"
          "title" "archived run" "started_at_ms" 1000 "continuation" #f))))
      (a-set! "selected" 2) (a-set! "focused" 7)
    "#,
        )
        .unwrap();
    plugin
}

fn themes() -> Vec<Style> {
    let themes = yinz_config::assets::get_default_themes();
    ["gruvbox-light", "gruvbox-dark"]
        .into_iter()
        .map(|name| Style {
            colors: themes.get_theme(name).unwrap().palette,
            ..Default::default()
        })
        .collect()
}

fn render(plugin: &mut SteelReference, style: Style) -> Vec<Vec<TerminalCharacter>> {
    render_at_width(plugin, style, 100)
}

fn render_at_width(
    plugin: &mut SteelReference,
    style: Style,
    cols: usize,
) -> Vec<Vec<TerminalCharacter>> {
    render_sized(plugin, style, 40, cols)
}

fn render_sized(
    plugin: &mut SteelReference,
    style: Style,
    rows: usize,
    cols: usize,
) -> Vec<Vec<TerminalCharacter>> {
    let mut grid = Grid::new(
        rows,
        cols,
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        style,
        false,
        false,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, plugin.render(rows, cols).as_bytes());
    let lines = grid.as_character_lines();
    for cell in lines.iter().flatten() {
        assert!(
            [
                Some(style.colors.text_unselected.background.into()),
                Some(style.colors.text_selected.background.into()),
            ]
            .contains(&cell.styles.background),
            "unthemed background {:?} for {:?}",
            cell.styles.background,
            cell.character,
        );
    }
    for cell in lines
        .iter()
        .flatten()
        .filter(|cell| !cell.character.is_whitespace())
    {
        assert_ne!(cell.styles.dim, Some(AnsiCode::On), "dimmed {cell:?}");
    }
    lines
}

fn cells<'a>(lines: &'a [Vec<TerminalCharacter>], text: &str) -> &'a [TerminalCharacter] {
    for line in lines {
        let plain: String = line.iter().map(|cell| cell.character).collect();
        if let Some(start) = plain.find(text) {
            let start = plain[..start].chars().count();
            return &line[start..start + text.chars().count()];
        }
    }
    panic!(
        "missing {text:?} in {:?}",
        lines
            .iter()
            .map(|line| line.iter().map(|c| c.character).collect::<String>())
            .collect::<Vec<_>>()
    );
}

fn assert_text(lines: &[Vec<TerminalCharacter>], text: &str, color: PaletteColor, bold: bool) {
    for cell in cells(lines, text) {
        assert_eq!(
            cell.styles.foreground,
            Some(color.into()),
            "{text}: {cell:?}"
        );
        assert_eq!(
            cell.styles.bold == Some(AnsiCode::On),
            bold,
            "{text}: {cell:?}"
        );
    }
}

fn assert_selection(row: &[TerminalCharacter], style: Style, selected: bool, marker: char) {
    assert_eq!(
        row[0].character,
        marker,
        "row: {}",
        row.iter().map(|cell| cell.character).collect::<String>()
    );
    let background = if selected {
        style.colors.text_selected.background
    } else {
        style.colors.text_unselected.background
    };
    for cell in row {
        assert_eq!(cell.styles.background, Some(background.into()), "{cell:?}");
    }
}

#[test]
fn agents_selections_and_secondary_text_resolve_theme_colors() {
    for style in themes() {
        let mut plugin = plugin();
        for (state, selected, marker) in [
            ("", "┃ ! claude", '┃'),
            (
                "(a-set! \"focused\" 8) (a-set! \"has_keyboard\" #f)",
                "│ ! claude",
                '│',
            ),
            (
                "(a-set! \"focus\" \"history\") (a-set! \"has_keyboard\" #t)",
                "│ ! claude",
                '│',
            ),
            (
                "(a-set! \"selected\" 3) (a-set! \"focus\" \"list\")",
                "archived run",
                '┃',
            ),
            ("(a-set! \"has_keyboard\" #f)", "archived run", '│'),
        ] {
            plugin.runtime.eval(state).unwrap();
            let lines = render(&mut plugin, style);
            // The waiting agent is pinned under its own header and drawn again in its group.
            assert_text(
                &lines[..1],
                "Needs you",
                style.colors.text_unselected.emphasis_1,
                true,
            );
            assert_text(
                &lines[2..3],
                "project",
                style.colors.text_unselected.base,
                true,
            );
            assert_text(
                &lines[2..3],
                "topic",
                style.colors.text_unselected.base,
                false,
            );
            assert_text(&lines, selected, style.colors.text_selected.base, false);
            let selected_rows: &[usize] = if selected == "archived run" {
                &[4]
            } else {
                &[1, 3]
            };
            for row in [1, 3, 4] {
                let chosen = selected_rows.contains(&row);
                assert_selection(
                    &lines[row][..33],
                    style,
                    chosen,
                    if chosen { marker } else { ' ' },
                );
            }
            for cell in selected_rows.iter().flat_map(|row| &lines[*row][..33]) {
                assert_eq!(
                    cell.styles.foreground,
                    Some(style.colors.text_selected.base.into())
                );
                assert_eq!(
                    cell.styles.background,
                    Some(style.colors.text_selected.background.into())
                );
            }
            if selected == "archived run" {
                assert_text(&lines, "!", style.colors.text_unselected.emphasis_1, false);
                assert_text(
                    &lines,
                    "needs input",
                    style.colors.text_unselected.emphasis_1,
                    false,
                );
            } else {
                assert_text(
                    &lines,
                    "archived run",
                    style.colors.text_unselected.base,
                    false,
                );
                assert_text(
                    &lines,
                    " 1m ",
                    style.colors.text_unselected.emphasis_1,
                    false,
                );
            }
        }
        plugin
            .runtime
            .eval("(a-set! \"key_sheet_scroll\" 0)")
            .unwrap();
        let lines = render(&mut plugin, style);
        assert_text(
            &lines,
            "choose an agent or a run",
            style.colors.text_unselected.base,
            false,
        );
        assert_text(
            &lines,
            "↑/↓ j/k",
            style.colors.text_unselected.emphasis_1,
            false,
        );
        plugin
            .runtime
            .eval("(a-set! \"key_sheet_scroll\" #f) (a-set! \"rows\" '())")
            .unwrap();
        let lines = render(&mut plugin, style);
        assert_text(
            &lines,
            "waiting for agent…",
            style.colors.text_unselected.base,
            false,
        );
        let first_line: String = lines[0].iter().map(|cell| cell.character).collect();
        assert!(first_line.starts_with("waiting for agent…"));
        plugin.runtime.eval("(a-set! \"strip_seen\" #t)").unwrap();
        let lines = render(&mut plugin, style);
        assert!(lines[0]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>()
            .starts_with("no agents"));
        assert_text(
            &lines,
            "no agents",
            style.colors.text_unselected.base,
            false,
        );
        assert_text(
            &lines,
            "Ctrl e n starts one",
            style.colors.text_unselected.emphasis_1,
            false,
        );
    }
}

#[test]
fn agents_history_and_reader_keep_markdown_and_navigation_without_dim() {
    for style in themes() {
        let mut plugin = plugin();
        let mut run = run("active", 7, false);
        run.entries.truncate(1);
        run.summary = Some(agent_proto::agents::Summary {
            text: "Summary body".into(),
            title: "Summary".into(),
            awaiting: false,
            hash: 0,
            created_at_ms: Some(2000),
        });
        run.entries[0].text =
            "Plain **strong** *emphasis* `code` ~~removed~~\n\n> quoted\n\n- [ ] task".into();
        plugin
            .runtime
            .invoke("a-load-run!", vec![json!(run), json!(0)])
            .unwrap();
        let lines = render(&mut plugin, style);
        assert_text(&lines, "summary:", style.colors.text_unselected.base, true);
        assert_text(
            &lines,
            "Jan 1, 1970, 12:00 AM UTC",
            style.colors.text_unselected.emphasis_3,
            false,
        );
        assert_text(
            &lines,
            "Plain",
            style.colors.text_unselected.emphasis_0,
            false,
        );
        assert_text(
            &lines,
            " 1m ",
            style.colors.text_unselected.emphasis_1,
            false,
        );
        plugin
            .runtime
            .eval("(a-set! \"focus\" \"history\") (a-set! \"history_cursor\" 1) (a-open-reader!)")
            .unwrap();
        let lines = render(&mut plugin, style);
        assert_text(&lines, "Plain", style.colors.text_unselected.base, false);
        assert_text(&lines, "strong", style.colors.text_unselected.base, true);
        assert_text(
            &lines,
            "emphasis",
            style.colors.text_unselected.emphasis_1,
            false,
        );
        assert_text(
            &lines,
            "code",
            style.colors.text_unselected.emphasis_3,
            false,
        );
        assert!(cells(&lines, "removed")
            .iter()
            .all(|c| c.styles.strike == Some(AnsiCode::On)));
        assert_text(&lines, "☐", style.colors.text_unselected.base, false);
        assert_text(&lines, "1m", style.colors.text_unselected.emphasis_1, false);
    }
}

#[test]
fn agents_waiting_cues_are_distinct_from_failures() {
    for style in themes() {
        let mut plugin = plugin();
        plugin
            .runtime
            .eval(
                r#"
          (a-set! "summaries" (hash "7" (hash "text" "Ready for review" "title" "review"
            "awaiting" #t "hash" 0 "created_at_ms" 2000)))
        "#,
            )
            .unwrap();
        let lines = render(&mut plugin, style);
        assert_text(
            &lines,
            "────",
            style.colors.text_unselected.emphasis_1,
            false,
        );
        assert_text(
            &lines,
            "Ready for review",
            style.colors.text_unselected.base,
            false,
        );
        for state in [
            "(a-set! \"resume_note\" '(\"pane:7\" \"refused\"))",
            "(a-set! \"resume_note\" #f) (a-set! \"run_error\" '(\"pane:7\" \"unavailable\")) (a-set! \"summaries\" (hash)) (a-set! \"history_cache\" #f)",
            "(a-set! \"search_editing\" #t) (a-set! \"search_error\" \"search failed\")",
        ] {
            plugin.runtime.eval(state).unwrap();
            let lines = render(&mut plugin, style);
            let message = if state.contains("search_editing") { "failed: search failed" }
                else if state.contains("unavailable") { "history unavailable: unavailable" }
                else { "resume failed: refused" };
            assert_text(&lines, message, style.colors.exit_code_error.base, false);
        }
    }
}

fn state(plugin: &mut SteelReference, key: &str) -> serde_json::Value {
    plugin.runtime.invoke("a-get", vec![json!(key)]).unwrap()
}

#[test]
fn agents_message_selection_covers_visible_event_rows_across_focus_and_layouts() {
    for style in themes() {
        for (rows, cols) in [(40, 100), (40, 40), (5, 100)] {
            let mut plugin = plugin();
            let mut run = run("active", 7, false);
            run.entries.truncate(2);
            run.entries[1].text =
                "Plain **strong** *emphasis* `code` ~~removed~~\n\nSecond line\n\nThird line\n\nFourth line".into();
            plugin
                .runtime
                .invoke("a-load-run!", vec![json!(run), json!(0)])
                .unwrap();
            for (focus, keyboard) in [
                ("list", true),
                ("history", true),
                ("history", false),
                ("list", false),
            ] {
                plugin
                    .runtime
                    .eval(&format!(
                        "(a-set! \"focus\" \"{focus}\") (a-set! \"has_keyboard\" #{})",
                        if keyboard { "t" } else { "f" }
                    ))
                    .unwrap();
                let lines = render_sized(&mut plugin, style, rows, cols);
                let event = plugin.runtime.invoke("a-cursor-event", vec![]).unwrap();
                let event_start = event["start"].as_u64().unwrap() as usize;
                let event_end = event_start + event["len"].as_u64().unwrap() as usize;
                assert!(event_end > event_start + 1);
                let start = state(&mut plugin, "last_history_window_start")
                    .as_u64()
                    .unwrap() as usize;
                let room = state(&mut plugin, "last_history_body_rows")
                    .as_u64()
                    .unwrap() as usize;
                let top = [
                    "last_label_rows",
                    "last_history_top",
                    "last_history_body_top",
                ]
                .into_iter()
                .map(|key| state(&mut plugin, key).as_u64().unwrap() as usize)
                .sum::<usize>();
                let divider = state(&mut plugin, "last_divider_col")
                    .as_u64()
                    .map(|n| n as usize);
                let x = divider.map_or(0, |n| n + 1);
                let list_width = divider.unwrap_or(cols);
                // The first drawn line of the selected agent, pinned or grouped: both are marked.
                let window_start =
                    state(&mut plugin, "last_window_start").as_u64().unwrap() as usize;
                let line = state(&mut plugin, "list_lines").as_array().unwrap()[window_start..]
                    .iter()
                    .position(|line| line[0] == 2)
                    .unwrap();
                let agent_row = ["last_label_rows", "last_list_offset"]
                    .into_iter()
                    .map(|key| state(&mut plugin, key).as_u64().unwrap() as usize)
                    .sum::<usize>()
                    + line;
                assert_selection(
                    &lines[agent_row][..list_width],
                    style,
                    true,
                    if focus == "list" && keyboard {
                        '┃'
                    } else {
                        '│'
                    },
                );
                if rows == 5 && focus == "list" && keyboard {
                    assert!(
                        start > event_start,
                        "expected a clipped message: {start}, {event:?}"
                    );
                }
                let mut selected_rows = 0;
                let mut adjacent_rows = 0;
                for i in start..event_end.min(start + room) {
                    let selected = i >= event_start && focus == "history" && keyboard;
                    let marker = if selected {
                        selected_rows += 1;
                        '┃'
                    } else {
                        adjacent_rows += 1;
                        ' '
                    };
                    assert_selection(&lines[top + i - start][x..], style, selected, marker);
                }
                let focused = focus == "history" && keyboard;
                assert_eq!(selected_rows > 0, focused);
                if rows == 40 {
                    assert_eq!(selected_rows > 1, focused);
                    assert!(adjacent_rows > 0);
                    if cols == 100 {
                        let colors = if focused {
                            style.colors.text_selected
                        } else {
                            style.colors.text_unselected
                        };
                        assert_text(&lines, "strong", colors.emphasis_2, true);
                        assert_text(&lines, "emphasis", colors.emphasis_2, false);
                        assert_text(&lines, "code", colors.emphasis_2, false);
                        assert!(cells(&lines, "removed")
                            .iter()
                            .all(|c| c.styles.strike == Some(AnsiCode::On)));
                    }
                }
                assert_eq!(state(&mut plugin, "history_cursor"), 0);
            }
            if rows == 40 {
                plugin
                    .runtime
                    .eval("(a-set! \"pending_history_revision\" 1)")
                    .unwrap();
                let lines = render_sized(&mut plugin, style, rows, cols);
                assert_eq!(state(&mut plugin, "history_cursor"), 1);
                assert_text(
                    &lines,
                    "history",
                    style.colors.text_unselected.emphasis_0,
                    true,
                );
                assert_text(
                    &lines,
                    "strong",
                    style.colors.text_unselected.emphasis_2,
                    true,
                );
            }
        }
    }
}

#[test]
fn agents_reader_centers_and_rewraps_at_max_agent_width() {
    let style = themes()[0];
    let mut plugin = plugin();
    let mut run = run("active", 7, false);
    run.entries.last_mut().unwrap().text =
        "Message **detail** with enough words to wrap across several lines. ".repeat(80);
    plugin
        .runtime
        .invoke("a-load-run!", vec![json!(run), json!(0)])
        .unwrap();
    render(&mut plugin, style);
    plugin.runtime.eval("(a-open-reader!)").unwrap();

    // Reuse the open reader so each case also exercises cache invalidation.
    for (cols, limit, width) in [
        (121, Some(80), 80),
        (120, Some(100), 100),
        (60, Some(80), 60),
        (120, Some(0), 120),
        (120, Some(79), 120),
        (120, None, 120),
    ] {
        plugin.update(&yinz_types::data::Event::ModeUpdate(
            yinz_types::data::ModeInfo {
                style,
                max_agent_width: limit,
                ..Default::default()
            },
        ));
        let expected = render_sized(&mut plugin, style, 12, width);
        let actual = render_sized(&mut plugin, style, 12, cols);
        let left = (cols - width) / 2;
        assert!(actual[1..10]
            .iter()
            .flatten()
            .any(|cell| cell.character.is_alphanumeric()));
        for row in 1..10 {
            assert_eq!(actual[row][left..left + width], expected[row][..width]);
            assert!(actual[row][..left]
                .iter()
                .chain(&actual[row][left + width..])
                .all(|cell| cell.character == ' '));
        }
        let cache = state(&mut plugin, "reader_cache");
        assert_eq!(cache["key"][2], width);
        let count = cache["lines"]["lines"].as_array().unwrap().len();
        let room = state(&mut plugin, "last_reader_rows").as_u64().unwrap() as usize;
        assert_eq!(
            state(&mut plugin, "last_reader_max_scroll"),
            count.saturating_sub(room)
        );
    }
}

#[test]
fn agents_reader_selection_covers_only_its_section_even_when_clipped() {
    for style in themes() {
        let mut plugin = plugin();
        let mut run = run("active", 7, false);
        run.entries.last_mut().unwrap().text = "<task-notification><summary>Finished</summary><note>Plain **strong** *emphasis* `code` ~~removed~~\n\nSecond line\n\nThird line</note><result>Adjacent result</result></task-notification>".into();
        plugin
            .runtime
            .invoke("a-load-run!", vec![json!(run), json!(0)])
            .unwrap();
        render(&mut plugin, style);
        plugin
            .runtime
            .eval(
                r#"
            (a-open-reader!)
            (a-reader-set! "cursor" 1)
            (a-toggle-fold!)
        "#,
            )
            .unwrap();
        for (rows, keyboard) in [(40, true), (40, false), (5, true), (5, false)] {
            plugin
                .runtime
                .eval(if keyboard {
                    "(a-set! \"has_keyboard\" #t)"
                } else {
                    "(a-set! \"has_keyboard\" #f)"
                })
                .unwrap();
            if rows == 5 {
                plugin
                    .runtime
                    .eval(r#"(a-reader-set! "scroll" (+ 1 (list-ref (a-reader-starts) 1)))"#)
                    .unwrap();
            }
            let lines = render_sized(&mut plugin, style, rows, 100);
            let starts = plugin.runtime.invoke("a-reader-starts", vec![]).unwrap();
            let start = starts[1].as_u64().unwrap() as usize;
            let end = starts[2].as_u64().unwrap() as usize;
            assert!(end > start + 1);
            let scroll = plugin
                .runtime
                .invoke("a-reader", vec![json!("scroll")])
                .unwrap()
                .as_u64()
                .unwrap() as usize;
            let count = plugin
                .runtime
                .invoke("a-reader-lines", vec![])
                .unwrap()
                .as_array()
                .unwrap()
                .len();
            let room = state(&mut plugin, "last_reader_rows").as_u64().unwrap() as usize;
            if rows == 5 {
                assert!(scroll > start);
            }
            for i in scroll..count.min(scroll + room) {
                let selected = (start..end).contains(&i);
                assert_selection(
                    &lines[1 + i - scroll],
                    style,
                    selected,
                    if selected {
                        if keyboard {
                            '┃'
                        } else {
                            '│'
                        }
                    } else {
                        ' '
                    },
                );
            }
            if rows == 40 {
                assert_text(&lines, "strong", style.colors.text_selected.base, true);
                assert_text(
                    &lines,
                    "emphasis",
                    style.colors.text_selected.emphasis_1,
                    false,
                );
                assert_text(&lines, "code", style.colors.text_selected.emphasis_3, false);
                assert_text(
                    &lines,
                    "Adjacent result",
                    style.colors.text_unselected.base,
                    false,
                );
                assert!(cells(&lines, "removed")
                    .iter()
                    .all(|c| c.styles.strike == Some(AnsiCode::On)));
            }
        }
    }
}

#[test]
fn agents_search_labels_keep_queries_and_cursors_visible_in_narrow_panes() {
    let mut plugin = plugin();
    let style = themes()[0];
    plugin.runtime.eval("(a-begin-search!)").unwrap();
    for (cols, labels) in [
        (24, ["All: ", "User: "]),
        (16, ["All: ", "User: "]),
        (8, ["/", "/"]),
        (40, ["Search [All messages]: ", "Search [User prompts]: "]),
    ] {
        for (prefix, label) in ["", "prompt: "].into_iter().zip(labels) {
            for text in ["history", "a long search query ending in xyz"] {
                let query = format!("{prefix}{text}");
                plugin
                    .runtime
                    .invoke("a-set!", vec![json!("search_query"), json!(query)])
                    .unwrap();
                for editing in [true, false] {
                    plugin
                        .runtime
                        .invoke("a-set!", vec![json!("search_editing"), json!(editing)])
                        .unwrap();
                    plugin.take_commands();
                    let lines = render_at_width(&mut plugin, style, cols);
                    let line: String = lines[0].iter().map(|cell| cell.character).collect();
                    let commands = plugin.take_commands();
                    let cursor = commands
                        .iter()
                        .find_map(|command| command.get("ShowCursor"))
                        .unwrap();
                    if editing {
                        let visible = line.trim_end().strip_prefix(label).unwrap_or_else(|| {
                            panic!("{cols} columns: missing {label:?} in {line:?}")
                        });
                        assert!(!visible.is_empty(), "{cols} columns: {line:?}");
                        assert!(query.ends_with(visible), "{cols} columns: {line:?}");
                        if label.len() + query.len() < cols {
                            assert_eq!(visible, query);
                        } else {
                            assert!(visible.len() < query.len());
                        }
                        assert!(visible.ends_with(if text == "history" { "story" } else { "xyz" }));
                        assert_eq!(cursor, &json!([line.trim_end().len(), 0]));
                        assert!(cursor[0].as_u64().unwrap() < cols as u64);
                    } else {
                        let expected: String =
                            format!("{label}{query}").chars().take(cols).collect();
                        assert_eq!(line.trim_end(), expected.trim_end());
                        assert!(cursor.is_null());
                    }
                }
            }
        }
    }
}

#[test]
fn agents_clipped_remote_hosts_preserve_status_and_waiting_emphasis() {
    for style in themes() {
        let mut plugin = plugin();
        for host in [
            "very-long-remote-host.example",
            "遠隔計算機.example",
            "a遠隔計算機.example",
        ] {
            for (symbol, status) in [
                ('!', "needs input"),
                ('○', "idle"),
                ('●', "working"),
                ('?', "unknown"),
                ('■', "held"),
                ('×', "exited"),
            ] {
                let rows = json!([
                    {"Project": "project"},
                    {"Agent": {"pane_id": 7, "kind": "claude",
                        "text": format!("7 CLA@{host} {symbol}"), "waiting": symbol == '!',
                        "host": host}}
                ]);
                plugin
                    .runtime
                    .invoke("a-set!", vec![json!("rows"), rows.clone()])
                    .unwrap();
                for (selected, focused, marker) in [(1, 7, '┃'), (1, 8, '┃'), (0, 7, ' ')] {
                    plugin
                        .runtime
                        .invoke("a-set!", vec![json!("selected"), json!(selected)])
                        .unwrap();
                    plugin
                        .runtime
                        .invoke("a-set!", vec![json!("focused"), json!(focused)])
                        .unwrap();
                    let lines = render_at_width(&mut plugin, style, 60);
                    assert_eq!(state(&mut plugin, "last_divider_col"), 20);
                    assert_eq!(state(&mut plugin, "rows"), rows);
                    let row = &lines[1];
                    let divider = row.iter().position(|cell| cell.character == '│').unwrap();
                    assert_eq!(
                        row[..divider]
                            .iter()
                            .map(|cell| cell.width())
                            .sum::<usize>(),
                        20
                    );
                    let plain: String = row[..divider].iter().map(|cell| cell.character).collect();
                    // The glyph leads; the status word and then the host are cut to what the
                    // ten columns after the kind leave, never past a wide character.
                    let tail = agents::cut_to_width(format!("  {status} @{host}"), 10);
                    assert_eq!(
                        plain.trim_end(),
                        format!("{marker} {symbol} claude{tail}").trim_end()
                    );
                    let color = if selected == 1 {
                        style.colors.text_selected.base
                    } else if symbol == '!' {
                        style.colors.text_unselected.emphasis_1
                    } else if symbol == '●' {
                        style.colors.exit_code_success.base
                    } else {
                        style.colors.text_unselected.base
                    };
                    assert_text(&lines[1..2], &symbol.to_string(), color, false);
                    if selected == 1 {
                        cells(&lines, &format!("7 · claude · {status}"));
                    }
                }
            }
        }
    }
}

fn mouse(plugin: &mut SteelReference, kind: &str, point: serde_json::Value) {
    plugin
        .runtime
        .invoke("a-on-mouse!", vec![json!({kind: point})])
        .unwrap();
}

#[test]
fn agents_divider_drag_clamps_restores_and_never_activates() {
    let mut plugin = plugin();
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 30);
    assert_eq!(state(&mut plugin, "last_list_rows"), 23);
    plugin.take_commands();
    mouse(&mut plugin, "LeftClick", json!([2, 30]));
    mouse(&mut plugin, "Hold", json!([2, 45]));
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 45);
    assert_eq!(state(&mut plugin, "selected"), 2);
    mouse(&mut plugin, "Release", json!([100, 45]));
    assert_eq!(state(&mut plugin, "divider_drag"), false);
    assert!(!plugin
        .take_commands()
        .iter()
        .any(|c| c.get("FocusTerminalPane").is_some()));
    plugin.render(24, 50);
    assert_eq!(state(&mut plugin, "last_divider_col"), false);
    assert!(state(&mut plugin, "last_history_top").as_i64().unwrap() > 0);
    plugin.render(24, 120);
    assert_eq!(state(&mut plugin, "last_divider_col"), 60);
    plugin.runtime.invoke("a-end-visit!", vec![]).unwrap();
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 45);
    mouse(&mut plugin, "LeftClick", json!([2, 45]));
    mouse(&mut plugin, "Hold", json!([2, -10]));
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 20);
    mouse(&mut plugin, "Hold", json!([2, 200]));
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 59);
    mouse(&mut plugin, "Release", json!([-1, 200]));
    assert_eq!(state(&mut plugin, "divider_drag"), false);
    plugin.render(24, 51);
    assert_eq!(state(&mut plugin, "last_divider_col"), 20);
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), 59);
    mouse(&mut plugin, "LeftClick", json!([2, 59]));
    plugin
        .runtime
        .invoke(
            "a-on-key!",
            vec![json!({"bare_key":{"Char":"?"},"key_modifiers":[]})],
        )
        .unwrap();
    assert_eq!(state(&mut plugin, "divider_drag"), false);
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), false);
}

#[test]
fn agents_message_scrollback_uses_the_divider_below_the_header() {
    for (rows, cols) in [(16, 90), (20, 48)] {
        let mut plugin = plugin();
        let mut history = run("claude:header-test", 7, false);
        history.entries = (1..=30)
            .map(|revision| agent_proto::agents::RunEntry {
                revision,
                text: format!("Message {revision}"),
                ..history.entries[0].clone()
            })
            .collect();
        plugin
            .runtime
            .invoke("a-load-run!", vec![json!(history), json!(0)])
            .unwrap();
        plugin.render(rows, cols);
        plugin
            .runtime
            .invoke("a-scroll-history!", vec![json!(2)])
            .unwrap();
        let lines = render_sized(&mut plugin, themes()[0], rows, cols);
        let top = state(&mut plugin, "last_label_rows").as_u64().unwrap()
            + state(&mut plugin, "last_history_top").as_u64().unwrap();
        let body = top
            + state(&mut plugin, "last_history_body_top")
                .as_u64()
                .unwrap();
        let x = state(&mut plugin, "last_divider_col")
            .as_u64()
            .map_or(0, |col| col as usize + 1);
        let text = |row: usize| {
            lines[row]
                .iter()
                .skip(x)
                .map(|cell| cell.character)
                .collect::<String>()
        };
        assert!(text(body as usize - 2).contains("session claude:header-test"));
        let above = state(&mut plugin, "last_history_window_start")
            .as_u64()
            .unwrap();
        let below = state(&mut plugin, "history_scroll").as_u64().unwrap();
        assert!(above > 0 && below > 0);
        let readout = agent_proto::panel_text::scroll_readout(above as usize, below as usize);
        assert!(text(body as usize - 1).contains(&readout));
        assert!(!text(top as usize).contains(&readout));
    }
}

#[test]
fn agents_columns_clip_align_and_route_mouse_independently() {
    let mut plugin = plugin();
    let mut history = run("active", 7, false);
    history.entries[0].text = "wrapped history words ".repeat(40);
    plugin
        .runtime
        .invoke("a-load-run!", vec![json!(history), json!(0)])
        .unwrap();
    plugin.runtime.eval(r#"
        (a-set! "rows" (append (a-get "rows") (map (lambda (i)
          (hash "Run" (hash "run_id" (number->string i) "kind" "codex" "worktree" ""
            "title" "a long archived title that must stay in the list" "started_at_ms" 1000 "continuation" #f))) (range 30))))
    "#).unwrap();
    let lines = render(&mut plugin, themes()[0]);
    // "Needs you", its pinned agent, then "project · topic" and the agent's grouped copy.
    assert_eq!(lines[2][10].character, 't');
    assert_eq!(lines[3][2].character, '!');
    assert_eq!(lines[3][33].character, '│');
    plugin.render(10, 90);
    mouse(&mut plugin, "Hover", json!([1, 4]));
    mouse(&mut plugin, "ScrollDown", json!(3));
    assert_eq!(state(&mut plugin, "list_scroll"), 3);
    assert_eq!(state(&mut plugin, "history_scroll"), 0);
    mouse(&mut plugin, "Hover", json!([2, 50]));
    mouse(&mut plugin, "ScrollUp", json!(2));
    assert_eq!(state(&mut plugin, "history_scroll"), 2);
    assert_eq!(state(&mut plugin, "list_scroll"), 3);
    plugin.render(10, 90);
    mouse(&mut plugin, "LeftClick", json!([2, 50]));
    assert_eq!(state(&mut plugin, "focus"), "history");
    assert_eq!(state(&mut plugin, "selected"), 2);
    // Row 0 is the cue for what the scrolled window hides, so the window starts on row 1.
    assert_eq!(state(&mut plugin, "last_list_offset"), 1);
    mouse(&mut plugin, "LeftClick", json!([2, 4]));
    assert_eq!(state(&mut plugin, "focus"), "list");
    assert_eq!(state(&mut plugin, "selected"), 3);
    mouse(&mut plugin, "Release", json!([100, 4]));
    assert_eq!(state(&mut plugin, "click_press"), false);
}

#[test]
fn agents_history_reflows_at_the_dragged_width_and_keeps_anchors() {
    let mut plugin = plugin();
    let mut history = run("active", 7, false);
    history.entries = (1..=15)
        .map(|revision| agent_proto::agents::RunEntry {
            revision,
            text: format!(
                "Event {revision}: {}",
                "words to wrap across several lines ".repeat(12)
            ),
            ..history.entries[0].clone()
        })
        .collect();
    plugin
        .runtime
        .invoke("a-load-run!", vec![json!(history), json!(0)])
        .unwrap();
    plugin.render(12, 120);
    mouse(&mut plugin, "Hover", json!([6, 80]));
    mouse(&mut plugin, "ScrollUp", json!(9));
    plugin.render(12, 120);
    let anchor = state(&mut plugin, "history_bottom_anchor");
    assert_ne!(anchor, false);
    let cache = state(&mut plugin, "history_cache");
    assert_eq!(cache["key"][1], 79);
    mouse(&mut plugin, "LeftClick", json!([2, 40]));
    mouse(&mut plugin, "Hold", json!([2, 85]));
    mouse(&mut plugin, "Release", json!([2, 85]));
    plugin.render(12, 120);
    assert_eq!(state(&mut plugin, "history_cache")["key"][1], 34);
    assert_eq!(state(&mut plugin, "history_bottom_anchor"), anchor);
    assert_eq!(state(&mut plugin, "selected"), 2);
    assert_eq!(state(&mut plugin, "focus"), "list");
}

#[test]
fn agents_detail_updates_repaint_without_changing_compact_rows() {
    let mut plugin = plugin();
    let mut message = super::snapshot_message(1, &[], 2);
    let mut snapshot: serde_json::Value =
        serde_json::from_str(message.payload.as_deref().unwrap()).unwrap();
    snapshot["hooks"]["7"] = json!({"phase":"Busy","detail":"Bash cargo test"});
    message.payload = Some(snapshot.to_string());
    assert_eq!(plugin.pipe(&message), Some(true));
    let rows = state(&mut plugin, "rows");
    let first = render(&mut plugin, themes()[0]);
    cells(&first, "● claude  working");
    cells(&first, "working");
    let history_col = state(&mut plugin, "last_divider_col").as_u64().unwrap() as usize + 1;
    assert!(!first.iter().any(|line| line
        .iter()
        .skip(history_col)
        .map(|cell| cell.character)
        .collect::<String>()
        .contains("Bash cargo test")));
    assert!(!first.iter().any(|line| line
        .iter()
        .map(|cell| cell.character)
        .collect::<String>()
        .contains("State:")));
    snapshot["panes"][0]["title"] = json!("New title");
    snapshot["hooks"]["7"]["detail"] = json!("Read render.scm");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "2".into());
    assert_eq!(plugin.pipe(&message), Some(true));
    assert_eq!(state(&mut plugin, "rows"), rows);
    let next = render(&mut plugin, themes()[0]);
    cells(&next, "● claude  working");
    assert!(!next.iter().any(|line| line
        .iter()
        .skip(history_col)
        .map(|cell| cell.character)
        .collect::<String>()
        .contains("Read render.scm")));
    cells(&next, "New title");

    snapshot["hooks"]["8"]["waiting_on"] = json!("Review the patch?");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "3".into());
    assert_eq!(plugin.pipe(&message), Some(true));
    assert_eq!(state(&mut plugin, "rows"), rows);
    let next = render(&mut plugin, themes()[0]);
    cells(&next, "Review the patch?");

    // A shell's title and folder draw nowhere in the panel, so they do not repaint it.
    snapshot["panes"].as_array_mut().unwrap().push(json!({
        "pane_id": 9, "title": "zsh", "command": null, "cwd": "/work", "status": "Running",
        "tab": [1, "work"]
    }));
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "4".into());
    assert_eq!(plugin.pipe(&message), Some(false));
    snapshot["panes"][2]["title"] = json!("vim main.rs");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "5".into());
    assert_eq!(plugin.pipe(&message), Some(false));
    assert_eq!(state(&mut plugin, "rows"), rows);

    // A compact list draws no activity lines, so only the selected agent's details repaint it.
    assert_eq!(state(&mut plugin, "selected"), 2);
    plugin.render(7, 90);
    snapshot["panes"][1]["title"] = json!("⠐ codex working");
    snapshot["hooks"]["8"]["waiting_on"] = json!("Another question?");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "6".into());
    assert_eq!(plugin.pipe(&message), Some(false));
    snapshot["hooks"]["7"]["detail"] = json!("Edit render.scm");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "7".into());
    assert_eq!(plugin.pipe(&message), Some(true));
}

#[test]
fn agents_first_reported_activity_repaints_an_unselected_agent() {
    let mut plugin = plugin();
    let mut message = super::snapshot_message(1, &[], 2);
    let mut snapshot: serde_json::Value =
        serde_json::from_str(message.payload.as_deref().unwrap()).unwrap();
    snapshot["panes"][0]["title"] = json!("");
    message.payload = Some(snapshot.to_string());
    assert_eq!(plugin.pipe(&message), Some(true));
    let rows = state(&mut plugin, "rows");
    let quiet = rows
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["Agent"]["pane_id"] == 7)
        .unwrap();
    let other = rows
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["Agent"]["pane_id"] == 8)
        .unwrap();
    plugin
        .runtime
        .eval(&format!("(a-set! \"selected\" {other})"))
        .unwrap();
    render(&mut plugin, themes()[0]);
    // With nothing to report, the agent draws no activity line yet.
    assert!(!state(&mut plugin, "list_lines")
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line[0] == quiet && line[1] == true));
    snapshot["panes"][0]["title"] = json!("Fix the flaky test");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "2".into());
    assert_eq!(plugin.pipe(&message), Some(true));
    assert_eq!(state(&mut plugin, "rows"), rows);
    cells(&render(&mut plugin, themes()[0]), "Fix the flaky test");
}

#[test]
fn agents_detail_wraps_a_long_question_the_list_row_cuts() {
    let mut plugin = plugin();
    plugin
        .runtime
        .eval(
            r#"(a-set! "snapshot" (hash "hooks" (hash "7" (hash "phase" "Waiting"
                "waiting_on" "Allow Bash to run the full release build with every feature enabled, then compare the outputs before finishing"))))"#,
        )
        .unwrap();
    let lines = render(&mut plugin, themes()[0]);
    cells(&lines, "Allow Bash to run");
    cells(&lines, "before finishing");
}

#[test]
fn agents_short_history_keeps_the_start_of_long_details() {
    let mut plugin = plugin();
    plugin
        .runtime
        .eval(
            r#"(a-set! "snapshot" (hash "hooks" (hash "7" (hash "phase" "Waiting"
                "waiting_on" "Allow Bash to run the full release build with every feature enabled, compare the outputs, then write a report about every difference before finishing"))))"#,
        )
        .unwrap();
    let lines = render_sized(&mut plugin, themes()[0], 24, 90);
    cells(&lines, "7 · claude · needs input");
    // Seven history rows cannot hold all detail lines, so the status and question stay.
    let lines = render_sized(&mut plugin, themes()[0], 10, 90);
    cells(&lines, "7 · claude · needs input");
    cells(&lines, "Allow Bash to run");
}

#[test]
fn agents_activity_lines_share_selection_and_activate_only_on_release() {
    let mut plugin = plugin();
    plugin
        .runtime
        .eval(
            r#"(a-set! "selected" 3)
            (a-set! "snapshot" (hash "hooks" (hash "7" (hash "phase" "Waiting" "waiting_on" "Approve?"))))"#,
        )
        .unwrap();
    plugin.render(24, 90);
    // The waiting agent's activity line sits under its pinned copy, below "Needs you".
    assert_eq!(
        state(&mut plugin, "list_lines")[2],
        json!([2, true, "pinned"])
    );
    plugin.take_commands();
    mouse(&mut plugin, "LeftClick", json!([2, 5]));
    mouse(&mut plugin, "Release", json!([2, 5]));
    assert_eq!(state(&mut plugin, "selected"), 2);
    assert!(!plugin
        .take_commands()
        .iter()
        .any(|command| command.get("FocusTerminalPane").is_some()));

    mouse(&mut plugin, "LeftClick", json!([2, 5]));
    assert!(!plugin
        .take_commands()
        .iter()
        .any(|command| command.get("FocusTerminalPane").is_some()));
    mouse(&mut plugin, "Release", json!([2, 5]));
    assert!(plugin
        .take_commands()
        .iter()
        .any(|command| command.get("FocusTerminalPane") == Some(&json!([7, false, false]))));
}

#[test]
fn agents_activity_rows_collapse_in_small_views_without_losing_selection() {
    let mut plugin = plugin();
    // An agent with nothing to report draws no activity line at all.
    plugin
        .runtime
        .eval(r#"(a-set! "snapshot" (hash "hooks" (hash "7" (hash "phase" "Waiting" "waiting_on" "Approve?"))))"#)
        .unwrap();
    let mut previous_rich = false;
    for (rows, cols, rich) in [
        (24, 90, true),
        (7, 90, false),
        (24, 23, false),
        (24, 60, false),
        (24, 90, true),
    ] {
        plugin.render(rows, cols);
        let lines = state(&mut plugin, "list_lines");
        assert_eq!(
            lines.as_array().unwrap().iter().any(|line| line[1] == true),
            rich
        );
        if rich != previous_rich {
            assert_eq!(state(&mut plugin, "list_scroll"), false);
        }
        let start = state(&mut plugin, "last_window_start").as_u64().unwrap() as usize;
        let height = state(&mut plugin, "last_list_rows").as_u64().unwrap() as usize;
        assert!(lines.as_array().unwrap()
            [start..lines.as_array().unwrap().len().min(start + height)]
            .iter()
            .any(|line| line[0] == 2));
        assert_eq!(state(&mut plugin, "selected"), 2);
        plugin.runtime.eval(r#"(a-set! "list_scroll" 1)"#).unwrap();
        previous_rich = rich;
    }
}

#[test]
fn agents_history_clicks_open_a_full_panel_reader_only_on_release() {
    let mut plugin = plugin();
    plugin
        .runtime
        .invoke(
            "a-load-run!",
            vec![json!(run("active", 7, false)), json!(0)],
        )
        .unwrap();
    plugin.render(24, 90);
    let row = state(&mut plugin, "last_history_body_top")
        .as_u64()
        .unwrap()
        + state(&mut plugin, "last_label_rows").as_u64().unwrap();
    mouse(&mut plugin, "LeftClick", json!([row, 50]));
    assert_eq!(state(&mut plugin, "focus"), "history");
    let cursor = state(&mut plugin, "history_cursor");
    mouse(&mut plugin, "LeftClick", json!([row, 50]));
    assert_eq!(state(&mut plugin, "reading"), false);
    mouse(&mut plugin, "Hold", json!([row + 1, 50]));
    mouse(&mut plugin, "Release", json!([row, 50]));
    assert_eq!(state(&mut plugin, "reading"), false);
    mouse(&mut plugin, "LeftClick", json!([row, 50]));
    mouse(&mut plugin, "Release", json!([row, 50]));
    assert_eq!(
        plugin
            .runtime
            .invoke("a-reader", vec![json!("scroll")])
            .unwrap(),
        0
    );
    assert_eq!(state(&mut plugin, "history_cursor"), cursor);
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_divider_col"), false);
}

#[test]
fn agents_header_ends_with_the_session_and_styles_details_with_the_theme() {
    for style in themes() {
        let mut plugin = plugin();
        let mut history = run("active", 7, false);
        history.session_id = "cccccccc-cccc-cccc-cccc-cccccccccccc".into();
        history.origin_run_id = Some("claude:origin-hidden".into());
        history.previous_run_id = Some("claude:previous-hidden".into());
        plugin
            .runtime
            .invoke("a-load-run!", vec![json!(history), json!(0)])
            .unwrap();
        plugin.runtime.eval(r#"(a-set! "snapshot" (hash "panes" (list (hash "pane_id" 7 "title" "Review the release" "command" "claude" "status" "Running"))
            "hooks" (hash "7" (hash "phase" "Waiting" "waiting_on" "Allow the release build to run and report any differences before finishing"))))"#).unwrap();
        for (rows, cols) in [(30, 120), (12, 90), (15, 48), (8, 90), (6, 40)] {
            let lines = render_sized(&mut plugin, style, rows, cols);
            let text: String = lines
                .iter()
                .flat_map(|line| line.iter().map(|cell| cell.character))
                .collect();
            assert!(text.contains("session"), "{rows}x{cols}: {text}");
            let x = state(&mut plugin, "last_divider_col")
                .as_u64()
                .map_or(0, |col| col as usize + 1);
            let top = state(&mut plugin, "last_label_rows").as_u64().unwrap()
                + state(&mut plugin, "last_history_top").as_u64().unwrap();
            let header: String = lines
                .iter()
                .skip(top as usize)
                .flat_map(|line| line.iter().skip(x).map(|cell| cell.character))
                .filter(|ch| !ch.is_whitespace())
                .collect();
            assert!(
                header.contains(if cols >= 48 {
                    "sessioncccccccc-cccc-cccc-cccc-cccccccccccc"
                } else {
                    "sessioncccccccc-"
                }),
                "{rows}x{cols}: {text}"
            );
            assert!(!text.contains("origin-hidden") && !text.contains("previous-hidden"));
            let header_rows: Vec<String> = lines
                .iter()
                .skip(top as usize)
                .map(|line| line.iter().skip(x).map(|cell| cell.character).collect())
                .collect();
            let session_row = header_rows
                .iter()
                .position(|row| row.contains("session"))
                .unwrap();
            for (index, row) in header_rows.iter().enumerate() {
                if row.contains("7 · claude")
                    || row.contains("project · topic")
                    || row.contains("Jan 1, 1970")
                {
                    assert!(index < session_row, "{rows}x{cols}: {text}");
                }
            }

            if rows == 30 {
                assert_text(
                    &lines,
                    "session",
                    style.colors.text_unselected.emphasis_3,
                    true,
                );
                assert_text(
                    &lines,
                    "cccccccc-cccc-cccc-cccc-cccccccccccc",
                    style.colors.text_unselected.base,
                    false,
                );
                assert_text(
                    &lines,
                    "7 · claude",
                    style.colors.text_unselected.emphasis_0,
                    true,
                );
                assert_text(
                    &lines,
                    "Task: Review the release",
                    style.colors.text_unselected.base,
                    true,
                );
                assert_text(
                    &lines,
                    "project · topic",
                    style.colors.text_unselected.emphasis_2,
                    false,
                );
            }
        }
    }
}

#[test]
fn agents_summary_and_quota_share_the_message_header_in_light_and_dark_themes() {
    for style in themes() {
        let mut plugin = plugin();
        plugin.runtime.eval(r#"
          (a-set! "snapshot" (hash "panes" (list (hash "pane_id" 7 "title" "Review the release" "command" "claude" "status" "Running"))
            "hooks" (hash "7" (hash "phase" "Waiting" "waiting_on" "Allow the release build to run with all features and report any differences before finishing"))))
          (define observation (hash "status" "ok" "windows" (list (hash "label" "5-hour" "used_percent" 68 "resets_at_ms" 3690000) (hash "label" "Weekly limit" "used_percent" 26 "resets_at_ms" 259290000)) "observed_at_ms" 90000))
          (a-set! "usage_cache" (hash "claude" (hash "observation" observation "result" observation)))
        "#).unwrap();
        plugin.update(&yinz_types::data::Event::ModeUpdate(
            yinz_types::data::ModeInfo {
                style,
                ..Default::default()
            },
        ));
        let lines = render_sized(&mut plugin, style, 30, 120);
        let row = |needle: &str| {
            lines
                .iter()
                .position(|line| {
                    line.iter()
                        .skip(41)
                        .map(|cell| cell.character)
                        .collect::<String>()
                        .contains(needle)
                })
                .unwrap()
        };
        assert!(row("7 · claude · needs input") < row("Task: Review the release"));
        assert!(row("Task: Review the release") < row("project · topic"));
        assert!(row("project · topic") < row("Allow the release build"));
        assert_eq!(row("7 · claude · needs input"), row("Weekly"));
        assert!(row("3d left") < row("nothing to show yet"));
        let lines = render_sized(&mut plugin, style, 10, 120);
        cells(&lines, "7 · claude · needs input");
        cells(&lines, "Weekly · 26% · 5hr · 68%");
    }
}

#[test]
fn agents_tool_spans_collapse_and_enter_reads_every_call_and_result() {
    use agent_proto::agents::RunEntryKind::{Message, ToolCall, ToolResult};
    let mut plugin = plugin();
    let mut run = run("active", 7, false);
    let template = run.entries[0].clone();
    run.entries = [
        (ToolCall, "first **literal** call"),
        (ToolResult, "first result"),
        (ToolCall, "second call"),
        (ToolResult, "second result"),
        (Message, "between spans"),
        (ToolCall, "third call"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, text))| agent_proto::agents::RunEntry {
        revision: index as u64 + 1,
        kind,
        text: text.into(),
        ..template.clone()
    })
    .collect();
    plugin
        .runtime
        .invoke("a-load-run!", vec![json!(run), json!(0)])
        .unwrap();
    plugin.render(40, 100);
    let history = state(&mut plugin, "history_cache")["lines"].clone();
    let events = history["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["revisions"], json!([1, 2, 3, 4]));
    assert_eq!(events[0]["len"], 1);
    assert_eq!(events[1]["start"], 1);
    assert!(history["entries"][0]["text"]
        .as_str()
        .unwrap()
        .ends_with("tool call x2"));
    assert!(
        history["entries"][events[2]["start"].as_u64().unwrap() as usize]["text"]
            .as_str()
            .unwrap()
            .ends_with("tool call x1")
    );
    plugin
        .runtime
        .eval(
            r#"
        (a-set! "focus" "history")
        (a-set! "pending_history_revision" 4)
        (a-position-search-hit!)
    "#,
        )
        .unwrap();
    assert_eq!(state(&mut plugin, "history_cursor"), 2);
    plugin
        .runtime
        .invoke(
            "a-on-key!",
            vec![json!({"bare_key": "Enter", "key_modifiers": []})],
        )
        .unwrap();
    assert_eq!(plugin.runtime.invoke("a-reader", vec![json!("text")]).unwrap(),
        "tool_call:\nfirst **literal** call\n\ntool_result:\nfirst result\n\ntool_call:\nsecond call\n\ntool_result:\nsecond result");
    plugin.render(40, 100);
    let reader = state(&mut plugin, "reader_cache")["lines"]["lines"].to_string();
    assert!(reader.contains("first **literal** call"));
    assert!(reader.contains("second result"));
}

#[test]
fn agents_session_and_reset_share_one_row_and_truncate_the_id() {
    for style in themes() {
        let mut plugin = plugin();
        let mut history = run("active", 7, false);
        history.session_id = "session-id-".repeat(20);
        plugin
            .runtime
            .invoke("a-load-run!", vec![json!(history), json!(0)])
            .unwrap();
        plugin.runtime.eval(r#"
          (a-set! "snapshot" (hash "panes" (list (hash "pane_id" 7 "title" "Review the release" "command" "claude" "status" "Running"))))
          (define observation (hash "status" "ok" "windows" (list (hash "label" "Weekly limit" "used_percent" 26 "resets_at_ms" 259290000)) "observed_at_ms" 90000))
          (a-set! "usage_cache" (hash "claude" (hash "observation" observation "result" observation)))
        "#).unwrap();
        for (rows, cols) in [(30, 120), (12, 90), (15, 48), (8, 90), (6, 40)] {
            let lines = render_sized(&mut plugin, style, rows, cols);
            let x = state(&mut plugin, "last_divider_col")
                .as_u64()
                .map_or(0, |col| col as usize + 1);
            let text: Vec<String> = lines
                .iter()
                .map(|line| line.iter().skip(x).map(|cell| cell.character).collect())
                .collect();
            let session_rows: Vec<_> = text
                .iter()
                .filter(|line| line.contains("session "))
                .collect();
            assert_eq!(session_rows.len(), 1, "{rows}x{cols}: {text:?}");
            let row = session_rows[0];
            assert!(row.contains("session session-id-"), "{row}");
            assert!(row.ends_with("3d left"), "{row}");
            assert!(!row.contains(&"session-id-".repeat(20)), "{row}");
            assert_eq!(
                text.iter().filter(|line| line.contains("3d left")).count(),
                1
            );
        }
    }
}

fn working_snapshot(plugin: &mut SteelReference) {
    let mut message = super::snapshot_message(1, &[], 2);
    let mut snapshot: serde_json::Value =
        serde_json::from_str(message.payload.as_deref().unwrap()).unwrap();
    snapshot["hooks"]["7"] = json!({"phase": "Busy", "detail": "hidden tool"});
    message.payload = Some(snapshot.to_string());
    plugin.pipe(&message);
}

#[test]
fn agents_working_pulse_restarts_and_restores_header() {
    use std::sync::atomic::Ordering;
    let clock = Arc::new(AtomicU64::new(90_000));
    let mut plugin = plugin_with_clock(clock.clone());
    assert!(plugin.take_commands().iter().any(|c| c
        .get("Subscribe")
        .and_then(|v| v.as_array())
        .is_some_and(|events| events.contains(&json!("AgentEvent")))));
    working_snapshot(&mut plugin);
    plugin
        .runtime
        .eval(r#"(a-set! "maintenance_at" 92000)"#)
        .unwrap();
    let normal = render(&mut plugin, themes()[0]);
    let hook = json!({"AgentEvent": {"pane_id": 7}});
    plugin.take_commands();
    assert!(!plugin
        .runtime
        .update(json!({"AgentEvent": {"pane_id": 8}}))
        .unwrap());
    assert_eq!(state(&mut plugin, "pulse"), false);
    assert!(plugin.runtime.update(hook.clone()).unwrap());
    let first = render(&mut plugin, themes()[0]);
    assert_ne!(cells(&first, " · working"), cells(&normal, " · working"));
    clock.store(90_050, Ordering::SeqCst);
    assert!(plugin.runtime.update(json!({"Timer": 0.05})).unwrap());
    let next = render(&mut plugin, themes()[0]);
    assert_ne!(cells(&first, " · working"), cells(&next, " · working"));
    assert!(!plugin
        .take_commands()
        .iter()
        .any(|c| c.get("SetTimeout") == Some(&json!(2.0))));
    clock.store(90_075, Ordering::SeqCst);
    assert!(plugin.runtime.update(hook).unwrap());
    assert!(!plugin
        .take_commands()
        .iter()
        .any(|c| c.get("SetTimeout").is_some()));
    assert_eq!(
        cells(&render(&mut plugin, themes()[0]), " · working"),
        cells(&first, " · working")
    );
    clock.store(91_025, Ordering::SeqCst);
    plugin.runtime.update(json!({"Timer": 0.05})).unwrap();
    assert_ne!(state(&mut plugin, "pulse"), false);
    clock.store(91_075, Ordering::SeqCst);
    assert!(plugin.runtime.update(json!({"Timer": 0.05})).unwrap());
    assert_eq!(state(&mut plugin, "pulse"), false);
    assert_eq!(
        cells(&render(&mut plugin, themes()[0]), " · working"),
        cells(&normal, " · working")
    );
    assert!(!plugin.runtime.update(json!({"Timer": 0.05})).unwrap());
    plugin.take_commands();
    clock.store(92_000, Ordering::SeqCst);
    plugin.runtime.update(json!({"Timer": 2.0})).unwrap();
    assert_eq!(
        plugin
            .take_commands()
            .iter()
            .filter(|c| c.get("SetTimeout") == Some(&json!(2.0)))
            .count(),
        1
    );
    assert_eq!(state(&mut plugin, "maintenance_at"), 94_000);
}

#[test]
fn agents_working_pulse_clears_and_clips() {
    for change in [
        r#"(a-set! "selected" 3) (a-follow-selection! "pane:7")"#,
        r#"(a-end-visit!)"#,
        r#"(a-set! "rows" (list (hash "Agent" (hash "pane_id" 7 "kind" "claude" "text" "7 CLA ○" "waiting" #f)))) (a-set! "selected" 0)"#,
        r#"(a-set! "rows" (list (hash "Agent" (hash "pane_id" 7 "kind" "claude" "text" "7 CLA !" "waiting" #t)))) (a-set! "selected" 0)"#,
    ] {
        let mut plugin = plugin();
        working_snapshot(&mut plugin);
        plugin
            .runtime
            .update(json!({"AgentEvent": {"pane_id": 7}}))
            .unwrap();
        assert_ne!(state(&mut plugin, "pulse"), false);
        plugin.runtime.eval(change).unwrap();
        plugin.runtime.invoke("a-check-pulse!", vec![]).unwrap();
        assert_eq!(state(&mut plugin, "pulse"), false);
    }
    for style in themes() {
        let mut plugin = plugin();
        working_snapshot(&mut plugin);
        for width in [10, 17, 20, 30, 50, 51, 100] {
            plugin.runtime.eval(r#"(a-set! "pulse" #f)"#).unwrap();
            let normal = render_at_width(&mut plugin, style, width);
            plugin
                .runtime
                .update(json!({"AgentEvent": {"pane_id": 7}}))
                .unwrap();
            let pulse = render_at_width(&mut plugin, style, width);
            for (normal, pulse) in normal.iter().flatten().zip(pulse.iter().flatten()) {
                assert_eq!(normal.character, pulse.character);
                assert_eq!(normal.styles.background, pulse.styles.background);
            }
        }
    }
}

#[test]
fn agents_list_pins_waiting_agents_and_labels_groups_archive_and_scroll() {
    let style = themes()[0];
    let mut plugin = plugin();
    let text = |lines: &[Vec<TerminalCharacter>], row: usize| -> String {
        lines[row][..33].iter().map(|cell| cell.character).collect()
    };
    let lines = render(&mut plugin, style);
    assert!(text(&lines, 0).starts_with("Needs you · 1"));
    assert_eq!(
        state(&mut plugin, "list_lines")[0],
        json!([-1, false, "pinned"])
    );
    // One header per checkout, naming its project, with its agent and waiting counts.
    assert!(text(&lines, 2).starts_with("project · topic"));
    assert!(text(&lines, 2).trim_end().ends_with("1! · 1"));
    assert_text(
        &lines[2..3],
        "1!",
        style.colors.text_unselected.emphasis_1,
        true,
    );
    assert!(text(&lines, 3).starts_with("┃ ! claude  needs input"));
    assert!(text(&lines, 3).trim_end().ends_with("◂ #7"));

    // A filtered list draws no pinned copies.
    plugin
        .runtime
        .eval(r#"(a-set! "waiting_base" (hash "rows" (a-get "rows")))"#)
        .unwrap();
    let lines = render(&mut plugin, style);
    assert!(!(0..lines.len()).any(|row| text(&lines, row).contains("Needs you")));
    plugin
        .runtime
        .eval(
            r#"(a-set! "waiting_base" #f)
            (a-set! "rows" (append (a-get "rows") (list (hash "Project" "archive" "archive" "31+"))
              (map (lambda (i) (hash "Run" (hash "run_id" (number->string i) "kind" "opencode"
                "worktree" "" "title" "older run" "started_at_ms" 1000 "continuation" #f))) (range 30))
              (list "MoreRuns")))"#,
        )
        .unwrap();
    let lines = render_sized(&mut plugin, style, 12, 100);
    // The window starts below a cue row, which stays blank while nothing is hidden above.
    assert_eq!(state(&mut plugin, "last_list_offset"), 1);
    assert_eq!(text(&lines, 0).trim(), "");
    assert!(text(&lines, 6).starts_with("─ archive · 31+ ─"));
    assert!(text(&lines, 7).contains("opencode  older run"));
    // The last list row counts the runs and the "load older runs" entry below the window.
    let list_rows = state(&mut plugin, "last_list_rows").as_u64().unwrap() as usize;
    assert_eq!(text(&lines, list_rows - 1).trim(), "↓ 28 more");
}
