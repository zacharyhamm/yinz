//! The focus container: what turns a pile of widgets into a form.
//!
//! A `Form` owns the focus ring, routes keys to whichever widget has focus,
//! routes clicks to whichever widget's recorded rectangle contains the point,
//! and places the host cursor. That last one is the reason this exists rather
//! than each screen wiring its own widgets: `show_cursor` must be called
//! *before* any `print_*`, because the host reads the cursor position out of
//! the render. Every hand-rolled screen has had to know that
//! (see layout-manager's "Update cursor BEFORE rendering (critical!)"), and
//! every one of them re-derived the cursor column by hand. Here it happens once,
//! in [`Form::render`].
//!
//! ## Tab is not assumed
//!
//! `Tab` already means five different things across the builtin plugins: next
//! screen (session-manager, plugin-manager), toggle save target
//! (layout-manager), next coordinate (multiple-select), descend into a
//! directory (strider), and completion inside [`TextInput`] itself. So a form
//! never insists on it:
//!
//! - traversal keys are configurable via [`Form::with_traversal_keys`], and
//!   default to Tab / Shift-Tab;
//! - anything the form does not consume comes back as [`FormEvent::Ignored`],
//!   so the plugin's own bindings still work and the form never silently
//!   swallows a key.
//!
//! Note the ordering consequence: traversal is checked *before* the focused
//! field sees the key, so with the default keys `Tab` moves focus and a field
//! never reports [`FormEvent::Complete`]. A form that wants tab-completion
//! should pick different traversal keys.

use super::button::{Button, Rect};
use super::choice::{Checkbox, RadioGroup};
use super::label::Label;
use super::select::{Select, SelectAction};
use super::text_input::{InputAction, TextInput};
use super::viewport::{column_to_cursor, viewport};
use crate::prelude::*;
use crate::ui_components::{print_text_with_coordinates, Text};

/// Identifies a field or button within its form.
pub type FieldId = String;

/// What handling an event did, from the plugin's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    /// A field's contents changed.
    FieldChanged(FieldId),
    /// A field asked for completion (see the note on traversal keys above).
    Complete(FieldId),
    /// A button was activated, by key or by click.
    ButtonPressed(FieldId),
    /// Focus moved between widgets.
    FocusMoved,
    /// The form was submitted — Enter in a field.
    Submit,
    /// The form was cancelled — Esc, or Ctrl-c in a field.
    Cancel,
    /// The form did not consume this. The plugin's own bindings apply.
    Ignored,
}

impl FormEvent {
    /// Whether this event means the plugin should re-render.
    pub fn should_render(&self) -> bool {
        !matches!(self, FormEvent::Ignored)
    }
}

/// What a field is: something typed into, or something chosen from.
#[derive(Debug, Clone)]
enum Widget {
    Text(TextInput),
    Select(Select),
    Checkbox(Checkbox),
    Radio(RadioGroup),
}

/// One labelled row of a form.
#[derive(Debug, Clone)]
pub struct Field {
    id: FieldId,
    label: Label,
    widget: Widget,
    last_rendered: Option<Rect>,
}

impl Field {
    pub fn new<I: ToString, L: ToString>(id: I, label: L, input: TextInput) -> Self {
        Field {
            id: id.to_string(),
            label: Label::new(label),
            widget: Widget::Text(input),
            last_rendered: None,
        }
    }

    pub fn new_select<I: ToString, L: ToString>(id: I, label: L, select: Select) -> Self {
        Field {
            id: id.to_string(),
            label: Label::new(label),
            widget: Widget::Select(select),
            last_rendered: None,
        }
    }

    pub fn new_checkbox<I: ToString, L: ToString>(id: I, label: L, checkbox: Checkbox) -> Self {
        Field {
            id: id.to_string(),
            label: Label::new(label),
            widget: Widget::Checkbox(checkbox),
            last_rendered: None,
        }
    }

    pub fn new_radio<I: ToString, L: ToString>(id: I, label: L, radio: RadioGroup) -> Self {
        Field {
            id: id.to_string(),
            label: Label::new(label),
            widget: Widget::Radio(radio),
            last_rendered: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The text editor behind this field, or `None` for another widget type.
    pub fn input(&self) -> Option<&TextInput> {
        match &self.widget {
            Widget::Text(input) => Some(input),
            _ => None,
        }
    }

    pub fn input_mut(&mut self) -> Option<&mut TextInput> {
        match &mut self.widget {
            Widget::Text(input) => Some(input),
            _ => None,
        }
    }

    /// The select behind this field, or `None` for another widget type.
    pub fn select(&self) -> Option<&Select> {
        match &self.widget {
            Widget::Select(select) => Some(select),
            _ => None,
        }
    }

    pub fn select_mut(&mut self) -> Option<&mut Select> {
        match &mut self.widget {
            Widget::Select(select) => Some(select),
            _ => None,
        }
    }

    pub fn checkbox(&self) -> Option<&Checkbox> {
        match &self.widget {
            Widget::Checkbox(checkbox) => Some(checkbox),
            _ => None,
        }
    }

    pub fn checkbox_mut(&mut self) -> Option<&mut Checkbox> {
        match &mut self.widget {
            Widget::Checkbox(checkbox) => Some(checkbox),
            _ => None,
        }
    }

    pub fn radio(&self) -> Option<&RadioGroup> {
        match &self.widget {
            Widget::Radio(radio) => Some(radio),
            _ => None,
        }
    }

    pub fn radio_mut(&mut self) -> Option<&mut RadioGroup> {
        match &mut self.widget {
            Widget::Radio(radio) => Some(radio),
            _ => None,
        }
    }

    pub fn is_select(&self) -> bool {
        matches!(self.widget, Widget::Select(_))
    }

    /// What this field currently holds: typed text or a chosen select/radio option.
    ///
    /// A select or radio group with nothing chosen reads as `""`.
    pub fn text(&self) -> &str {
        match &self.widget {
            Widget::Text(input) => input.get_text(),
            Widget::Select(select) => select.selected_option().unwrap_or(""),
            Widget::Radio(radio) => radio.selected_option().unwrap_or(""),
            Widget::Checkbox(_) => "",
        }
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    /// The prompt drawn to the left of the value, e.g. `"Name: "`.
    fn prompt(&self) -> String {
        format!("{}: ", self.label.content())
    }
}

/// Which widget in a form has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Field(usize),
    Button(usize),
}

#[derive(Debug, Clone)]
pub struct Form {
    fields: Vec<Field>,
    buttons: Vec<(FieldId, Button)>,
    focus: Option<Focus>,
    next_key: KeyWithModifier,
    prev_key: KeyWithModifier,
}

impl Default for Form {
    fn default() -> Self {
        Form::new()
    }
}

impl Form {
    pub fn new() -> Self {
        Form {
            fields: vec![],
            buttons: vec![],
            focus: None,
            next_key: KeyWithModifier::new(BareKey::Tab),
            prev_key: KeyWithModifier::new(BareKey::Tab).with_shift_modifier(),
        }
    }

    /// Add a text field. The first widget added takes focus.
    pub fn field<I: ToString, L: ToString>(mut self, id: I, label: L, input: TextInput) -> Self {
        self.fields.push(Field::new(id, label, input));
        if self.focus.is_none() {
            self.focus = Some(Focus::Field(0));
        }
        self
    }

    /// Add a select field. The first widget added takes focus.
    pub fn select<I: ToString, L: ToString>(mut self, id: I, label: L, select: Select) -> Self {
        self.fields.push(Field::new_select(id, label, select));
        if self.focus.is_none() {
            self.focus = Some(Focus::Field(0));
        }
        self
    }

    /// Add a checkbox field. The first widget added takes focus.
    pub fn checkbox<I: ToString, L: ToString>(
        mut self,
        id: I,
        label: L,
        checkbox: Checkbox,
    ) -> Self {
        self.fields.push(Field::new_checkbox(id, label, checkbox));
        if self.focus.is_none() {
            self.focus = Some(Focus::Field(0));
        }
        self
    }

    /// Add an inline radio group. The first widget added takes focus.
    pub fn radio<I: ToString, L: ToString>(mut self, id: I, label: L, radio: RadioGroup) -> Self {
        self.fields.push(Field::new_radio(id, label, radio));
        if self.focus.is_none() {
            self.focus = Some(Focus::Field(0));
        }
        self
    }

    /// Add a button. The first widget added takes focus.
    pub fn button<I: ToString>(mut self, id: I, button: Button) -> Self {
        self.buttons.push((id.to_string(), button));
        if self.focus.is_none() {
            self.focus = Some(Focus::Button(0));
        }
        self
    }

    /// Replace the default Tab / Shift-Tab traversal.
    pub fn with_traversal_keys(mut self, next: KeyWithModifier, prev: KeyWithModifier) -> Self {
        self.next_key = next;
        self.prev_key = prev;
        self
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn field_by_id(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn field_by_id_mut(&mut self, id: &str) -> Option<&mut Field> {
        self.fields.iter_mut().find(|f| f.id == id)
    }

    /// The text in a field, or `""` if there is no such field.
    pub fn text(&self, id: &str) -> &str {
        self.field_by_id(id).map(|f| f.text()).unwrap_or("")
    }

    pub fn select_by_id(&self, id: &str) -> Option<&Select> {
        self.field_by_id(id).and_then(|field| field.select())
    }

    pub fn select_by_id_mut(&mut self, id: &str) -> Option<&mut Select> {
        self.field_by_id_mut(id)
            .and_then(|field| field.select_mut())
    }

    pub fn checkbox_by_id(&self, id: &str) -> Option<&Checkbox> {
        self.field_by_id(id).and_then(|field| field.checkbox())
    }

    pub fn checkbox_by_id_mut(&mut self, id: &str) -> Option<&mut Checkbox> {
        self.field_by_id_mut(id)
            .and_then(|field| field.checkbox_mut())
    }

    pub fn radio_by_id(&self, id: &str) -> Option<&RadioGroup> {
        self.field_by_id(id).and_then(|field| field.radio())
    }

    pub fn radio_by_id_mut(&mut self, id: &str) -> Option<&mut RadioGroup> {
        self.field_by_id_mut(id).and_then(|field| field.radio_mut())
    }

    /// Whether a checkbox is checked. Unknown and non-checkbox fields are false.
    pub fn checked(&self, id: &str) -> bool {
        self.checkbox_by_id(id)
            .map(Checkbox::is_checked)
            .unwrap_or(false)
    }

    pub fn button_by_id_mut(&mut self, id: &str) -> Option<&mut Button> {
        self.buttons
            .iter_mut()
            .find(|(button_id, _)| button_id == id)
            .map(|(_, button)| button)
    }

    /// The id of the focused widget, if the form has any widgets.
    pub fn focused_id(&self) -> Option<&str> {
        match self.focus? {
            Focus::Field(i) => self.fields.get(i).map(|f| f.id.as_str()),
            Focus::Button(i) => self.buttons.get(i).map(|(id, _)| id.as_str()),
        }
    }

    /// Move focus to a widget by id. Returns whether it was found.
    pub fn focus_on(&mut self, id: &str) -> bool {
        if let Some(i) = self.fields.iter().position(|f| f.id == id) {
            self.set_focus(Focus::Field(i));
            return true;
        }
        if let Some(i) = self
            .buttons
            .iter()
            .position(|(button_id, _)| button_id == id)
        {
            self.set_focus(Focus::Button(i));
            return true;
        }
        false
    }

    fn set_focus(&mut self, focus: Focus) {
        // A dropdown left open over a field the user has moved away from keeps
        // taking up the rows below it with a list nothing is pointing at.
        if self.focus != Some(focus) {
            self.close_open_select();
        }
        self.focus = Some(focus);
        for (i, (_, button)) in self.buttons.iter_mut().enumerate() {
            button.set_focused(focus == Focus::Button(i));
        }
    }

    /// Close whichever select is open, if any. At most one ever is: opening
    /// one requires focus, and moving focus closes the one being left.
    fn close_open_select(&mut self) {
        for field in self.fields.iter_mut() {
            if let Some(select) = field.select_mut() {
                if select.is_open() {
                    select.close();
                }
            }
        }
    }

    /// The area an open dropdown covers, or `None` if none is open.
    ///
    /// A screen that prints its own chrome after [`Form::render`] would paint
    /// over the list; this is how it knows where not to draw.
    pub fn overlay_rect(&self) -> Option<Rect> {
        self.fields
            .iter()
            .filter_map(|field| field.select())
            .find_map(|select| select.overlay_rect())
    }

    fn widget_count(&self) -> usize {
        self.fields.len() + self.buttons.len()
    }

    /// Focus as a flat index over fields-then-buttons.
    fn focus_index(&self) -> Option<usize> {
        match self.focus? {
            Focus::Field(i) => Some(i),
            Focus::Button(i) => Some(self.fields.len() + i),
        }
    }

    fn focus_from_index(&self, index: usize) -> Focus {
        if index < self.fields.len() {
            Focus::Field(index)
        } else {
            Focus::Button(index - self.fields.len())
        }
    }

    /// Move focus one widget forward or back, wrapping.
    ///
    /// Disabled buttons are skipped: focus that lands somewhere it cannot act
    /// looks like the form has stopped responding.
    pub fn move_focus(&mut self, forward: bool) -> FormEvent {
        let count = self.widget_count();
        let Some(current) = self.focus_index() else {
            return FormEvent::Ignored;
        };
        if count == 0 {
            return FormEvent::Ignored;
        }

        let mut index = current;
        for _ in 0..count {
            index = if forward {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            let candidate = self.focus_from_index(index);
            let focusable = match candidate {
                Focus::Field(_) => true,
                Focus::Button(i) => !self.buttons[i].1.is_disabled(),
            };
            if focusable {
                self.set_focus(candidate);
                return FormEvent::FocusMoved;
            }
        }
        FormEvent::Ignored
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> FormEvent {
        // Traversal first — see the module note. A field would otherwise
        // consume Tab as a completion request.
        if key == self.next_key {
            return self.move_focus(true);
        }
        if key == self.prev_key {
            return self.move_focus(false);
        }

        match self.focus {
            Some(Focus::Field(i)) => {
                let Some(field) = self.fields.get_mut(i) else {
                    return FormEvent::Ignored;
                };
                let id = field.id.clone();
                match &mut field.widget {
                    Widget::Text(input) => match input.handle_key(key) {
                        InputAction::Continue => FormEvent::FieldChanged(id),
                        InputAction::Submit => FormEvent::Submit,
                        InputAction::Cancel => FormEvent::Cancel,
                        InputAction::Complete => FormEvent::Complete(id),
                        InputAction::NoAction => FormEvent::Ignored,
                    },
                    Widget::Select(select) => match select.handle_key(key.clone()) {
                        SelectAction::Continue | SelectAction::Committed => {
                            FormEvent::FieldChanged(id)
                        },
                        // Esc closed the dropdown. It did not cancel the form:
                        // one Esc per thing being backed out of.
                        SelectAction::Cancelled => FormEvent::FocusMoved,
                        // A closed select does not take Enter or Esc, so the
                        // form still submits and cancels from one.
                        SelectAction::NoAction => match key.bare_key {
                            BareKey::Enter if key.has_no_modifiers() => FormEvent::Submit,
                            BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                            _ => FormEvent::Ignored,
                        },
                    },
                    Widget::Checkbox(checkbox) => match key.bare_key {
                        BareKey::Char(' ') if key.has_no_modifiers() => {
                            checkbox.toggle();
                            FormEvent::FieldChanged(id)
                        },
                        BareKey::Enter if key.has_no_modifiers() => FormEvent::Submit,
                        BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                        _ => FormEvent::Ignored,
                    },
                    Widget::Radio(radio) => {
                        if radio.handle_key(key.clone()) {
                            FormEvent::FieldChanged(id)
                        } else {
                            match key.bare_key {
                                BareKey::Enter if key.has_no_modifiers() => FormEvent::Submit,
                                BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                                _ => FormEvent::Ignored,
                            }
                        }
                    },
                }
            },
            Some(Focus::Button(i)) => {
                let Some((id, button)) = self.buttons.get(i) else {
                    return FormEvent::Ignored;
                };
                match key.bare_key {
                    BareKey::Enter | BareKey::Char(' ') if key.has_no_modifiers() => {
                        if button.is_disabled() {
                            FormEvent::Ignored
                        } else {
                            FormEvent::ButtonPressed(id.clone())
                        }
                    },
                    BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                    _ => FormEvent::Ignored,
                }
            },
            None => FormEvent::Ignored,
        }
    }

    pub fn handle_mouse(&mut self, mouse: Mouse) -> FormEvent {
        match mouse {
            Mouse::LeftClick(line, column) => {
                if line < 0 {
                    return FormEvent::Ignored;
                }
                self.handle_click(column, line as usize)
            },
            Mouse::Hover(line, column) => {
                if line < 0 {
                    return self.clear_hover();
                }
                self.handle_hover(column, line as usize)
            },
            _ => FormEvent::Ignored,
        }
    }

    fn handle_click(&mut self, x: usize, y: usize) -> FormEvent {
        // The dropdown is drawn on top, so it is hit-tested first: a click that
        // lands on it belongs to it whatever is underneath.
        if let Some(i) = self.open_select_index() {
            let field = &mut self.fields[i];
            let id = field.id.clone();
            let select = field.select_mut().expect("open selects are select fields");
            if let Some(option) = select.option_at(x, y) {
                select.commit_option(option);
                self.set_focus(Focus::Field(i));
                return FormEvent::FieldChanged(id);
            }
            let on_its_own_row = select.row_contains(x, y);
            select.close();
            if on_its_own_row {
                // Clicking the row that opened the list closes it again.
                self.set_focus(Focus::Field(i));
                return FormEvent::FocusMoved;
            }
            // Anything else dismisses the list, and then lands where it was
            // aimed — a click on a button should still press it.
        }

        for (i, (id, button)) in self.buttons.iter().enumerate() {
            if button.contains(x, y) {
                let id = id.clone();
                self.set_focus(Focus::Button(i));
                return FormEvent::ButtonPressed(id);
            }
        }

        for i in 0..self.fields.len() {
            let Some(rect) = self.fields[i].last_rendered else {
                continue;
            };
            if !rect.contains(x, y) {
                continue;
            }
            let field = &mut self.fields[i];
            let prompt = field.prompt();
            let id = field.id.clone();
            let event = match &mut field.widget {
                // Put the cursor where the click landed, not just at the end —
                // clicking into the middle of a word and typing at the end of
                // it is exactly the sort of thing that makes a field feel
                // broken.
                Widget::Text(input) => {
                    let cursor = column_to_cursor(
                        &prompt,
                        input.get_text(),
                        input.get_cursor_position(),
                        rect.columns,
                        x - rect.x,
                    );
                    input.set_cursor_position(cursor);
                    FormEvent::FocusMoved
                },
                // Clicking a collapsed select opens it, which is the only thing
                // there is to do with a row that is a list.
                Widget::Select(select) => {
                    select.open();
                    FormEvent::FocusMoved
                },
                Widget::Checkbox(checkbox) => {
                    checkbox.toggle();
                    FormEvent::FieldChanged(id)
                },
                Widget::Radio(radio) => match radio.option_at(x, y) {
                    Some(option) if radio.select_index(option) => FormEvent::FieldChanged(id),
                    _ => FormEvent::FocusMoved,
                },
            };
            self.set_focus(Focus::Field(i));
            return event;
        }

        FormEvent::Ignored
    }

    /// The field index of the select with an open dropdown, if there is one.
    fn open_select_index(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|field| field.select().map(|s| s.is_open()).unwrap_or(false))
    }

    fn handle_hover(&mut self, x: usize, y: usize) -> FormEvent {
        let mut changed = false;
        if let Some(i) = self.open_select_index() {
            let select = self.fields[i]
                .select_mut()
                .expect("open selects are select fields");
            if let Some(option) = select.option_at(x, y) {
                // Moving the pointer down the list moves the highlight, so
                // clicking and pressing Enter pick the same row.
                changed |= select.highlight_option(option);
            }
        }
        for (_, button) in self.buttons.iter_mut() {
            changed |= button.set_hovered(button.contains(x, y));
        }
        if changed {
            FormEvent::FocusMoved
        } else {
            FormEvent::Ignored
        }
    }

    fn clear_hover(&mut self) -> FormEvent {
        let mut changed = false;
        for (_, button) in self.buttons.iter_mut() {
            changed |= button.set_hovered(false);
        }
        if changed {
            FormEvent::FocusMoved
        } else {
            FormEvent::Ignored
        }
    }

    /// Rows this form draws in: one per field, plus one for the button row.
    pub fn height(&self) -> usize {
        self.fields.len() + if self.buttons.is_empty() { 0 } else { 1 }
    }

    /// Draw the form at `(x, y)`, `width` columns wide, and place the cursor.
    ///
    /// The cursor is placed before anything is printed, which is required: the
    /// host reads the cursor position out of the render, so a `show_cursor`
    /// after a `print_*` lands a frame late.
    pub fn render(&mut self, x: usize, y: usize, width: usize) {
        self.render_rows(x, y, width, self.height(), None);
    }

    /// Draw the form inside a bounded-height viewport.
    ///
    /// When the form is taller than `rows`, the focused widget is kept on
    /// screen. Moving focus with the form's traversal keys is therefore enough
    /// to reach every field and the button row without a second set of scroll
    /// controls.
    pub fn render_in_viewport(&mut self, x: usize, y: usize, width: usize, rows: usize) {
        self.render_rows(x, y, width, rows, Some(y.saturating_add(rows)));
    }

    fn render_rows(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
        overlay_bottom: Option<usize>,
    ) {
        let start = overlay_bottom
            .map(|_| self.viewport_start(rows))
            .unwrap_or(0);
        let end = start.saturating_add(rows).min(self.height());

        for field in &mut self.fields {
            field.last_rendered = None;
            match &mut field.widget {
                Widget::Select(select) => select.clear_rendered(),
                Widget::Radio(radio) => radio.clear_rendered(),
                _ => {},
            }
        }
        for (_, button) in &mut self.buttons {
            button.clear_rendered();
        }

        // 1. Work out where the cursor goes, printing nothing yet.
        let cursor = match self.focus {
            Some(Focus::Field(i)) if i >= start && i < end => {
                self.fields.get(i).and_then(|field| {
                    let prompt = field.prompt();
                    let column = match &field.widget {
                        Widget::Text(input) => {
                            let (_, column) = viewport(
                                &prompt,
                                input.get_text(),
                                input.get_cursor_position(),
                                width,
                            );
                            Some(column)
                        },
                        // A collapsed select is not typed into, so it gets no
                        // cursor; an open one gets it in its filter.
                        Widget::Select(select) => select.cursor_column(&prompt, width),
                        Widget::Checkbox(_) | Widget::Radio(_) => None,
                    };
                    column.map(|column| (x + column, y + i - start))
                })
            },
            // Focus is on a button: there is nothing to type into, and a cursor
            // parked in a field the user is not editing reads as focus being
            // somewhere it is not.
            _ => None,
        };
        show_cursor(cursor);

        // 2. Now draw.
        for (i, field) in self
            .fields
            .iter_mut()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let row_y = y + i - start;
            let prompt = field.prompt();
            match &mut field.widget {
                Widget::Text(input) => {
                    let (line, _) = viewport(
                        &prompt,
                        input.get_text(),
                        input.get_cursor_position(),
                        width,
                    );
                    let prompt_len = prompt.chars().count().min(line.chars().count());
                    let text = Text::new(&line).color_range(2, 0..prompt_len);
                    print_text_with_coordinates(text, x, row_y, Some(width), None);
                },
                Widget::Select(select) => select.render_row(&prompt, x, row_y, width),
                Widget::Checkbox(checkbox) => checkbox.render_row(
                    &prompt,
                    x,
                    row_y,
                    width,
                    self.focus == Some(Focus::Field(i)),
                ),
                Widget::Radio(radio) => radio.render_row(
                    &prompt,
                    x,
                    row_y,
                    width,
                    self.focus == Some(Focus::Field(i)),
                ),
            }
            field.last_rendered = Some(Rect::new(x, row_y, width, 1));
        }

        let button_row = self.fields.len();
        let buttons_are_visible = button_row >= start && button_row < end;
        if !buttons_are_visible {
            for (_, button) in &mut self.buttons {
                button.set_hovered(false);
            }
        }
        if !self.buttons.is_empty() && buttons_are_visible {
            let button_y = y + button_row - start;
            let mut button_x = x;
            for (_, button) in self.buttons.iter_mut() {
                let room = (x + width).saturating_sub(button_x);
                if room == 0 {
                    break;
                }
                button_x += button.render(button_x, button_y, Some(room));
                button_x += 1; // one column between buttons
            }
        }

        // 3. The dropdown last, so it lands on top of the rows it covers.
        for field in self.fields.iter_mut() {
            if let Some(select) = field.select_mut() {
                let max_rows = select
                    .row_rect()
                    .map(|row| {
                        overlay_bottom
                            .map(|bottom| bottom.saturating_sub(row.y + 1))
                            .unwrap_or(usize::MAX)
                    })
                    .unwrap_or(0);
                select.render_overlay(max_rows);
            }
        }
    }

    fn viewport_start(&self, rows: usize) -> usize {
        if rows == 0 {
            return 0;
        }
        let max_start = self.height().saturating_sub(rows);
        let Some(focused_row) = self.focus_index() else {
            return 0;
        };
        let focused_select_is_open = match self.focus {
            Some(Focus::Field(i)) => self.fields[i]
                .select()
                .map(|select| select.is_open())
                .unwrap_or(false),
            _ => false,
        };
        if focused_select_is_open {
            focused_row.min(self.height().saturating_sub(rows - 1))
        } else {
            focused_row
                .saturating_add(1)
                .saturating_sub(rows)
                .min(max_start)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_form() -> Form {
        Form::new()
            .field("name", "Name", TextInput::empty())
            .field("path", "Path", TextInput::empty())
            .button("save", Button::new("Save"))
            .button("cancel", Button::new("Cancel"))
    }

    fn key(bare_key: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(bare_key)
    }

    #[test]
    fn the_first_widget_added_takes_focus() {
        assert_eq!(a_form().focused_id(), Some("name"));
    }

    #[test]
    fn typing_goes_to_the_focused_field() {
        let mut form = a_form();
        let event = form.handle_key(key(BareKey::Char('a')));
        assert_eq!(event, FormEvent::FieldChanged("name".to_string()));
        assert_eq!(form.text("name"), "a");
        assert_eq!(form.text("path"), "");
    }

    #[test]
    fn tab_traverses_every_widget_and_wraps() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .select("kind", "Kind", Select::new(["One", "Two"]))
            .checkbox("frames", "Frames", Checkbox::new(false))
            .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
            .button("save", Button::new("Save"))
            .button("cancel", Button::new("Cancel"));
        for expected in ["kind", "frames", "mode", "save", "cancel", "name"] {
            assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
            assert_eq!(form.focused_id(), Some(expected));
        }
    }

    #[test]
    fn cursor_tracks_the_focused_field_row_after_traversal() {
        use crate::shim::take_last_cursor_position;

        let mut form = Form::new()
            .select("kind", "Kind", Select::new(["One", "Two"]))
            .field("name", "Name", TextInput::empty())
            .field("path", "Path", TextInput::empty());

        form.handle_key(key(BareKey::Tab));
        form.render(2, 5, 40);
        let name_row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
        assert_eq!(take_last_cursor_position(), Some(Some((8, name_row))));

        form.handle_key(key(BareKey::Tab));
        form.render_in_viewport(2, 5, 40, 1);
        let path_row = form.field_by_id("path").unwrap().last_rendered.unwrap().y;
        assert_eq!(take_last_cursor_position(), Some(Some((8, path_row))));
    }

    #[test]
    fn shift_tab_traverses_backwards() {
        let mut form = a_form();
        let back = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
        assert_eq!(form.handle_key(back), FormEvent::FocusMoved);
        assert_eq!(form.focused_id(), Some("cancel"));
    }

    /// Tab reaching the field as a completion request would mean traversal
    /// silently stopped working, which is the failure this ordering prevents.
    #[test]
    fn traversal_beats_the_fields_own_tab_handling() {
        let mut form = Form::new().field("name", "Name", TextInput::empty());
        assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
    }

    #[test]
    fn a_form_with_other_traversal_keys_lets_tab_reach_the_field() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .with_traversal_keys(
                KeyWithModifier::new(BareKey::Down),
                KeyWithModifier::new(BareKey::Up),
            );
        assert_eq!(
            form.handle_key(key(BareKey::Tab)),
            FormEvent::Complete("name".to_string())
        );
    }

    #[test]
    fn unhandled_keys_come_back_as_ignored() {
        let mut form = a_form();
        let event = form.handle_key(KeyWithModifier::new(BareKey::F(5)));
        assert_eq!(event, FormEvent::Ignored);
        assert!(!event.should_render());
    }

    #[test]
    fn enter_in_a_field_submits_and_esc_cancels() {
        let mut form = a_form();
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Submit);
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn enter_on_a_button_presses_it() {
        let mut form = a_form();
        form.focus_on("save");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::ButtonPressed("save".to_string())
        );
    }

    #[test]
    fn traversal_skips_a_disabled_button() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .button("save", Button::new("Save").disabled(true))
            .button("cancel", Button::new("Cancel"));
        form.handle_key(key(BareKey::Tab));
        assert_eq!(form.focused_id(), Some("cancel"));
    }

    #[test]
    fn a_disabled_button_cannot_be_pressed_by_key() {
        let mut form = Form::new().button("save", Button::new("Save").disabled(true));
        form.focus_on("save");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Ignored);
    }

    #[test]
    fn an_empty_form_ignores_everything() {
        let mut form = Form::new();
        assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::Ignored);
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Ignored);
        assert_eq!(form.focused_id(), None);
    }

    #[test]
    fn a_click_before_any_render_hits_nothing() {
        let mut form = a_form();
        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(0, 0)),
            FormEvent::Ignored,
            "a form that never rendered has no rectangles to hit"
        );
    }

    #[test]
    fn a_negative_click_line_is_ignored() {
        let mut form = a_form();
        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(-1, 0)),
            FormEvent::Ignored
        );
    }

    #[test]
    fn height_counts_the_button_row_once() {
        assert_eq!(a_form().height(), 3);
        assert_eq!(
            Form::new().field("a", "A", TextInput::empty()).height(),
            1,
            "no buttons means no button row"
        );
    }

    #[test]
    fn a_bounded_form_follows_the_focused_row() {
        let mut form = a_form();
        assert_eq!(form.viewport_start(2), 0);
        form.focus_on("save");
        assert_eq!(form.viewport_start(2), 1);
        form.focus_on("cancel");
        assert_eq!(form.viewport_start(2), 1, "buttons share one row");
        form.focus_on("name");
        assert_eq!(form.viewport_start(2), 0);
    }

    #[test]
    fn widgets_outside_the_viewport_lose_their_hit_targets() {
        let mut form = a_form();
        form.render(0, 0, 40);
        assert!(form.fields[0].last_rendered.is_some());
        assert!(form.buttons[0].1.last_rendered().is_some());

        form.focus_on("path");
        form.render_in_viewport(0, 0, 40, 1);
        assert_eq!(form.fields[0].last_rendered, None);
        assert!(form.fields[1].last_rendered.is_some());
        assert_eq!(form.buttons[0].1.last_rendered(), None);
    }

    #[test]
    fn focus_on_an_unknown_id_reports_failure_and_changes_nothing() {
        let mut form = a_form();
        assert!(!form.focus_on("nope"));
        assert_eq!(form.focused_id(), Some("name"));
    }

    #[test]
    fn text_of_an_unknown_field_is_empty() {
        assert_eq!(a_form().text("nope"), "");
    }

    fn a_form_with_a_select() -> Form {
        Form::new()
            .field("name", "Name", TextInput::empty())
            .select(
                "layout",
                "Layout",
                Select::new(["default", "compact", "dev-three-pane"]),
            )
            .button("save", Button::new("Save"))
    }

    #[test]
    fn a_select_field_reads_back_as_its_chosen_option() {
        let form = a_form_with_a_select();
        assert_eq!(form.text("layout"), "default");
        assert_eq!(
            form.select_by_id("layout").and_then(|s| s.selected_index()),
            Some(0)
        );
    }

    #[test]
    fn typing_into_a_focused_select_opens_and_filters_it() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        let event = form.handle_key(key(BareKey::Char('c')));
        assert_eq!(event, FormEvent::FieldChanged("layout".to_string()));
        let select = form.select_by_id("layout").unwrap();
        assert!(select.is_open());
        assert_eq!(select.filter_text(), "c");
    }

    #[test]
    fn enter_commits_the_dropdown_rather_than_submitting_the_form() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char('c')));
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("layout".to_string())
        );
        assert_eq!(form.text("layout"), "compact");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::Submit,
            "with the dropdown closed, Enter submits again"
        );
    }

    /// Esc has two things to back out of here, and the inner one goes first.
    #[test]
    fn esc_closes_the_dropdown_before_it_cancels_the_form() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
        assert!(!form.select_by_id("layout").unwrap().is_open());
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn traversing_away_closes_an_open_dropdown() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        assert!(form.select_by_id("layout").unwrap().is_open());
        form.handle_key(key(BareKey::Tab));
        assert_eq!(form.focused_id(), Some("save"));
        assert!(
            !form.select_by_id("layout").unwrap().is_open(),
            "the dropdown outlived the focus that opened it"
        );
    }

    #[test]
    fn an_open_dropdown_does_not_change_the_forms_height() {
        let mut form = a_form_with_a_select();
        let closed_height = form.height();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        assert_eq!(form.height(), closed_height);
    }

    #[test]
    fn there_is_no_overlay_until_a_dropdown_has_been_drawn() {
        let mut form = a_form_with_a_select();
        assert_eq!(form.overlay_rect(), None);
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        assert_eq!(
            form.overlay_rect(),
            None,
            "a list that has never rendered covers nothing"
        );
    }

    #[test]
    fn a_dropdown_is_clipped_to_the_form_viewport() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        form.render_in_viewport(0, 0, 40, 2);
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 1, 40, 1)));

        form.render_in_viewport(0, 0, 40, 1);
        assert_eq!(form.overlay_rect(), None);
    }

    #[test]
    fn a_bottom_row_dropdown_reserves_space_in_the_viewport() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .select("layout", "Layout", Select::new(["default", "compact"]));
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        form.handle_key(key(BareKey::Down));

        form.render_in_viewport(0, 0, 40, 2);

        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 1, 40, 1)));
        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(1, 0)),
            FormEvent::FieldChanged("layout".to_string())
        );
        assert_eq!(form.text("layout"), "compact");
    }

    #[test]
    fn the_original_render_keeps_dropdowns_unbounded() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        form.render(0, 0, 40);
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 3)));
    }

    fn a_form_with_choices() -> Form {
        Form::new()
            .checkbox("frames", "Frames", Checkbox::new(false))
            .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
            .button("save", Button::new("Save"))
    }

    #[test]
    fn space_toggles_a_checkbox_and_typed_accessors_find_it() {
        let mut form = a_form_with_choices();
        assert!(!form.checked("frames"));
        assert_eq!(
            form.handle_key(key(BareKey::Char(' '))),
            FormEvent::FieldChanged("frames".to_string())
        );
        assert!(form.checked("frames"));
        assert!(form.checkbox_by_id_mut("frames").is_some());
        assert!(!form.checked("missing"));
    }

    #[test]
    fn arrows_change_a_radio_and_text_reads_its_option() {
        let mut form = a_form_with_choices();
        form.focus_on("mode");
        assert_eq!(
            form.handle_key(key(BareKey::Right)),
            FormEvent::FieldChanged("mode".to_string())
        );
        assert_eq!(form.text("mode"), "Two");
        assert_eq!(
            form.radio_by_id("mode")
                .and_then(RadioGroup::selected_index),
            Some(1)
        );
        assert!(form.radio_by_id_mut("mode").is_some());
    }

    #[test]
    fn clicking_choices_changes_only_when_the_value_changes() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(0, 0)),
            FormEvent::FieldChanged("frames".to_string())
        );
        assert!(form.checked("frames"));

        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(1, 14)),
            FormEvent::FieldChanged("mode".to_string())
        );
        assert_eq!(form.text("mode"), "Two");
        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(1, 14)),
            FormEvent::FocusMoved
        );
    }

    #[test]
    fn enter_still_submits_from_choice_fields() {
        let mut form = a_form_with_choices();
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Submit);
        form.focus_on("mode");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Submit);
    }
}
