use super::{ErrorScreen, KeyResponse, LayoutListScreen, OptimisticUpdate, Screen};
use crate::ui::LayoutDetail;
use crate::DisplayLayout;
use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{Button, Form, FormEvent, Select, TextInput};

const NAME_FIELD: &str = "name";
const TARGET_FIELD: &str = "target";
/// The two things "save the session" can mean, in the order the dropdown shows
/// them. Index 1 is the current-tab-only one.
const TARGETS: [&str; 2] = ["All Tabs", "Current Tab Only"];

#[derive(Clone)]
pub struct NewLayoutFromCurrentSessionScreen {
    pub form: Form,
    pub session_layout: String,
    pub current_layout_metadata: LayoutMetadata,
}

impl NewLayoutFromCurrentSessionScreen {
    pub fn new(session_layout: String, current_layout_metadata: LayoutMetadata) -> Self {
        Self {
            // "Update Layout" re-reads the session, for changes made to it
            // since this screen opened. It used to be `<u>`, which a focused
            // name field now swallows as a typed character.
            form: Form::new()
                .field(NAME_FIELD, "Save as", TextInput::empty())
                .select(TARGET_FIELD, "Save", Select::new(TARGETS))
                .button("save", Button::new("Save"))
                .button("update", Button::new("Update Layout"))
                .button("cancel", Button::new("Cancel")),
            session_layout,
            current_layout_metadata,
        }
    }

    /// Whether what is being saved is the focused tab rather than the whole
    /// session — i.e. which option the target select is on.
    fn save_current_tab_only(&self) -> bool {
        self.form
            .select_by_id(TARGET_FIELD)
            .and_then(|select| select.selected_index())
            == Some(1)
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> KeyResponse {
        match self.form.handle_key(key.clone()) {
            FormEvent::Submit => self.attempt_save_current_session(),
            FormEvent::Cancel => KeyResponse::new_screen(self.cancel_save()),
            // A different target means a different layout to save, so the dump
            // behind the details block is re-read.
            FormEvent::FieldChanged(id) if id == TARGET_FIELD => self.handle_update_layout(),
            FormEvent::FieldChanged(_) | FormEvent::FocusMoved => KeyResponse::render(),
            FormEvent::ButtonPressed(id) => self.handle_button(&id),
            FormEvent::Complete(_) | FormEvent::Ignored => KeyResponse::none(),
        }
    }

    pub fn handle_mouse(&mut self, mouse: Mouse) -> KeyResponse {
        match self.form.handle_mouse(mouse) {
            FormEvent::ButtonPressed(id) => self.handle_button(&id),
            FormEvent::FieldChanged(id) if id == TARGET_FIELD => self.handle_update_layout(),
            FormEvent::Ignored => KeyResponse::none(),
            _ => KeyResponse::render(),
        }
    }

    fn handle_button(&mut self, id: &str) -> KeyResponse {
        match id {
            "save" => self.attempt_save_current_session(),
            "update" => self.handle_update_layout(),
            _ => KeyResponse::new_screen(self.cancel_save()),
        }
    }

    fn handle_update_layout(&mut self) -> KeyResponse {
        if let Some(new_screen) = self.update_layout_from_session() {
            KeyResponse::new_screen(new_screen)
        } else {
            KeyResponse::render()
        }
    }

    fn update_layout_from_session(&mut self) -> Option<Screen> {
        let Ok(focused_pane_info) = get_focused_pane_info() else {
            eprintln!("Cannot retrieve focused tab info");
            return None;
        };
        let focused_tab_index = focused_pane_info.0;
        if self.save_current_tab_only() {
            match dump_session_layout_for_tab(focused_tab_index) {
                Ok((session_layout, Some(session_layout_metadata))) => {
                    self.session_layout = session_layout;
                    self.current_layout_metadata = session_layout_metadata;
                    None
                },
                Ok((_, None)) => Some(Screen::Error(super::ErrorScreen {
                    message: "Failed to retrieve session layout metadata".to_string(),
                    return_to_screen: Box::new(Screen::LayoutList(Default::default())),
                })),
                Err(error_msg) => Some(Screen::Error(super::ErrorScreen {
                    message: format!("Failed to dump session layout: {}", error_msg),
                    return_to_screen: Box::default(),
                })),
            }
        } else {
            match dump_session_layout() {
                Ok((session_layout, Some(session_layout_metadata))) => {
                    self.session_layout = session_layout;
                    self.current_layout_metadata = session_layout_metadata;
                    None
                },
                Ok((_, None)) => Some(Screen::Error(super::ErrorScreen {
                    message: "Failed to retrieve session layout metadata".to_string(),
                    return_to_screen: Box::new(Screen::LayoutList(Default::default())),
                })),
                Err(error_msg) => Some(Screen::Error(super::ErrorScreen {
                    message: format!("Failed to dump session layout: {}", error_msg),
                    return_to_screen: Box::default(),
                })),
            }
        }
    }

    fn cancel_save(&self) -> Screen {
        show_cursor(None);
        Screen::LayoutList(LayoutListScreen::default())
    }

    fn attempt_save_current_session(&self) -> KeyResponse {
        let layout_name = self.determine_layout_name();

        // Capture optimistic update BEFORE the API call
        let optimistic = OptimisticUpdate::Add {
            name: layout_name.clone(),
            metadata: self.current_layout_metadata.clone(),
        };

        match self.save_session_layout(&layout_name) {
            Ok(()) => {
                show_cursor(None);
                KeyResponse::new_screen(Screen::LayoutList(LayoutListScreen::default()))
                    .with_optimistic(optimistic)
            },
            Err(error_msg) => KeyResponse::new_screen(self.create_error_screen(&error_msg)),
        }
    }

    fn create_error_screen(&self, message: &str) -> Screen {
        Screen::Error(ErrorScreen {
            message: message.to_string(),
            return_to_screen: Box::new(Screen::NewLayoutFromSession(self.clone())),
        })
    }

    fn determine_layout_name(&self) -> String {
        let input_text = self.form.text(NAME_FIELD);
        if input_text.is_empty() {
            generate_random_name()
        } else {
            input_text.to_string()
        }
    }

    fn save_session_layout(&self, layout_name: &str) -> Result<(), String> {
        save_layout(layout_name.to_owned(), self.session_layout.clone(), false)
            .map_err(|err| format!("Failed to save layout '{}': {}", layout_name, err))?;

        eprintln!(
            "Successfully saved current session as layout: {}",
            layout_name
        );
        Ok(())
    }

    pub fn render(&mut self, rows: usize, cols: usize) {
        let display_layout = DisplayLayout::Valid(LayoutInfo::File(
            "current".to_string(),
            self.current_layout_metadata.clone(),
        ));

        let layout_detail = LayoutDetail::new(&display_layout);

        let desired_ui_width = std::cmp::max(
            self.help_text_full().0.chars().count(),
            self.max_description_width(),
        );

        let actual_ui_width = std::cmp::min(desired_ui_width, cols);

        let available_cols_for_details = actual_ui_width.saturating_sub(2); // Account for indentation
        let desired_details_height =
            layout_detail.calculate_required_height(available_cols_for_details);

        let desired_ui_height = 12 + desired_details_height;
        let base_y = rows.saturating_sub(desired_ui_height) / 2;
        let base_x = cols.saturating_sub(actual_ui_width) / 2;

        // The form places the cursor itself, and has to do so before anything
        // is printed, so it goes first and the decorations follow.
        let form_y = base_y + 5;
        self.form.render(base_x, form_y, actual_ui_width);

        self.render_title(base_x, base_y, actual_ui_width);
        self.render_description(base_x, base_y + 2, actual_ui_width);

        // An open dropdown is drawn over the rows below the form, so the
        // details block starts below it rather than on top of it.
        let overlay_bottom = self
            .form
            .overlay_rect()
            .map(|rect| rect.y + rect.rows)
            .unwrap_or(0);
        let details_y = std::cmp::max(base_y + 9, overlay_bottom);

        let actual_details_height = if rows < desired_ui_height {
            rows.saturating_sub(details_y + 2)
        } else {
            desired_details_height
        };

        layout_detail.render(
            base_x + 2,
            details_y,
            actual_details_height,
            available_cols_for_details,
        );

        let help_y = details_y + actual_details_height + 1;
        self.render_help_text(base_x, help_y, actual_ui_width);
    }

    fn render_title(&self, x: usize, y: usize, width: usize) {
        let title = Text::new("Save Layout of Current Session").color_all(2);
        print_text_with_coordinates(title, x, y, Some(width), None);
    }

    fn description_text_full(&self) -> (&str, &str) {
        (
            "This layout was created from the current session.",
            "Save it to recreate the session later or share it with others.",
        )
    }
    fn description_text_short(&self) -> (&str, &str) {
        (
            "Layout from current session.",
            "Save it to recreate or share the later.",
        )
    }
    fn render_description(&self, x: usize, y: usize, width: usize) {
        let (line1, line2) = if width >= self.max_description_width() {
            self.description_text_full()
        } else {
            self.description_text_short()
        };
        print_text_with_coordinates(Text::new(line1), x, y, None, None);
        print_text_with_coordinates(Text::new(line2), x, y + 1, None, None);
    }
    fn max_description_width(&self) -> usize {
        let (line1, line2) = self.description_text_full();
        std::cmp::max(line1.chars().count(), line2.chars().count())
    }

    fn help_text_full(&self) -> (&str, &[&str]) {
        (
            "<Tab> - Move between fields, <Enter> - Save, <Esc> - Back",
            &["<Tab>", "<Enter>", "<Esc>"],
        )
    }
    fn help_text_short(&self) -> (&str, &[&str]) {
        (
            "<Tab> - Move, <Enter> - Save, <Esc> - Back",
            &["<Tab>", "<Enter>", "<Esc>"],
        )
    }
    fn render_help_text(&self, x: usize, y: usize, width: usize) {
        let (text, items_to_color) = if width < self.help_text_full().0.chars().count() {
            self.help_text_short()
        } else {
            self.help_text_full()
        };
        let mut text = Text::new(text);
        for item in items_to_color {
            text = text.color_substring(3, item)
        }
        print_text_with_coordinates(text, x, y, None, None);
    }
}
