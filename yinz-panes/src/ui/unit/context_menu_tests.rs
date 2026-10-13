use super::*;
use yinz_types::data::ContextMenuItem;

fn viewport(cols: usize, rows: usize) -> Viewport {
    Viewport {
        x: 0,
        y: 0,
        rows,
        cols,
    }
}

fn items(labels: &[&str]) -> Vec<ContextMenuItem> {
    labels
        .iter()
        .map(|label| {
            if *label == "-" {
                ContextMenuItem::separator()
            } else {
                ContextMenuItem::new(*label, vec![])
            }
        })
        .collect()
}

#[test]
fn a_menu_starts_at_the_clicked_cell() {
    let items = items(&["Copy", "Paste"]);
    let layout = context_menu_layout(&items, Position::new(5, 10), viewport(80, 24), None);
    assert_eq!(layout.x, 10, "left edge meets the clicked column");
    assert_eq!(layout.y, 5, "top border meets the clicked row");
    assert_eq!(layout.rows.len(), 2);
    assert_eq!(layout.height(), 4, "two rows plus two borders");
}

#[test]
fn a_menu_that_fits_exactly_below_the_click_does_not_flip() {
    let items = items(&["Copy", "Paste"]);
    let layout = context_menu_layout(&items, Position::new(20, 10), viewport(80, 24), None);
    assert_eq!(layout.y, 20, "top border meets the clicked row");
    assert_eq!(layout.y + layout.height(), 24);
}

#[test]
fn a_menu_at_the_right_edge_flips_rather_than_shrinking() {
    let items = items(&["A fairly long label"]);
    let wide = context_menu_layout(&items, Position::new(0, 0), viewport(80, 24), None);
    let flipped = context_menu_layout(&items, Position::new(0, 78), viewport(80, 24), None);
    assert_eq!(
        flipped.width, wide.width,
        "the menu keeps its width - shrinking is what a pane would have done"
    );
    assert_eq!(
        flipped.x,
        78 - wide.width,
        "its right edge meets the click instead of its left"
    );
    assert!(flipped.x + flipped.width <= 80);
}

#[test]
fn a_menu_at_the_bottom_edge_flips_above_the_click() {
    let items = items(&["One", "Two", "Three"]);
    let layout = context_menu_layout(&items, Position::new(22, 4), viewport(80, 24), None);
    assert_eq!(
        layout.y,
        22 - layout.height(),
        "the menu sits above the pointer"
    );
    assert!(layout.y + layout.height() <= 24);
}

#[test]
fn a_menu_larger_than_the_viewport_is_clamped_into_it() {
    let items = items(&["A really quite excessively long menu item label indeed"]);
    let layout = context_menu_layout(&items, Position::new(1, 18), viewport(20, 6), None);
    assert!(
        layout.x + layout.width <= 20,
        "x={} w={}",
        layout.x,
        layout.width
    );
    assert!(layout.y + layout.height() <= 6);
}

#[test]
fn a_list_too_tall_is_windowed_and_says_so() {
    let items = items(&["1", "2", "3", "4", "5", "6", "7", "8"]);
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(40, 6), None);
    assert_eq!(layout.rows.len(), 4, "four rows fit in six viewport rows");
    assert!(!layout.more_above);
    assert!(layout.more_below, "the tail is not silently dropped");
    assert_eq!(layout.rows[0].item_index, 0);
}

#[test]
fn the_window_follows_the_selection_down_the_list() {
    let items = items(&["1", "2", "3", "4", "5", "6", "7", "8"]);
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(40, 6), Some(7));
    assert_eq!(layout.window_start, 4);
    assert_eq!(layout.rows.last().unwrap().item_index, 7);
    assert!(layout.more_above);
    assert!(!layout.more_below);
}

#[test]
fn hit_testing_agrees_with_the_layout_for_every_row() {
    let items = items(&["Copy", "-", "Paste", "Close"]);
    let layout = context_menu_layout(&items, Position::new(3, 7), viewport(80, 24), None);
    for row in &layout.rows {
        let line = layout.row_of_item(row.item_index).unwrap();
        let position = Position::new(line as i32, (layout.x + 2) as u16);
        let hit = context_menu_item_at(&layout, position);
        if row.separator {
            assert_eq!(hit, None, "a separator is not a target");
        } else {
            assert_eq!(hit, Some(row.item_index), "row {line} hit the wrong item");
        }
    }
}

#[test]
fn the_border_is_inside_the_menu_but_is_not_an_item() {
    let items = items(&["Copy"]);
    let layout = context_menu_layout(&items, Position::new(3, 7), viewport(80, 24), None);
    let top_border = Position::new(layout.y as i32, (layout.x + 1) as u16);
    assert!(context_menu_contains(&layout, top_border));
    assert_eq!(context_menu_item_at(&layout, top_border), None);
}

#[test]
fn a_click_outside_the_box_is_neither_contained_nor_an_item() {
    let items = items(&["Copy"]);
    let layout = context_menu_layout(&items, Position::new(3, 7), viewport(80, 24), None);
    let outside = Position::new((layout.y + layout.height()) as i32, layout.x as u16);
    assert!(!context_menu_contains(&layout, outside));
    assert_eq!(context_menu_item_at(&layout, outside), None);
}

#[test]
fn a_disabled_row_is_drawn_but_cannot_be_hit() {
    let mut items = items(&["Copy", "Paste"]);
    items[1].disabled = true;
    let layout = context_menu_layout(&items, Position::new(3, 7), viewport(80, 24), None);
    assert_eq!(layout.rows.len(), 2, "it is still drawn");
    let line = layout.row_of_item(1).unwrap();
    let position = Position::new(line as i32, (layout.x + 2) as u16);
    assert_eq!(context_menu_item_at(&layout, position), None);
}

#[test]
fn a_negative_line_is_not_a_hit() {
    let items = items(&["Copy"]);
    let layout = context_menu_layout(&items, Position::new(3, 7), viewport(80, 24), None);
    let above = Position::new(-1, (layout.x + 2) as u16);
    assert!(!context_menu_contains(&layout, above));
    assert_eq!(context_menu_item_at(&layout, above), None);
}

#[test]
fn a_shortcut_is_right_aligned_within_the_menus_width() {
    let items = vec![ContextMenuItem::new("Copy", vec![]).with_shortcut("Ctrl-c")];
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(80, 24), None);
    assert_eq!(layout.rows[0].shortcut.as_deref(), Some("Ctrl-c"));
    assert_eq!(
        layout.width,
        "Copy".len() + SHORTCUT_GAP + "Ctrl-c".len() + CHROME_COLS,
        "the menu is wide enough for both, plus its chrome"
    );
}

#[test]
fn a_shortcut_is_dropped_before_the_label_it_annotates() {
    let items = vec![ContextMenuItem::new("Rename", vec![]).with_shortcut("Ctrl-alt-shift-r")];
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(14, 24), None);
    assert_eq!(
        layout.rows[0].shortcut, None,
        "no room for both, and the label is what says what the row does"
    );
    assert!(!layout.rows[0].label.is_empty());
}

#[test]
fn a_wide_label_is_truncated_by_display_width_not_byte_count() {
    let items = vec![ContextMenuItem::new("日本語のとても長いラベル", vec![])];
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(16, 24), None);
    let row = &layout.rows[0];
    assert!(
        row.label.width() <= layout.width - CHROME_COLS,
        "{:?} is {} columns in a {} column menu",
        row.label,
        row.label.width(),
        layout.width
    );
}

/// A combining mark is zero columns to `UnicodeWidthStr`, which is what the layout measures the
/// menu's width with, and zero to `TerminalCharacter`, which is what actually advances the
/// terminal's cursor. Counting it as one while drawing left the row a column short of its border.
#[test]
fn a_combining_mark_costs_the_row_no_width() {
    let items = items(&["Cafe\u{0301} au lait"]);
    let layout = context_menu_layout(&items, Position::new(2, 2), viewport(80, 24), None);
    let row = &layout.rows[0];
    assert_eq!(
        row.label, "Cafe\u{0301} au lait",
        "the mark rides along with the character it belongs to"
    );
    assert_eq!(
        row.label.width(),
        layout.width - CHROME_COLS,
        "the label fills the text area exactly"
    );
    let chunks = context_menu_chunks(&layout, None, &Style::default());
    for chunk in &chunks {
        let width: usize = chunk.terminal_characters.iter().map(|c| c.width()).sum();
        assert_eq!(
            width, layout.width,
            "a mark counted as a column tears the border"
        );
    }
}

/// The truncation and the width the menu was laid out to have to agree about what a zero-width
/// character costs, or a label that fits is cut anyway - losing the accent off its last character
/// while keeping the character.
#[test]
fn truncation_measures_a_combining_mark_as_zero_columns() {
    assert_eq!(
        truncate_to_width("Cafe\u{0301}", 4),
        "Cafe\u{0301}",
        "four columns of text, and the mark is not a fifth"
    );
    assert_eq!(
        truncate_to_width("Cafe\u{0301}s", 4),
        "Cafe\u{0301}",
        "the mark stays with its base character when the next one is cut"
    );
}

#[test]
fn every_drawn_row_is_exactly_the_menus_width() {
    let mut items = items(&["Copy", "-", "Paste"]);
    items[0].shortcut = Some("Ctrl-c".to_owned());
    let layout = context_menu_layout(&items, Position::new(2, 2), viewport(80, 24), None);
    let chunks = context_menu_chunks(&layout, Some(0), &Style::default());
    assert_eq!(chunks.len(), layout.height());
    for chunk in &chunks {
        let width: usize = chunk.terminal_characters.iter().map(|c| c.width()).sum();
        assert_eq!(
            width, layout.width,
            "a row that is not the menu's width tears the border"
        );
        assert_eq!(chunk.x, layout.x);
    }
}

#[test]
fn an_empty_menu_draws_nothing() {
    let layout = context_menu_layout(&[], Position::new(0, 0), viewport(80, 24), None);
    assert!(layout.is_empty());
    assert!(context_menu_chunks(&layout, None, &Style::default()).is_empty());
    assert!(!context_menu_contains(&layout, Position::new(0, 0)));
}

#[test]
fn a_viewport_with_no_room_yields_no_menu_rather_than_a_broken_one() {
    let items = items(&["Copy"]);
    assert!(context_menu_layout(&items, Position::new(0, 0), viewport(3, 24), None).is_empty());
    assert!(context_menu_layout(&items, Position::new(0, 0), viewport(80, 2), None).is_empty());
}

#[test]
fn moving_the_selection_skips_separators_and_disabled_rows() {
    let mut items = items(&["Copy", "-", "Paste", "Close"]);
    items[2].disabled = true;
    assert_eq!(next_selectable(&items, None, true), Some(0));
    assert_eq!(next_selectable(&items, Some(0), true), Some(3));
    assert_eq!(next_selectable(&items, Some(3), true), Some(0), "wraps");
    assert_eq!(
        next_selectable(&items, Some(0), false),
        Some(3),
        "wraps back"
    );
    assert_eq!(next_selectable(&items, None, false), Some(3));
}

#[test]
fn a_menu_of_nothing_selectable_has_no_selection_to_move_to() {
    let items = items(&["-", "-"]);
    assert_eq!(next_selectable(&items, None, true), None);
    assert_eq!(next_selectable(&items, Some(0), true), None);
}

#[test]
fn the_window_hints_at_what_it_cannot_show() {
    let items = items(&["1", "2", "3", "4", "5", "6", "7", "8"]);
    let layout = context_menu_layout(&items, Position::new(0, 0), viewport(40, 6), Some(4));
    let chunks = context_menu_chunks(&layout, Some(4), &Style::default());
    let top: String = chunks[0]
        .terminal_characters
        .iter()
        .map(|c| c.character)
        .collect();
    let bottom: String = chunks
        .last()
        .unwrap()
        .terminal_characters
        .iter()
        .map(|c| c.character)
        .collect();
    assert!(top.contains('┄'), "{top}");
    assert!(bottom.contains('┄'), "{bottom}");
}
