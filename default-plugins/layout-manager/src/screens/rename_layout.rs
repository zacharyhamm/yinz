use super::{ErrorScreen, KeyResponse, LayoutListScreen, OptimisticUpdate, Screen};
use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{Button, Form, FormEvent, TextInput};

/// Rows the dialog occupies on top of the form itself: title, blank, blank, help.
const DIALOG_CHROME_HEIGHT: usize = 4;
const HELP_TEXT: &str = "<Tab/Arrows> Move, <Enter> Activate, <Esc> Back";
const FIELD_ID: &str = "name";

#[derive(Clone)]
pub struct RenameLayoutScreen {
    pub form: Form,
    pub old_file_name: String,
    pub selected_layout_index: usize,
}

impl RenameLayoutScreen {
    pub fn new(old_file_name: String, selected_layout_index: usize) -> Self {
        Self {
            form: Form::new()
                .field(
                    FIELD_ID,
                    "Rename Layout",
                    TextInput::new(old_file_name.clone()),
                )
                .button("save", Button::new("Save"))
                .button("cancel", Button::new("Cancel")),
            old_file_name,
            selected_layout_index,
        }
    }

    fn new_name(&self) -> String {
        self.form.text(FIELD_ID).to_string()
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> KeyResponse {
        match self.form.handle_key(key) {
            FormEvent::ButtonPressed(id) if id == "save" => self.attempt_rename(),
            FormEvent::ButtonPressed(_) | FormEvent::Cancel => {
                KeyResponse::new_screen(self.cancel_rename())
            },
            FormEvent::FieldChanged(_) | FormEvent::FocusMoved => KeyResponse::render(),
            FormEvent::Complete(_) | FormEvent::Ignored => KeyResponse::none(),
        }
    }

    pub fn handle_mouse(&mut self, mouse: Mouse) -> KeyResponse {
        match self.form.handle_mouse(mouse) {
            FormEvent::ButtonPressed(id) if id == "save" => self.attempt_rename(),
            FormEvent::ButtonPressed(_) => KeyResponse::new_screen(self.cancel_rename()),
            FormEvent::Ignored => KeyResponse::none(),
            _ => KeyResponse::render(),
        }
    }

    fn cancel_rename(&self) -> Screen {
        show_cursor(None);
        Screen::LayoutList(LayoutListScreen::with_selected_index(
            self.selected_layout_index,
        ))
    }

    fn attempt_rename(&self) -> KeyResponse {
        let new_name = self.new_name();

        if new_name.is_empty() {
            return KeyResponse::new_screen(
                self.create_error_screen("Layout name cannot be empty"),
            );
        }

        let optimistic = OptimisticUpdate::Rename {
            old_name: self.old_file_name.clone(),
            new_name: new_name.clone(),
        };

        match rename_layout(&self.old_file_name, &new_name) {
            Ok(_) => {
                show_cursor(None);
                KeyResponse::new_screen(Screen::LayoutList(LayoutListScreen::with_selected_index(
                    self.selected_layout_index,
                )))
                .with_optimistic(optimistic)
            },
            Err(error_msg) => KeyResponse::new_screen(self.create_error_screen(&error_msg)),
        }
    }

    fn create_error_screen(&self, message: &str) -> Screen {
        show_cursor(None);
        Screen::Error(ErrorScreen {
            message: message.to_string(),
            return_to_screen: Box::new(Screen::RenameLayout(self.clone())),
        })
    }

    pub fn render(&mut self, rows: usize, cols: usize) {
        // Two columns of margin on each side, so nothing touches the pane edge.
        let width = cols.saturating_sub(4).max(1);
        let dialog_height = self.form.height() + DIALOG_CHROME_HEIGHT;
        let base_y = rows.saturating_sub(dialog_height) / 2;
        let base_x = cols.saturating_sub(width) / 2;

        // The form places the cursor itself, and it must do so before anything
        // is printed — so it renders first, and the decorations follow.
        self.form.render(base_x, base_y + 2, width);

        print_text_with_coordinates(
            Text::new("Rename Layout").color_all(2),
            base_x,
            base_y,
            Some(width),
            None,
        );

        // In a pane too short for the whole dialog the help line has nowhere to
        // go but on top of the form's button row, so it is dropped instead - it
        // is a hint; keyboard activation still works without it.
        if rows >= dialog_height {
            let mut help = Text::new(HELP_TEXT);
            for item in ["<Enter>", "<Esc>"] {
                help = help.color_substring(3, item);
            }
            print_text_with_coordinates(help, base_x, base_y + dialog_height - 1, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_save_button_attempts_a_rename() {
        let mut screen = RenameLayoutScreen::new(String::new(), 0);
        assert!(screen
            .handle_key(KeyWithModifier::new(BareKey::Enter))
            .new_screen
            .is_none());
        assert!(screen
            .handle_key(KeyWithModifier::new(BareKey::Enter))
            .new_screen
            .is_none());
        screen.handle_key(KeyWithModifier::new(BareKey::Down));
        assert!(matches!(
            screen
                .handle_key(KeyWithModifier::new(BareKey::Enter))
                .new_screen,
            Some(Screen::Error(_))
        ));
    }
}
