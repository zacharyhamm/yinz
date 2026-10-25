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
