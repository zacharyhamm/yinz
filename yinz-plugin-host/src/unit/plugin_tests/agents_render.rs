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
      (a-set! "rows" (list (hash "Project" "project") (hash "Branch" "topic")
        (hash "Agent" (hash "pane_id" 7 "kind" "claude" "text" "! live agent" "waiting" #t))
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
    let mut grid = Grid::new(
        40,
        100,
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
    vte::Parser::new().advance(&mut grid, plugin.render(40, 100).as_bytes());
    let lines = grid.as_character_lines();
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
            ("", "> ! live agent"),
            (
                "(a-set! \"focused\" 8) (a-set! \"has_keyboard\" #f)",
                "· ! live agent",
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
            for cell in &lines[selected_row] {
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
