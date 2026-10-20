use super::{host, run};
use crate::plugin_tests::runtime_parity::SteelReference;
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
    let clock = Arc::new(AtomicU64::new(90_000));
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
    let mut grid = Grid::new(
        40,
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
    vte::Parser::new().advance(&mut grid, plugin.render(40, cols).as_bytes());
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

#[test]
fn agents_selections_and_secondary_text_resolve_theme_colors() {
    for style in themes() {
        let mut plugin = plugin();
        for (state, selected) in [
            ("", "> 7 CLA !"),
            (
                "(a-set! \"focused\" 8) (a-set! \"has_keyboard\" #f)",
                "· 7 CLA !",
            ),
            (
                "(a-set! \"selected\" 3) (a-set! \"has_keyboard\" #t)",
                "archived run",
            ),
            ("(a-set! \"has_keyboard\" #f)", "archived run"),
        ] {
            plugin.runtime.eval(state).unwrap();
            let lines = render(&mut plugin, style);
            assert_text(&lines, "project", style.colors.text_unselected.base, true);
            assert_text(&lines, "topic", style.colors.text_unselected.base, false);
            assert_text(&lines, selected, style.colors.text_selected.base, false);
            let selected_row = if selected == "archived run" { 3 } else { 2 };
            for cell in &lines[selected_row][..33] {
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
        plugin.runtime.eval("(a-set! \"strip_seen\" #t)").unwrap();
        let lines = render(&mut plugin, style);
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
        assert_text(&lines, "started", style.colors.text_unselected.base, false);
        assert_text(&lines, "prompt:", style.colors.text_unselected.base, false);
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
            "claude !",
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
        for (host, clipped) in [
            ("very-long-remote-host.example", "7 CLA@very-long-"),
            ("遠隔計算機.example", "7 CLA@遠隔計算機"),
            ("a遠隔計算機.example", "7 CLA@a遠隔計算"),
        ] {
            for (symbol, status) in [
                ('!', "waiting"),
                ('○', "idle"),
                ('●', "busy"),
                ('?', "running"),
                ('■', "held"),
                ('×', "exited"),
            ] {
                let rows = json!([
                    {"Project": "project"},
                    {"Agent": {"pane_id": 7, "kind": "claude",
                        "text": format!("7 CLA@{host} {symbol}"), "waiting": symbol == '!'}}
                ]);
                plugin
                    .runtime
                    .invoke("a-set!", vec![json!("rows"), rows.clone()])
                    .unwrap();
                for (selected, focused, marker) in [(1, 7, '>'), (1, 8, '·'), (0, 7, ' ')] {
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
                    assert_eq!(plain.trim_end(), format!("{marker} {clipped} {symbol}"));
                    let color = if selected == 1 {
                        style.colors.text_selected.base
                    } else if symbol == '!' {
                        style.colors.text_unselected.emphasis_1
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
    assert_eq!(lines[1][2].character, 't');
    assert_eq!(lines[2][2].character, '7');
    assert_eq!(lines[2][33].character, '│');
    plugin.render(10, 90);
    mouse(&mut plugin, "Hover", json!([2, 4]));
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
    mouse(&mut plugin, "LeftClick", json!([0, 4]));
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
    cells(&first, "7 CLA ●");
    cells(&first, "busy · Bash cargo test");
    snapshot["panes"][0]["title"] = json!("New title");
    snapshot["hooks"]["7"]["detail"] = json!("Read render.scm");
    message.payload = Some(snapshot.to_string());
    message.args.insert("seq".into(), "2".into());
    assert_eq!(plugin.pipe(&message), Some(true));
    assert_eq!(state(&mut plugin, "rows"), rows);
    let next = render(&mut plugin, themes()[0]);
    cells(&next, "7 CLA ●");
    cells(&next, "busy · Read render.scm");
    cells(&next, "New title");
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
        + 1;
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
