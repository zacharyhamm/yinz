use super::*;
use crate::ui_components::forms::{Button, Select, TextInput};

fn dialog() -> Dialog {
    Dialog::new(
        Text::new("Rename").color_all(2),
        Form::new()
            .field("name", "Name", TextInput::empty())
            .field("path", "Path", TextInput::empty())
            .button("save", Button::new("Save")),
    )
    .message(Text::new("Choose a new name"))
    .help(Text::new("<Enter> Save"))
}

#[test]
fn full_layout_is_centered_with_all_chrome() {
    let dialog = dialog();
    let layout = dialog.layout(Rect::new(0, 10, 40, 15));
    assert_eq!(dialog.height(), 9);
    assert_eq!(
        layout,
        Layout {
            title_y: 13,
            message_y: Some(15),
            form_y: 17,
            form_rows: 3,
            status_y: None,
            help_y: Some(21),
        }
    );
}

#[test]
fn short_layout_drops_optional_chrome_and_scrolls_the_form() {
    let dialog = dialog();
    assert_eq!(
        dialog.layout(Rect::new(0, 0, 40, 3)),
        Layout {
            title_y: 0,
            message_y: None,
            form_y: 1,
            form_rows: 2,
            status_y: None,
            help_y: None,
        }
    );
    assert_eq!(dialog.layout(Rect::new(0, 0, 40, 1)).form_rows, 0);
}

#[test]
fn a_reserved_status_row_sits_below_the_form_and_above_the_help() {
    let dialog = dialog().status_row();
    assert_eq!(dialog.height(), 10, "one blank, one status, one help");
    assert_eq!(
        dialog.layout(Rect::new(0, 10, 40, 15)),
        Layout {
            title_y: 12,
            message_y: Some(14),
            form_y: 16,
            form_rows: 3,
            status_y: Some(20),
            help_y: Some(21),
        }
    );
}

#[test]
fn a_status_appearing_does_not_move_the_form() {
    let area = Rect::new(0, 10, 40, 15);
    let mut dialog = dialog().status_row();
    let empty = dialog.layout(area);
    dialog.set_status(Some(Text::new("branch and base are required")));
    assert_eq!(dialog.layout(area), empty);
    dialog.set_status(None);
    assert_eq!(dialog.layout(area), empty);
}

#[test]
fn a_dialog_that_only_ever_sets_a_status_still_reserves_the_row() {
    let mut dialog = dialog();
    dialog.set_status(Some(Text::new("nope")));
    let with_status = dialog.layout(Rect::new(0, 10, 40, 15));
    dialog.set_status(None);
    assert_eq!(dialog.layout(Rect::new(0, 10, 40, 15)), with_status);
}

#[test]
fn a_cramped_status_dialog_drops_the_help_then_the_message_then_the_row() {
    let dialog = dialog().status_row();
    let status_y = |rows| dialog.layout(Rect::new(0, 0, 40, rows));

    let no_help = status_y(9);
    assert_eq!(no_help.help_y, None);
    assert_eq!((no_help.message_y, no_help.status_y), (Some(2), Some(8)));

    let no_message = status_y(7);
    assert_eq!(no_message.message_y, None);
    assert_eq!((no_message.form_y, no_message.status_y), (2, Some(6)));

    let nothing = status_y(5);
    assert_eq!(nothing.status_y, None);
    assert_eq!((nothing.form_y, nothing.form_rows), (2, 3));
}

#[test]
fn events_are_the_forms_events() {
    let mut dialog = dialog();
    assert_eq!(
        dialog.handle_key(KeyWithModifier::new(BareKey::Char('a'))),
        FormEvent::FieldChanged("name".to_string())
    );
    assert_eq!(dialog.form().text("name"), "a");
    assert_eq!(
        dialog.handle_key(KeyWithModifier::new(BareKey::F(5))),
        FormEvent::Ignored
    );

    dialog.render(Rect::new(0, 0, 40, dialog.height()));
    assert_eq!(
        dialog.handle_mouse(Mouse::LeftClick(5, 2)),
        FormEvent::FocusMoved
    );
    assert_eq!(dialog.form().focused_id(), Some("path"));
}

#[test]
fn centering_uses_terminal_columns() {
    assert_eq!(centered_span(Rect::new(4, 0, 10, 1), "界界").0, 7);
    assert_eq!(centered_span(Rect::new(4, 0, 2, 1), "界界").0, 4);
}

/// A centered line padded out to the area's *full* width starts indented and
/// so ends past the pane, where it wraps and scrolls the frame. It has to
/// end exactly at the right edge instead.
#[test]
fn a_tail_row_under_an_open_dropdown_is_left_to_the_dropdown() {
    let overlay = Some(Rect::new(0, 5, 40, 3));
    assert!(!covered(overlay, 4));
    assert!(covered(overlay, 5));
    assert!(covered(overlay, 7));
    assert!(!covered(overlay, 8), "one past the last option row");
    assert!(!covered(None, 5), "a closed dropdown covers nothing");
}

/// The dialog is the same size and in the same place whether or not a list
/// is open - the list overlays the tail rather than displacing anything.
#[test]
fn an_open_dropdown_does_not_move_the_dialog() {
    let mut dialog = Dialog::new(
        Text::new("New agent"),
        Form::new()
            .field("name", "Name", TextInput::empty())
            .select("agent", "Agent", Select::new(["Codex", "Claude"]))
            .button("create", Button::new("Create")),
    )
    .help(Text::new("<Enter> create"))
    .status_row();
    let area = Rect::new(0, 0, 40, dialog.height());
    let closed = dialog.layout(area);

    dialog.form_mut().focus_on("agent");
    dialog.form_mut().select_by_id_mut("agent").unwrap().open();

    assert_eq!(dialog.height(), area.rows);
    assert_eq!(dialog.layout(area), closed);
}

#[test]
fn a_centered_line_never_reaches_past_the_area() {
    let area = Rect::new(4, 0, 20, 1);
    for text in [
        "",
        "x",
        "New agent",
        "界界",
        &"x".repeat(19),
        &"x".repeat(40),
    ] {
        let (x, width) = centered_span(area, text);
        assert_eq!(
            x + width,
            area.x + area.columns,
            "{text:?} is drawn from {x} for {width} columns"
        );
    }
}

fn assert_wrapped_bounds(output: &str, area: Rect) {
    let mut occupied = std::collections::HashSet::new();
    for component in output.split("\x1bPy").skip(1) {
        let mut parts = component.split("\x1b\\").next().unwrap().split(';');
        parts.next();
        let coordinates: Vec<_> = parts.next().unwrap().split('/').collect();
        let x: usize = coordinates[0].parse().unwrap();
        let y: usize = coordinates[1].parse().unwrap();
        let width: usize = coordinates[2].parse().unwrap_or(0);
        assert!(
            x >= area.x && x + width <= area.x + area.columns,
            "{coordinates:?}"
        );
        assert!(y >= area.y && y < area.y + area.rows, "{coordinates:?}");
        assert!(occupied.insert(y), "overlapping sections at {y}");
        let bytes: Vec<u8> = parts
            .next()
            .unwrap()
            .rsplit('$')
            .next()
            .unwrap()
            .trim_start_matches(['x', 'z', 'd', '|'])
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap())
            .collect();
        let text = String::from_utf8(bytes).unwrap();
        assert!(x + text.width() <= area.x + area.columns, "{text:?}");
    }
}

#[test]
fn wrapped_dialog_keeps_controls_cursor_and_drafts_through_resize() {
    let path = "/a/very/long/目录/with e\u{301}/a project folder";
    let mut dialog = Dialog::new(
        Text::new("Choose your first project 界"),
        Form::new()
            .field(
                "path",
                "Folder or local Git repository",
                TextInput::new(path.into()),
            )
            .field(
                "name",
                "Project name (optional)",
                TextInput::new("draft".into()),
            )
            .button("open", Button::new("Open project")),
    )
    .wrap_text(true)
    .message(Text::new(
        "Choose an existing local folder. You can add more projects later.",
    ))
    .help(Text::new("Tab next · Enter select · Esc skip"));
    dialog.set_status(Some(Text::new(
        "Could not open that folder. Check that it exists and is readable.",
    )));
    assert!(dialog.height_for_width(20) > dialog.height_for_width(80));
    for (cols, rows) in [
        (120, 40),
        (80, 24),
        (60, 20),
        (40, 12),
        (20, 8),
        (4, 2),
        (1, 1),
        (0, 8),
        (20, 0),
        (120, 40),
    ] {
        for id in ["path", "name", "open"] {
            dialog.form_mut().focus_on(id);
            if id != "open" {
                dialog.handle_key(KeyWithModifier::new(BareKey::Enter));
            }
            let area = Rect::new(3, 4, cols, rows);
            let cursor_before = dialog
                .form()
                .field_by_id(id)
                .and_then(|field| field.input())
                .map(TextInput::get_cursor_position);
            let buffer = dialog.render_buffer(area);
            assert_eq!(
                cursor_before,
                dialog
                    .form()
                    .field_by_id(id)
                    .and_then(|field| field.input())
                    .map(TextInput::get_cursor_position)
            );
            if cols == 0 || rows == 0 {
                assert!(buffer.output.is_empty());
                assert_eq!(buffer.cursor, Some(None));
            } else {
                assert_wrapped_bounds(&buffer.output, area);
                if id != "open" {
                    let (x, y) = buffer
                        .cursor
                        .flatten()
                        .expect("active input has a visible cursor");
                    assert!(area.contains(x, y));
                    // The geometry used for clicking follows the newly wrapped layout.
                    dialog.handle_mouse(Mouse::LeftClick(y as isize, x));
                    assert_eq!(dialog.form().focused_id(), Some(id));
                }
            }
            assert_eq!(dialog.form().text("path"), path);
            assert_eq!(dialog.form().text("name"), "draft");
        }
    }
}

#[test]
fn wrapped_styling_tracks_unicode_character_offsets() {
    let lines = Text::new("界界 abc").color_range(2, 3..).wrapped(4);
    assert!(lines.iter().all(|line| line.content().width() <= 4));
    let abc = lines
        .iter()
        .find(|line| line.content().contains("abc"))
        .unwrap();
    assert_eq!(abc.content(), " abc");
    assert_eq!(abc.indices()[2], vec![1, 2, 3]);
}
