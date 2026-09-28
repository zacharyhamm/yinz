use super::*;
use yinz_types::data::PaletteColor;
use yinz_types::style::Style;

fn style_with_distinct_selected_background() -> Style {
    let mut style = Style::default();
    style.colors.text_selected.background = PaletteColor::EightBit(200);
    style.colors.text_selected.base = PaletteColor::EightBit(201);
    style.colors.text_selected.emphasis_1 = PaletteColor::EightBit(202);
    style.colors.text_selected.emphasis_3 = PaletteColor::EightBit(203);
    style
}

fn sample_shortcuts() -> GuestModalShortcuts {
    GuestModalShortcuts {
        zoom: vec!["Ctrl g".to_string(), "o".to_string(), "f".to_string()],
        ascend: vec!["Ctrl o".to_string()],
        descend: vec!["Ctrl o".to_string(), "d".to_string()],
    }
}

fn render_lines(chunks: &[CharacterChunk]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for chunk in chunks {
        let text: String = chunk
            .terminal_characters
            .iter()
            .map(|c| c.character)
            .collect();
        lines.push(text);
    }
    lines
}

fn rendered_text(chunks: &[CharacterChunk]) -> String {
    chunks
        .iter()
        .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
        .collect()
}

#[test]
fn renders_all_expected_content() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let chunks = guest_modal_chunks(80, 30, 0, 0, &style, "my-session", 1, &shortcuts);
    assert_eq!(chunks.len(), 30);
    let rendered = rendered_text(&chunks);
    assert!(rendered.contains("Nested Yinz session detected: my-session"));
    assert!(rendered.contains("What would you like to do?"));
    assert!(rendered.contains("Zoom in and control this session"));
    assert!(rendered.contains("Control this session on focus"));
    assert!(rendered.contains("<↓↑> select"));
    assert!(rendered.contains("<Enter> confirm"));
    assert!(rendered.contains("<Esc> dismiss"));
    // the explanations are wrapped, so read them back as words rather than as a row
    let words = rendered_words(&chunks);
    assert!(
        words.contains("The session will take up the whole screen."),
        "the zoom explanation is missing from: {words}"
    );
    assert!(
        words.contains("keybindings will be sent to this session."),
        "the control-on-focus explanation is missing from: {words}"
    );
}

/// Every rendered character, with runs of whitespace (including row breaks) collapsed to one
/// space - what a wrapped paragraph reads as.
fn rendered_words(chunks: &[CharacterChunk]) -> String {
    render_lines(chunks)
        .iter()
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The modal is drawn over its pane, not over the screen: every chunk carries the pane's
/// content offset, and the rows count up from it.
#[test]
fn chunks_are_placed_at_the_pane_s_content_offset() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let (content_x, content_y) = (7, 3);
    let chunks = guest_modal_chunks(80, 30, content_x, content_y, &style, "s", 0, &shortcuts);
    assert_eq!(chunks.len(), 30);
    for (row, chunk) in chunks.iter().enumerate() {
        assert_eq!(
            (chunk.x, chunk.y),
            (content_x, content_y + row),
            "row {row}"
        );
    }
}

#[test]
fn renders_resolved_keybindings_as_joined_tokens() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let chunks = guest_modal_chunks(80, 30, 0, 0, &style, "s", 0, &shortcuts);
    let rendered = rendered_text(&chunks);
    assert!(rendered.contains("<Ctrl g> + <o> + <f>"));
    assert!(rendered.contains("<Ctrl o>"));
}

#[test]
fn renders_unbound_when_no_keys() {
    let style = Style::default();
    let shortcuts = GuestModalShortcuts::default();
    let chunks = guest_modal_chunks(80, 30, 0, 0, &style, "s", 0, &shortcuts);
    let rendered = rendered_text(&chunks);
    assert!(rendered.contains("<unbound>"));
}

#[test]
fn selection_marker_moves_with_selected_index() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();

    let first = render_lines(&guest_modal_chunks(
        80, 30, 0, 0, &style, "s", 0, &shortcuts,
    ));
    assert!(first
        .iter()
        .any(|line| line.trim_start().starts_with("> 1.")));
    assert!(first.iter().any(|line| line.trim_start().starts_with("2.")));
    assert!(!first
        .iter()
        .any(|line| line.trim_start().starts_with("> 2.")));

    let second = render_lines(&guest_modal_chunks(
        80, 30, 0, 0, &style, "s", 1, &shortcuts,
    ));
    assert!(second
        .iter()
        .any(|line| line.trim_start().starts_with("> 2.")));
    assert!(!second
        .iter()
        .any(|line| line.trim_start().starts_with("> 1.")));
}

fn number_column(line: &str, number: &str) -> usize {
    line.find(number).expect("option number present")
}

#[test]
fn options_are_left_justified_within_the_centered_block() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let chunks = guest_modal_chunks(80, 30, 0, 0, &style, "s", 0, &shortcuts);
    let lines = render_lines(&chunks);
    let selected_line = lines
        .iter()
        .find(|line| line.trim_start().starts_with("> 1."))
        .expect("selected option line present");
    let selected_number_column = number_column(selected_line, "1.");
    let (block_width, _) = guest_modal_block_size(80, &style, "s", 0, &shortcuts);
    assert!(block_width < 80, "the block must leave room to be centered");
    assert_eq!(
        selected_number_column,
        center_modal_pad(block_width, 80) + 2,
        "the number sits two cells (marker and space) into the centered block"
    );

    {
        let (number, needle) = ("2.", "2.");
        let line = lines
            .iter()
            .find(|line| line.trim_start().starts_with(needle))
            .expect("unselected option line present");
        assert_eq!(
            number_column(line, number),
            selected_number_column,
            "the option number column is stable across selection"
        );
    }
}

#[test]
fn small_columns_render_the_fallback() {
    let style = Style::default();
    let shortcuts = GuestModalShortcuts::default();
    let chunks = guest_modal_chunks(18, 20, 0, 0, &style, "s", 0, &shortcuts);
    assert_eq!(chunks.len(), 20);
    let rendered = rendered_text(&chunks);
    assert!(rendered.contains("Nested"));
    assert!(!rendered.contains("What would you like to do?"));
}

#[test]
fn small_rows_render_the_fallback() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let chunks = guest_modal_chunks(80, 5, 0, 0, &style, "s", 0, &shortcuts);
    assert_eq!(chunks.len(), 5);
    let rendered = rendered_text(&chunks);
    assert!(rendered.contains("Nested Yinz session"));
    assert!(!rendered.contains("What would you like to do?"));
}

#[test]
fn tiny_geometry_does_not_panic() {
    let style = Style::default();
    let shortcuts = GuestModalShortcuts::default();
    let chunks = guest_modal_chunks(3, 2, 0, 0, &style, "session", 0, &shortcuts);
    assert_eq!(chunks.len(), 2);
    let empty = guest_modal_chunks(0, 0, 0, 0, &style, "session", 0, &shortcuts);
    assert!(empty.is_empty());
    // the empty guard is `rows < 1 || columns < 1`: one cell is enough to draw something
    assert_eq!(
        guest_modal_chunks(1, 1, 0, 0, &style, "session", 0, &shortcuts).len(),
        1
    );
    assert!(guest_modal_chunks(1, 0, 0, 0, &style, "session", 0, &shortcuts).is_empty());
    assert!(guest_modal_chunks(0, 1, 0, 0, &style, "session", 0, &shortcuts).is_empty());
}

/// Rows that carry any text at all - one for the fallback banner, many for the full block.
fn rows_with_text(chunks: &[CharacterChunk]) -> usize {
    render_lines(chunks)
        .iter()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// Both fallbacks fire at an exact threshold: 20 columns is the full block and 19 the banner,
/// and a surface exactly as tall as the block gets the block while one row shorter gets the
/// banner. The height is read off the layout so this keeps holding as the modal's text changes.
#[test]
fn the_fallback_thresholds_are_exact() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let (_, block_height) = guest_modal_block_size(80, &style, "s", 0, &shortcuts);
    assert!(block_height > 2, "the block is more than a banner");

    let narrow = guest_modal_chunks(19, 40, 0, 0, &style, "s", 0, &shortcuts);
    assert_eq!(rows_with_text(&narrow), 1, "19 columns is the banner");
    let wide_enough = guest_modal_chunks(20, 40, 0, 0, &style, "s", 0, &shortcuts);
    assert!(
        rows_with_text(&wide_enough) > 1,
        "20 columns is the full block"
    );

    let short = guest_modal_chunks(80, block_height - 1, 0, 0, &style, "s", 0, &shortcuts);
    assert_eq!(
        rows_with_text(&short),
        1,
        "one row short of the block is the banner"
    );
    let tall_enough = guest_modal_chunks(80, block_height, 0, 0, &style, "s", 0, &shortcuts);
    assert!(
        rows_with_text(&tall_enough) > 1,
        "exactly the block's height is the full block"
    );
    assert!(
        (0..block_height).any(|row| {
            guest_modal_option_at_content_row(block_height, 80, row, &style, "s", 0, &shortcuts)
                .is_some()
        }),
        "the full block at exactly its height still has option rows to hit"
    );
}

#[test]
fn every_rendered_row_spans_the_full_width() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    // the full block, the narrow fallback, the short fallback, and a fallback too small for
    // even its banner
    for (columns, rows) in [(80, 30), (18, 30), (80, 5), (3, 2)] {
        let chunks = guest_modal_chunks(columns, rows, 0, 0, &style, "s", 0, &shortcuts);
        assert_eq!(chunks.len(), rows);
        for chunk in &chunks {
            let width: usize = chunk
                .terminal_characters
                .iter()
                .map(|c| c.character.width().unwrap_or(0).max(1))
                .sum();
            assert_eq!(width, columns, "at {columns}x{rows}");
        }
    }
}

#[test]
fn hit_test_round_trips_the_options() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let rows = 40;
    let columns = 80;
    let mut hit_rows: Vec<usize> = Vec::new();
    for row in 0..rows {
        if let Some(option) =
            guest_modal_option_at_content_row(rows, columns, row, &style, "session", 0, &shortcuts)
        {
            assert_eq!(hit_rows.len(), option);
            hit_rows.push(row);
        }
    }
    assert_eq!(hit_rows.len(), 2);
}

#[test]
fn hit_test_returns_none_off_option_rows_and_in_fallback() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    assert_eq!(
        guest_modal_option_at_content_row(40, 10, 0, &style, "session", 0, &shortcuts),
        None
    );
    // wide enough for the full block but too short for it: the rows fallback, which has no
    // option rows at all
    assert_eq!(
        guest_modal_option_at_content_row(4, 80, 2, &style, "session", 0, &shortcuts),
        None
    );
    assert_eq!(
        guest_modal_option_at_content_row(40, 80, 0, &style, "session", 0, &shortcuts),
        None
    );
}

#[test]
fn hit_test_matches_rendered_marker_rows() {
    let style = Style::default();
    let shortcuts = sample_shortcuts();
    let rows = 40;
    let columns = 80;
    let selected = 1;
    let lines = render_lines(&guest_modal_chunks(
        columns, rows, 0, 0, &style, "session", selected, &shortcuts,
    ));
    for (row, line) in lines.iter().enumerate() {
        let hit = guest_modal_option_at_content_row(
            rows, columns, row, &style, "session", selected, &shortcuts,
        );
        let trimmed = line.trim_start();
        let is_option_line =
            trimmed.starts_with("> ") || trimmed.starts_with("1.") || trimmed.starts_with("2.");
        assert_eq!(hit.is_some(), is_option_line, "row {} mismatch", row);
    }
}

#[test]
fn selected_rows_carry_the_selected_background_rectangle() {
    let style = style_with_distinct_selected_background();
    let shortcuts = sample_shortcuts();
    let selected = 0;
    let columns = 80;
    let rows = 40;
    let styles = guest_modal_styles(&style);
    let layout = guest_modal_layout(columns, rows, &styles, "session", selected, &shortcuts);
    let selected_option_offset = layout.option_rows[selected];

    let chunks = guest_modal_chunks(columns, rows, 0, 0, &style, "session", selected, &shortcuts);
    let selected_row_index = layout.start_row + selected_option_offset;
    let selected_chunk = &chunks[selected_row_index];

    let selected_background = styles.selected_fill.background;
    let base_background = styles.fill.background;
    assert_ne!(
        selected_background, base_background,
        "the test style must give the selected background a distinct value"
    );

    let block_start = layout.left_pad;
    let block_end = layout.left_pad + layout.block_width;
    let mut block_cells = 0;
    for (column, character) in selected_chunk.terminal_characters.iter().enumerate() {
        if column >= block_start && column < block_end {
            assert_eq!(
                character.styles.background, selected_background,
                "block cell {} on the selected row must carry the selected background",
                column
            );
            block_cells += 1;
        } else {
            assert_eq!(
                character.styles.background, base_background,
                "outer padding cell {} must keep the base background",
                column
            );
        }
    }
    assert_eq!(
        block_cells, layout.block_width,
        "the selected rectangle spans the whole block width"
    );
}

#[test]
fn unselected_rows_do_not_use_the_selected_background() {
    let style = style_with_distinct_selected_background();
    let shortcuts = sample_shortcuts();
    let selected = 0;
    let columns = 80;
    let rows = 40;
    let styles = guest_modal_styles(&style);
    let layout = guest_modal_layout(columns, rows, &styles, "session", selected, &shortcuts);
    let other_option_offset = layout.option_rows[1];

    let selected_background = styles.selected_fill.background;
    let chunks = guest_modal_chunks(columns, rows, 0, 0, &style, "session", selected, &shortcuts);
    let other_row = &chunks[layout.start_row + other_option_offset];
    for character in &other_row.terminal_characters {
        assert_ne!(
            character.styles.background, selected_background,
            "unselected option row must not carry the selected background"
        );
    }
}

#[test]
fn keycode_words_join_with_plus_separators() {
    let style = Style::default();
    let styles = guest_modal_styles(&style);
    let keys = vec!["Ctrl g".to_string(), "o".to_string()];
    let words = guest_modal_keycode_words(&keys, styles.fill.clone(), styles.keycode.clone());
    let joined: String = words.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(joined, "<Ctrl g>+<o>");
    assert_eq!(words[0].1, styles.keycode);
    assert_eq!(words[1].1, styles.fill);
    assert_eq!(words[2].1, styles.keycode);
}

#[test]
fn keycode_words_render_unbound_when_empty() {
    let style = Style::default();
    let styles = guest_modal_styles(&style);
    let words = guest_modal_keycode_words(&[], styles.fill.clone(), styles.keycode.clone());
    assert_eq!(words.len(), 1);
    assert_eq!(words[0].0, "<unbound>");
    assert_eq!(words[0].1, styles.fill);
}

#[test]
fn wrap_words_respects_width_and_indent() {
    let style = Style::default();
    let styles = guest_modal_styles(&style);
    let words = guest_modal_text_words("one two three four five", styles.fill.clone());
    let indent = 4;
    let width = 14;
    let lines = guest_modal_wrap_words(&words, indent, styles.fill.clone(), width);
    assert!(lines.len() > 1, "text wider than the width must wrap");
    // every word comes out, in order, exactly once
    let reassembled = lines
        .iter()
        .flat_map(|line| line.iter().skip(1).map(|(text, _)| text.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(reassembled, "one two three four five");
    for line in &lines {
        let indent_segment = &line[0];
        assert_eq!(indent_segment.0, " ".repeat(indent));
        let rendered_width: usize = line.iter().map(|(text, _)| text.width()).sum();
        assert!(
            rendered_width <= width,
            "wrapped line width {} exceeds {}",
            rendered_width,
            width
        );
    }
}

#[test]
fn center_pad_centers_and_clamps() {
    assert_eq!(center_modal_pad(10, 30), 10);
    // an odd gap rounds down: the spare cell goes to the right of the block
    assert_eq!(center_modal_pad(9, 30), 10);
    assert_eq!(center_modal_pad(30, 30), 0);
    assert_eq!(center_modal_pad(40, 30), 0);
}
