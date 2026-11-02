use super::*;
use crate::ui::command_palette::{CommandPalette, Field, Form};
use yinz_types::pane_size::Viewport;

fn fixture(text: &str, cols: usize) -> (CommandPalette, CommandPaletteLayout, Vec<CharacterChunk>) {
    let mut palette = CommandPalette::default();
    palette.open_text(text.into());
    let layout = palette.layout(Viewport {
        x: 3,
        y: 2,
        cols,
        rows: 24,
    });
    let chunks = palette.chunks(&layout, &Style::default());
    (palette, layout, chunks)
}

fn locate(chunks: &[CharacterChunk], needle: &str) -> Position {
    for chunk in chunks {
        let text: String = chunk
            .terminal_characters
            .iter()
            .map(|c| c.character)
            .collect();
        if let Some(byte) = text.find(needle) {
            use unicode_width::UnicodeWidthStr;
            return Position::new(chunk.y as i32, (chunk.x + text[..byte].width()) as u16);
        }
    }
    panic!("missing {needle:?}");
}

fn offset(position: Position, columns: usize) -> Position {
    Position::new(position.line() as i32, (position.column() + columns) as u16)
}

#[test]
fn drag_copies_visible_unicode_in_either_direction_and_highlights_whole_glyphs() {
    let (palette, layout, chunks) = fixture("alpha 界e\u{301} omega", 60);
    let start = locate(&chunks, "界");
    // End inside the double-width glyph: copying and highlighting include it whole.
    for (from, to, expected) in [
        (start, offset(start, 1), "界"),
        (start, offset(start, 3), "界e\u{301}"),
        (offset(start, 3), start, "界e\u{301}"),
    ] {
        let mut selection = PaletteSelection::default();
        selection.start(&layout, palette.view, &chunks, from);
        selection.extend(to);
        assert_eq!(selection.release(to), None);
        assert_eq!(selection.selected_text().as_deref(), Some(expected));
        let mut painted = palette.chunks(&layout, &Style::default());
        selection.highlight(&mut painted, &Style::default());
        let highlights = painted
            .iter()
            .flat_map(|chunk| chunk.selection_and_colors());
        assert!(highlights.clone().any(|highlight| highlight
            .selection
            .contains(start.line() as usize, start.column())));
        assert!(highlights.clone().any(|highlight| highlight
            .selection
            .contains(start.line() as usize, start.column() + 1)));
    }
}

#[test]
fn multiline_selection_excludes_frames_and_trailing_padding_and_clamps_outside() {
    let (palette, layout, chunks) = fixture("alpha\n\nbeta", 50);
    let start = locate(&chunks, "alpha");
    let end = offset(locate(&chunks, "beta"), 4);
    let mut selection = PaletteSelection::default();
    selection.start(&layout, palette.view, &chunks, start);
    selection.extend(end);
    selection.release(end);
    // Compact Markdown renders adjacent paragraphs without a spacer row.
    assert_eq!(selection.selected_text().as_deref(), Some("alpha\nbeta"));
    selection.start(&layout, palette.view, &chunks, start);
    selection.extend(Position::new(i32::MAX, u16::MAX));
    selection.release(Position::new(i32::MAX, u16::MAX));
    let text = selection.selected_text().unwrap();
    assert!(text.starts_with("alpha\nbeta"));
    assert!(!text.contains(['│', '╰', '╯', '─']));
    assert!(text.lines().all(|line| !line.ends_with(' ')));
}

#[test]
fn refresh_preserves_text_selection_but_content_geometry_or_view_changes_cancel_it() {
    for change in ["style", "text", "geometry", "view"] {
        let (palette, mut layout, mut chunks) = fixture("alpha", 50);
        let start = locate(&chunks, "alpha");
        let end = offset(start, 5);
        let mut selection = PaletteSelection::default();
        selection.start(&layout, palette.view, &chunks, start);
        selection.extend(end);
        let mut view = palette.view;
        match change {
            "style" => chunks[0].terminal_characters[0].styles = Default::default(),
            "text" => {
                let row = chunks
                    .iter_mut()
                    .find(|chunk| chunk.y == start.line() as usize)
                    .unwrap();
                row.terminal_characters[start.column() - row.x].character = 'z';
            },
            "geometry" => layout.rect.x += 1,
            _ => view += 1,
        }
        selection.reconcile(&layout, view, &chunks);
        assert!(
            selection.captures_mouse(),
            "{change}: cancellation keeps the release captured"
        );
        selection.release(end);
        assert!(!selection.captures_mouse());
        assert_eq!(selection.selected_text().is_some(), change == "style");
    }
}

#[test]
fn clicks_controls_and_cancelled_gestures_never_select_text() {
    let (palette, layout, chunks) = fixture("alpha", 50);
    let position = locate(&chunks, "alpha");
    let mut selection = PaletteSelection::default();
    selection.start(&layout, palette.view, &chunks, position);
    assert_eq!(selection.release(position), Some(position));
    assert!(selection.selected_text().is_none());
    for control in [false, true] {
        selection.start(&layout, palette.view, &chunks, position);
        if control {
            selection.capture_control();
        } else {
            selection.clear();
        }
        assert_eq!(selection.release(offset(position, 5)), None);
        assert!(selection.selected_text().is_none());
        assert!(!selection.captures_mouse());
    }
}

#[test]
fn dragging_back_to_the_anchor_inside_a_wide_character_leaves_no_selection() {
    let (palette, layout, chunks) = fixture("alpha 界 beta", 50);
    let start = offset(locate(&chunks, "界"), 1);
    let mut selection = PaletteSelection::default();
    selection.start(&layout, palette.view, &chunks, start);
    selection.extend(offset(start, 4));
    assert!(selection.selected_text().is_some());
    assert_eq!(selection.release(start), None);
    assert!(selection.selected_text().is_none());
}

#[test]
fn form_labels_and_button_padding_are_selectable_but_controls_capture() {
    let mut palette = CommandPalette::default();
    palette.open_form(Form::new(
        "form".into(),
        "Form".into(),
        vec![Field::text("Name", "value")],
        "Save",
        false,
    ));
    let layout = palette.layout(Viewport {
        x: 0,
        y: 0,
        cols: 60,
        rows: 24,
    });
    let chunks = palette.chunks(&layout, &Style::default());
    let label = locate(&chunks, "Name:");
    let input = locate(&chunks, "value");
    let button = locate(&chunks, "Save");
    assert!(!palette.mouse_control(&layout, label));
    assert!(palette.mouse_control(&layout, input));
    assert!(palette.mouse_control(&layout, button));
    assert!(!palette.mouse_control(&layout, offset(button, 8)));
}

#[test]
fn selections_work_on_small_borderless_surfaces() {
    let (palette, layout, chunks) = fixture("界", 2);
    assert_eq!(layout.border, 0);
    let start = locate(&chunks, "界");
    let mut selection = PaletteSelection::default();
    selection.start(&layout, palette.view, &chunks, start);
    selection.release(offset(start, 2));
    assert_eq!(selection.selected_text().as_deref(), Some("界"));
}
