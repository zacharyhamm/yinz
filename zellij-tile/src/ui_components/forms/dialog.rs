//! A centered, bounded container for a [`Form`](super::Form).
//!
//! `Dialog` owns presentation, not application state: callers decide when it
//! is active and what its [`FormEvent`](super::FormEvent)s mean. Unhandled keys
//! stay unhandled, so a plugin can either swallow them for strict modality or
//! keep its own shortcuts.
//! Launch a dialog plugin with `modal true` so it opens as an overlay without
//! revealing ordinary floating panes.

use super::{take_columns, Form, FormEvent, Rect};
use crate::prelude::*;
use crate::ui_components::{print_text_with_coordinates, Text};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct Dialog {
    title: Text,
    message: Option<Text>,
    help: Option<Text>,
    status: Option<Text>,
    reserve_status: bool,
    form: Form,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    title_y: usize,
    message_y: Option<usize>,
    form_y: usize,
    form_rows: usize,
    status_y: Option<usize>,
    help_y: Option<usize>,
}

impl Dialog {
    pub fn new(title: Text, form: Form) -> Self {
        Self {
            title,
            message: None,
            help: None,
            status: None,
            reserve_status: false,
            form,
        }
    }

    pub fn message(mut self, message: Text) -> Self {
        self.message = Some(message);
        self
    }

    pub fn set_message(&mut self, message: Option<Text>) {
        self.message = message;
    }

    /// Reserve a row at the bottom of the dialog for [`set_status`](Self::set_status).
    ///
    /// The row is held whether or not a status is set, so a message appearing
    /// or clearing never moves the form.
    pub fn status_row(mut self) -> Self {
        self.reserve_status = true;
        self
    }

    /// Put a transient message on the reserved bottom row, or clear it.
    ///
    /// Unlike [`message`](Self::message), which is a static prompt above the
    /// form, this is for something that comes and goes - a validation error,
    /// say. Setting one reserves the row for good, so a caller that never
    /// called [`status_row`](Self::status_row) still gets a stable layout after
    /// the first message.
    pub fn set_status(&mut self, status: Option<Text>) {
        self.reserve_status = true;
        self.status = status;
    }

    pub fn help(mut self, help: Text) -> Self {
        self.help = Some(help);
        self
    }

    pub fn form(&self) -> &Form {
        &self.form
    }

    pub fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> FormEvent {
        self.form.handle_key(key)
    }

    pub fn handle_mouse(&mut self, mouse: Mouse) -> FormEvent {
        self.form.handle_mouse(mouse)
    }

    /// Rows needed to draw every part of the dialog without scrolling.
    pub fn height(&self) -> usize {
        2 + self.form.height()
            + self.message.as_ref().map(|_| 2).unwrap_or(0)
            + tail_rows(self.help.is_some(), self.reserve_status)
    }

    /// Center the dialog vertically in `area`, shrinking only the form when
    /// the full content does not fit. Optional help and message rows are
    /// dropped before the title or form.
    pub fn render(&mut self, area: Rect) {
        let layout = self.layout(area);
        let inset = if area.columns > 4 { 2 } else { 0 };
        let form_x = area.x + inset;
        let form_width = area.columns.saturating_sub(inset * 2);

        // Form::render_in_viewport places the cursor, which must happen before
        // any print call. Keep it first even though its rows appear below the
        // title.
        if layout.form_rows > 0 && area.columns > 0 {
            self.form
                .render_in_viewport(form_x, layout.form_y, form_width, layout.form_rows);
        } else {
            show_cursor(None);
        }

        if area.rows == 0 || area.columns == 0 {
            return;
        }
        self.render_centered(&self.title, area, layout.title_y);
        if let (Some(message), Some(y)) = (&self.message, layout.message_y) {
            self.render_centered(message, area, y);
        }
        if let (Some(status), Some(y)) = (&self.status, layout.status_y) {
            // Clipped to the reserved row: a status is often a host error, and
            // git speaks in paragraphs. Wrapping one would push the help line
            // off the bottom, which is the whole thing the reserved row exists
            // to prevent. `take_columns` also flattens the newlines to spaces.
            let clipped = status
                .clone()
                .with_content(take_columns(status.content(), area.columns));
            self.render_centered(&clipped, area, y);
        }
        if let (Some(help), Some(y)) = (&self.help, layout.help_y) {
            self.render_centered(help, area, y);
        }
    }

    /// The status row is reserved from `reserve_status` alone, never from
    /// whether a status is currently set - that is what keeps the form still
    /// while messages come and go. When space runs out it outlives the help and
    /// message rows for the same reason: by then it is carrying the error that
    /// explains why nothing happened.
    fn layout(&self, area: Rect) -> Layout {
        let mut show_message = self.message.is_some();
        let mut show_help = self.help.is_some();
        let mut show_status = self.reserve_status;
        let mut gap_after_title = true;

        let desired = |message: bool, help: bool, status: bool, gap: bool| {
            1 + usize::from(gap)
                + self.form.height()
                + if message { 2 } else { 0 }
                + tail_rows(help, status)
        };
        if desired(show_message, show_help, show_status, gap_after_title) > area.rows {
            show_help = false;
        }
        if desired(show_message, show_help, show_status, gap_after_title) > area.rows {
            show_message = false;
        }
        if desired(show_message, show_help, show_status, gap_after_title) > area.rows {
            show_status = false;
        }
        if desired(show_message, show_help, show_status, gap_after_title) > area.rows {
            gap_after_title = false;
        }

        let chrome_rows = 1
            + usize::from(gap_after_title)
            + if show_message { 2 } else { 0 }
            + tail_rows(show_help, show_status);
        let form_rows = self
            .form
            .height()
            .min(area.rows.saturating_sub(chrome_rows));
        let drawn_rows = chrome_rows + form_rows;
        let title_y = area.y + area.rows.saturating_sub(drawn_rows) / 2;
        let mut next_y = title_y + 1 + usize::from(gap_after_title);
        let message_y = show_message.then_some(next_y);
        if show_message {
            next_y += 2;
        }
        let form_y = next_y;
        next_y += form_rows;
        // The blank row before the status doubles as the one before the help,
        // so the help follows immediately underneath it.
        let status_y = show_status.then_some(next_y + 1);
        let help_y = show_help.then_some(status_y.unwrap_or(next_y) + 1);

        Layout {
            title_y,
            message_y,
            form_y,
            form_rows,
            status_y,
            help_y,
        }
    }

    fn render_centered(&self, text: &Text, area: Rect, y: usize) {
        let x = centered_x(area, text.content());
        print_text_with_coordinates(text.clone(), x, y, Some(area.columns), None);
    }
}

/// Rows the blank-separated tail below the form takes: one blank line, then
/// the status row if reserved, then the help row.
fn tail_rows(show_help: bool, show_status: bool) -> usize {
    if show_status {
        2 + usize::from(show_help)
    } else if show_help {
        2
    } else {
        0
    }
}

fn centered_x(area: Rect, text: &str) -> usize {
    area.x + area.columns.saturating_sub(text.width()) / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_components::forms::{Button, TextInput};

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
        assert_eq!(centered_x(Rect::new(4, 0, 10, 1), "界界"), 7);
        assert_eq!(centered_x(Rect::new(4, 0, 2, 1), "界界"), 4);
    }
}
