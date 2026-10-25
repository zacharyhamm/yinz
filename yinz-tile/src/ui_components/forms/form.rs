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
//! Arrows traverse inactive controls. Enter activates a control; active controls
//! own their arrows until Enter, Esc, or traversal ends the interaction.
//! Tab and Shift-Tab always traverse. Additional traversal keys are configurable,
//! but cannot take arrows from an active control. Only buttons submit a form.

use super::button::{Button, Rect};
use super::choice::{Checkbox, RadioGroup};
use super::combo::{Combo, ComboAction};
use super::dropdown::HasDropdown;
use super::label::Label;
use super::select::{Select, SelectAction};
use super::text_input::{InputAction, TextInput};
use super::viewport::{column_to_cursor, viewport};
use super::RenderBuffer;
use crate::prelude::*;
use crate::ui_components::Text;

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
    /// Focus moved between widgets, or the focused widget's interaction state
    /// changed - a dropdown opened or closed, a text field entered or left
    /// input mode. Nothing a caller reads has a new value.
    FocusMoved,
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
    Combo(Combo),
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

    pub fn new_combo<I: ToString, L: ToString>(id: I, label: L, combo: Combo) -> Self {
        Field {
            id: id.to_string(),
            label: Label::new(label),
            widget: Widget::Combo(combo),
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

    /// The text editor behind this field, or `None` for a widget with no text
    /// in it.
    ///
    /// A [`Combo`] answers with its own editor: its value *is* typed text, so
    /// everything a form does to a text field - place the caret where a click
    /// landed, take a paste, enter and leave input mode - it wants done to it
    /// too.
    pub fn input(&self) -> Option<&TextInput> {
        match &self.widget {
            Widget::Text(input) => Some(input),
            Widget::Combo(combo) => Some(combo.input()),
            _ => None,
        }
    }

    pub fn input_mut(&mut self) -> Option<&mut TextInput> {
        match &mut self.widget {
            Widget::Text(input) => Some(input),
            Widget::Combo(combo) => Some(combo.input_mut()),
            _ => None,
        }
    }

    /// The combo behind this field, or `None` for another widget type.
    pub fn combo(&self) -> Option<&Combo> {
        match &self.widget {
            Widget::Combo(combo) => Some(combo),
            _ => None,
        }
    }

    pub fn combo_mut(&mut self) -> Option<&mut Combo> {
        match &mut self.widget {
            Widget::Combo(combo) => Some(combo),
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

    pub fn is_combo(&self) -> bool {
        matches!(self.widget, Widget::Combo(_))
    }

    /// The widget's dropdown, for the parts of a form that mean "the field with
    /// a list" rather than any particular kind of field.
    fn dropdown(&self) -> Option<&dyn HasDropdown> {
        match &self.widget {
            Widget::Select(select) => Some(select),
            Widget::Combo(combo) => Some(combo),
            _ => None,
        }
    }

    fn dropdown_mut(&mut self) -> Option<&mut dyn HasDropdown> {
        match &mut self.widget {
            Widget::Select(select) => Some(select),
            Widget::Combo(combo) => Some(combo),
            _ => None,
        }
    }

    /// Hand a key to the editor behind this field, keeping whatever else the
    /// widget hangs off a text change - a combo's list of suggestions.
    fn handle_input_key(&mut self, key: KeyWithModifier) -> InputAction {
        match &mut self.widget {
            Widget::Text(input) => input.handle_key(key),
            Widget::Combo(combo) => match combo.handle_key(key) {
                ComboAction::Continue | ComboAction::Committed | ComboAction::Cancelled => {
                    InputAction::Continue
                },
                ComboAction::Input(action) => action,
            },
            _ => InputAction::NoAction,
        }
    }

    /// What this field currently holds: typed text or a chosen select/radio option.
    ///
    /// A select or radio group with nothing chosen reads as `""`.
    pub fn text(&self) -> &str {
        match &self.widget {
            Widget::Text(input) => input.get_text(),
            Widget::Combo(combo) => combo.text(),
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
    radio_active: bool,
    editing: bool,
    pressed: Option<PressTarget>,
    first_button: usize,
}

/// What a left button went down on, remembered until it comes back up.
///
/// A widget acts on release rather than on press, and only if the release lands
/// back on the same widget - the way a mis-aimed click is taken back everywhere
/// else. The press itself still moves focus, so the row the pointer is on
/// highlights immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressTarget {
    Button(usize),
    Checkbox(usize),
    /// Field index and the option under the pointer.
    Radio(usize, usize),
    /// The row of a widget with a list: released on it, a closed list opens and
    /// an open one closes again.
    DropdownRow(usize),
    /// Field index and the option in the open dropdown.
    DropdownOption(usize, usize),
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
            radio_active: false,
            editing: false,
            pressed: None,
            first_button: 0,
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

    /// Add a combo field: free text with a dropdown of suggestions. The first
    /// widget added takes focus.
    pub fn combo<I: ToString, L: ToString>(mut self, id: I, label: L, combo: Combo) -> Self {
        self.fields.push(Field::new_combo(id, label, combo));
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

    /// Whether the focused text field is in input mode.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Whether the focused radio group owns arrow keys.
    pub fn is_radio_active(&self) -> bool {
        self.radio_active
    }

    /// Add traversal bindings alongside Tab / Shift-Tab and inactive arrows.
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

    pub fn combo_by_id(&self, id: &str) -> Option<&Combo> {
        self.field_by_id(id).and_then(|field| field.combo())
    }

    pub fn combo_by_id_mut(&mut self, id: &str) -> Option<&mut Combo> {
        self.field_by_id_mut(id).and_then(|field| field.combo_mut())
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
            self.close_open_dropdown();
        }
        // Unguarded, unlike the dropdown above: `focus_on` onto the field that
        // already has focus is a request to start over on it, not a no-op.
        self.editing = false;
        self.radio_active = false;
        self.focus = Some(focus);
        for (i, (_, button)) in self.buttons.iter_mut().enumerate() {
            button.set_focused(focus == Focus::Button(i));
        }
    }

    /// Close whichever dropdown is open, if any. At most one ever is: opening
    /// one requires focus, and moving focus closes the one being left.
    fn close_open_dropdown(&mut self) {
        for field in self.fields.iter_mut() {
            if let Some(dropdown) = field.dropdown_mut()
                && dropdown.is_open()
            {
                dropdown.close();
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
            .filter_map(|field| field.dropdown())
            .find_map(|dropdown| dropdown.overlay_rect())
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
                self.close_open_dropdown();
                self.set_focus(candidate);
                return FormEvent::FocusMoved;
            }
        }
        FormEvent::Ignored
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> FormEvent {
        if key == KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier() {
            return FormEvent::Cancel;
        }
        let arrow = matches!(
            key.bare_key,
            BareKey::Up | BareKey::Down | BareKey::Left | BareKey::Right
        );
        let active = self.editing || self.radio_active || self.open_dropdown_index().is_some();
        if key == KeyWithModifier::new(BareKey::Tab) {
            return self.move_focus(true);
        }
        if key == KeyWithModifier::new(BareKey::Tab).with_shift_modifier() {
            return self.move_focus(false);
        }
        if !(active && arrow) {
            if key == self.next_key {
                return self.move_focus(true);
            }
            if key == self.prev_key {
                return self.move_focus(false);
            }
            if arrow && key.has_no_modifiers() {
                return self.move_focus(matches!(key.bare_key, BareKey::Down | BareKey::Right));
            }
        }
        if key.bare_key == BareKey::Enter && !key.has_no_modifiers() {
            return FormEvent::Ignored;
        }
        if !active && key == KeyWithModifier::new(BareKey::Esc) {
            return FormEvent::Cancel;
        }
        let event = self.handle_control_key(key);
        if active && arrow && event == FormEvent::Ignored {
            FormEvent::FocusMoved
        } else {
            event
        }
    }

    fn handle_control_key(&mut self, key: KeyWithModifier) -> FormEvent {
        match self.focus {
            Some(Focus::Field(i)) => {
                // A combo with its list open is the one field where Enter and
                // Esc belong to the list rather than to the input mode below.
                if self
                    .fields
                    .get(i)
                    .and_then(Field::combo)
                    .map(|combo| combo.is_open())
                    == Some(true)
                {
                    return self.handle_key_combo(i, key);
                }
                // Text is split out because it is the one widget whose Enter
                // changes form-level state, which cannot be done while a field
                // is borrowed out of `self.fields`. A combo goes the same way:
                // closed, it is a text field with a list waiting behind it.
                if self.fields.get(i).map(|field| field.input().is_some()) == Some(true) {
                    return self.handle_key_text(i, key);
                }
                let Some(field) = self.fields.get_mut(i) else {
                    return FormEvent::Ignored;
                };
                let id = field.id.clone();
                match &mut field.widget {
                    // Handled above; `input()` said these have no editor.
                    Widget::Text(_) | Widget::Combo(_) => FormEvent::Ignored,
                    Widget::Select(select) => {
                        let before = select.selected_index();
                        let event = match select.handle_key(key.clone()) {
                            SelectAction::Continue
                            | SelectAction::Committed
                            | SelectAction::Cancelled => FormEvent::FocusMoved,
                            SelectAction::NoAction => match key.bare_key {
                                BareKey::Enter if key.has_no_modifiers() => {
                                    select.open();
                                    FormEvent::FocusMoved
                                },
                                BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                                _ => FormEvent::Ignored,
                            },
                        };
                        if before != select.selected_index() {
                            FormEvent::FieldChanged(id)
                        } else {
                            event
                        }
                    },
                    Widget::Checkbox(checkbox) => match key.bare_key {
                        BareKey::Char(' ') | BareKey::Enter if key.has_no_modifiers() => {
                            checkbox.toggle();
                            FormEvent::FieldChanged(id)
                        },
                        BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                        _ => FormEvent::Ignored,
                    },
                    Widget::Radio(radio) => match key.bare_key {
                        BareKey::Enter | BareKey::Char(' ') if key.has_no_modifiers() => {
                            self.radio_active = !self.radio_active;
                            FormEvent::FocusMoved
                        },
                        BareKey::Esc if key.has_no_modifiers() && self.radio_active => {
                            self.radio_active = false;
                            FormEvent::FocusMoved
                        },
                        BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                        _ if self.radio_active && radio.handle_key(key) => {
                            FormEvent::FieldChanged(id)
                        },
                        _ => FormEvent::Ignored,
                    },
                }
            },
            // Buttons are unchanged: this is the one place Enter still acts on
            // the form as a whole.
            Some(Focus::Button(_)) | None => self.handle_key_unfocused_or_button(key),
        }
    }

    /// Enter starts or ends text editing, preserving the value.
    fn handle_key_text(&mut self, i: usize, key: KeyWithModifier) -> FormEvent {
        let editing = self.editing;
        if key.bare_key == BareKey::Enter {
            // `TextInput`'s bare-key arm ignores modifiers, so an Alt-Enter
            // reaching it would come back as the `Submit` this form promises
            // never to emit. Left for the plugin instead.
            if !key.has_no_modifiers() {
                return FormEvent::Ignored;
            }
            self.editing = !editing;
            if self.editing
                && let Some(combo) = self.fields[i].combo_mut()
                && !combo.suggestions().is_empty()
            {
                combo.open();
            }
            return FormEvent::FocusMoved;
        }
        if key.bare_key == BareKey::Esc && key.has_no_modifiers() {
            // One Esc per thing being backed out of, as with an open dropdown:
            // the input mode first, the dialog only once it is left.
            if editing {
                self.editing = false;
                return FormEvent::FocusMoved;
            }
            return FormEvent::Cancel;
        }
        if !editing && !matches!(key.bare_key, BareKey::Char(_) if key.has_no_modifiers()) {
            return FormEvent::Ignored;
        }
        // Typing is intent enough: a character that fell on the floor because
        // the field was not in input mode would read as a dropped keystroke.
        // It still goes through `handle_key`, so which character started the
        // edit makes no difference to undo coalescing.
        self.editing = true;
        let Some(field) = self.fields.get_mut(i) else {
            return FormEvent::Ignored;
        };
        let id = field.id.clone();
        let before = field.text().to_owned();
        match field.handle_input_key(key) {
            InputAction::Continue if field.text() != before => FormEvent::FieldChanged(id),
            InputAction::Continue => FormEvent::FocusMoved,
            InputAction::Cancel => FormEvent::Cancel,
            InputAction::Complete => FormEvent::Complete(id),
            // Enter was dealt with above, whatever its modifiers.
            InputAction::Submit => FormEvent::FocusMoved,
            InputAction::NoAction => FormEvent::Ignored,
        }
    }

    /// A focused combo with its list open.
    ///
    /// Enter takes the highlighted suggestion, or - with nothing highlighted -
    /// leaves the input mode the way Enter does in any other field. Esc closes
    /// the list first, the input mode second, the dialog third.
    fn handle_key_combo(&mut self, i: usize, key: KeyWithModifier) -> FormEvent {
        let Some(field) = self.fields.get_mut(i) else {
            return FormEvent::Ignored;
        };
        let id = field.id.clone();
        let Some(combo) = field.combo_mut() else {
            return FormEvent::Ignored;
        };
        let before = combo.text().to_owned();
        let event = match combo.handle_key(key) {
            ComboAction::Continue => FormEvent::FocusMoved,
            ComboAction::Committed => {
                self.editing = false;
                FormEvent::FocusMoved
            },
            ComboAction::Cancelled => FormEvent::FocusMoved,
            // The list handed the key back: it is the input mode's now, and
            // that is form-level state.
            ComboAction::Input(InputAction::Submit) => {
                self.editing = false;
                FormEvent::FocusMoved
            },
            ComboAction::Input(InputAction::Continue) => FormEvent::FocusMoved,
            ComboAction::Input(InputAction::Cancel) => FormEvent::Cancel,
            ComboAction::Input(InputAction::Complete) => return FormEvent::Complete(id),
            ComboAction::Input(InputAction::NoAction) => FormEvent::Ignored,
        };
        if combo.text() != before {
            FormEvent::FieldChanged(id)
        } else {
            event
        }
    }

    fn handle_key_unfocused_or_button(&mut self, key: KeyWithModifier) -> FormEvent {
        let Some(Focus::Button(i)) = self.focus else {
            return FormEvent::Ignored;
        };
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
    }

    /// Insert pasted text into the focused text field, entering input mode.
    ///
    /// Returns the field it landed in, or `None` when the focused widget is not
    /// something typed into.
    pub fn insert_pasted_text(&mut self, text: &str) -> Option<FieldId> {
        let Some(Focus::Field(i)) = self.focus else {
            return None;
        };
        let field = self.fields.get_mut(i)?;
        let id = field.id.clone();
        match &mut field.widget {
            Widget::Text(input) => input.insert_text(text),
            // Through the combo rather than its editor, so the suggestions
            // follow the pasted text like they follow a typed character.
            Widget::Combo(combo) => combo.insert_pasted_text(text),
            _ => return None,
        }
        // Text arriving in a field that shows no cursor reads as a paste that
        // went somewhere else.
        self.editing = true;
        Some(id)
    }

    /// Route a mouse event.
    ///
    /// `Mouse::LeftClick` is the *press* - the host sends it when the button goes
    /// down - so nothing is activated by it. `Mouse::Release` is what toggles a
    /// checkbox, picks a radio option or fires a button, and only when it lands
    /// back on the widget the press did. `Mouse::Hold` is the drag between them,
    /// and only moves the highlight so that dragging off a button visibly takes
    /// the press back.
    pub fn handle_mouse(&mut self, mouse: Mouse) -> FormEvent {
        match mouse {
            Mouse::LeftClick(line, column) => {
                if line < 0 {
                    self.pressed = None;
                    return FormEvent::Ignored;
                }
                self.handle_press(column, line as usize)
            },
            Mouse::Hold(line, column) => {
                if line < 0 {
                    return self.clear_hover();
                }
                self.handle_hover(column, line as usize)
            },
            Mouse::Release(line, column) => {
                if line < 0 {
                    self.pressed = None;
                    return FormEvent::Ignored;
                }
                self.handle_release(column, line as usize)
            },
            Mouse::Hover(line, column) => {
                // A hover is buttonless motion, so any press that has not been
                // released by now was released somewhere this form never heard
                // about - off the pane, most likely.
                self.pressed = None;
                if line < 0 {
                    return self.clear_hover();
                }
                self.handle_hover(column, line as usize)
            },
            _ => FormEvent::Ignored,
        }
    }

    /// The button going down: move focus to whatever is under the pointer and
    /// remember it, without acting on it.
    fn handle_press(&mut self, x: usize, y: usize) -> FormEvent {
        self.pressed = None;

        // The dropdown is drawn on top, so it is hit-tested first: a press that
        // lands on it belongs to it whatever is underneath.
        if let Some(i) = self.open_dropdown_index() {
            let dropdown = self.fields[i]
                .dropdown_mut()
                .expect("an open list belongs to a field that has one");
            if let Some(option) = dropdown.option_at(x, y) {
                self.pressed = Some(PressTarget::DropdownOption(i, option));
                self.set_focus(Focus::Field(i));
                return FormEvent::FocusMoved;
            }
            if !dropdown.row_contains(x, y) {
                // Anything off the widget dismisses the list at once - a list
                // left open under a pointer that has already moved on is a list
                // in the way - and then the press lands where it was aimed. A
                // press on the row itself is left to the loop below, which knows
                // whether that row is a select to toggle or a combo to put the
                // caret in.
                dropdown.close();
            }
        }

        for i in 0..self.buttons.len() {
            if self.buttons[i].1.contains(x, y) {
                self.pressed = Some(PressTarget::Button(i));
                self.set_focus(Focus::Button(i));
                return FormEvent::FocusMoved;
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
            match &mut field.widget {
                // Put the cursor where the press landed, not just at the end —
                // clicking into the middle of a word and typing at the end of
                // it is exactly the sort of thing that makes a field feel
                // broken. Text is the one widget with nothing to activate, so
                // this is all a press on it ever does.
                Widget::Text(input) => {
                    let cursor = column_to_cursor(
                        &prompt,
                        input.get_text(),
                        input.get_cursor_position(),
                        rect.columns,
                        x - rect.x,
                    );
                    input.set_cursor_position(cursor);
                },
                Widget::Select(_) => self.pressed = Some(PressTarget::DropdownRow(i)),
                // A combo is a text field with a marker on the end: the marker
                // is the control, and the rest of the row is text to put the
                // caret in.
                Widget::Combo(combo) => {
                    if combo.marker_contains(x) {
                        self.pressed = Some(PressTarget::DropdownRow(i));
                    } else {
                        let input = combo.input_mut();
                        let cursor = column_to_cursor(
                            &prompt,
                            input.get_text(),
                            input.get_cursor_position(),
                            rect.columns,
                            x - rect.x,
                        );
                        input.set_cursor_position(cursor);
                    }
                },
                Widget::Checkbox(_) => self.pressed = Some(PressTarget::Checkbox(i)),
                Widget::Radio(radio) => {
                    if let Some(option) = radio.option_at(x, y) {
                        self.pressed = Some(PressTarget::Radio(i, option));
                    }
                },
            }
            let radio_active = self.radio_active
                && self.focus == Some(Focus::Field(i))
                && matches!(self.pressed, Some(PressTarget::Radio(_, _)));
            self.set_focus(Focus::Field(i));
            // Keep the active radio's options in place across a render before release.
            self.radio_active = radio_active;
            // After `set_focus`, which clears input mode. Placing the caret
            // where the press landed and then hiding it would be pointless: a
            // click into a field is intent enough to start typing in it.
            if self.fields[i].input().is_some() {
                self.editing = true;
            }
            return FormEvent::FocusMoved;
        }

        FormEvent::Ignored
    }

    /// The button coming back up. Only a release on the widget the press went
    /// down on does anything.
    fn handle_release(&mut self, x: usize, y: usize) -> FormEvent {
        let Some(pressed) = self.pressed.take() else {
            return FormEvent::Ignored;
        };
        match pressed {
            PressTarget::Button(i) => {
                let Some((id, button)) = self.buttons.get(i) else {
                    return FormEvent::Ignored;
                };
                if !button.contains(x, y) {
                    return FormEvent::Ignored;
                }
                FormEvent::ButtonPressed(id.clone())
            },
            PressTarget::Checkbox(i) => {
                if !self.field_row_contains(i, x, y) {
                    return FormEvent::Ignored;
                }
                let field = &mut self.fields[i];
                let id = field.id.clone();
                let Some(checkbox) = field.checkbox_mut() else {
                    return FormEvent::Ignored;
                };
                checkbox.toggle();
                FormEvent::FieldChanged(id)
            },
            PressTarget::Radio(i, option) => {
                let field = &mut self.fields[i];
                let id = field.id.clone();
                let Some(radio) = field.radio_mut() else {
                    return FormEvent::Ignored;
                };
                if radio.option_at(x, y) != Some(option) {
                    return FormEvent::Ignored;
                }
                if radio.select_index(option) {
                    FormEvent::FieldChanged(id)
                } else {
                    // Released on the option that was already selected.
                    FormEvent::Ignored
                }
            },
            PressTarget::DropdownRow(i) => {
                let on_row = self.field_row_contains(i, x, y);
                let Some(dropdown) = self.fields.get_mut(i).and_then(Field::dropdown_mut) else {
                    return FormEvent::Ignored;
                };
                if dropdown.is_open() {
                    if !dropdown.row_contains(x, y) {
                        return FormEvent::Ignored;
                    }
                    dropdown.close();
                } else {
                    if !on_row {
                        return FormEvent::Ignored;
                    }
                    dropdown.open();
                }
                FormEvent::FocusMoved
            },
            PressTarget::DropdownOption(i, option) => {
                let field = &mut self.fields[i];
                let id = field.id.clone();
                let before = (
                    field.text().to_owned(),
                    field.select().and_then(Select::selected_index),
                );
                let Some(dropdown) = field.dropdown_mut() else {
                    return FormEvent::Ignored;
                };
                if dropdown.option_at(x, y) != Some(option) {
                    return FormEvent::Ignored;
                }
                dropdown.commit_option(option);
                self.editing = false;
                if before
                    != (
                        field.text().to_owned(),
                        field.select().and_then(Select::selected_index),
                    )
                {
                    FormEvent::FieldChanged(id)
                } else {
                    FormEvent::FocusMoved
                }
            },
        }
    }

    fn field_row_contains(&self, i: usize, x: usize, y: usize) -> bool {
        self.fields
            .get(i)
            .and_then(|field| field.last_rendered)
            .map(|rect| rect.contains(x, y))
            .unwrap_or(false)
    }

    /// The field index of the widget with an open dropdown, if there is one.
    fn open_dropdown_index(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|field| field.dropdown().map(|d| d.is_open()).unwrap_or(false))
    }

    fn handle_hover(&mut self, x: usize, y: usize) -> FormEvent {
        let mut changed = false;
        if let Some(i) = self.open_dropdown_index() {
            let dropdown = self.fields[i]
                .dropdown_mut()
                .expect("an open list belongs to a field that has one");
            if let Some(option) = dropdown.option_at(x, y) {
                // Moving the pointer down the list moves the highlight, so
                // clicking and pressing Enter pick the same row.
                changed |= dropdown.highlight_option(option);
            }
        }
        for (_, button) in self.buttons.iter_mut() {
            changed |= button.set_hovered(button.contains(x, y));
        }
        // Checkbox and radio rows follow the pointer too. Without it those are
        // the only clickable things in a dialog that give no sign of being
        // clickable at all.
        for i in 0..self.fields.len() {
            let on_row = self.field_row_contains(i, x, y);
            match &mut self.fields[i].widget {
                Widget::Checkbox(checkbox) => changed |= checkbox.set_hovered(on_row),
                Widget::Radio(radio) => {
                    let option = if on_row { radio.option_at(x, y) } else { None };
                    changed |= radio.set_hovered(option);
                },
                _ => {},
            }
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
        for field in self.fields.iter_mut() {
            match &mut field.widget {
                Widget::Checkbox(checkbox) => changed |= checkbox.set_hovered(false),
                Widget::Radio(radio) => changed |= radio.set_hovered(None),
                _ => {},
            }
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

    /// Return the form output and cursor update for delivery by the caller.
    pub fn render(&mut self, x: usize, y: usize, width: usize) -> RenderBuffer {
        self.render_buffer(x, y, width)
    }

    /// Draw the form inside a bounded-height viewport.
    ///
    /// When the form is taller than `rows`, the focused widget is kept on
    /// screen. Moving focus with the form's traversal keys is therefore enough
    /// to reach every field and the button row without a second set of scroll
    /// controls.
    pub fn render_in_viewport(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
    ) -> RenderBuffer {
        self.render_buffer_in_viewport(x, y, width, rows)
    }

    /// Draw in a `rows`-tall viewport, letting an open dropdown overlay down to
    /// `overlay_bottom`.
    ///
    /// The rows between the viewport's own bottom and `overlay_bottom` belong to
    /// the caller - a dialog's blank, status and help lines, say - and it has to
    /// keep whatever it draws there off the list; [`Form::overlay_rect`] says
    /// where the list ended up. The alternative is for the form to scroll to
    /// make room, which moves every field the user is looking at.
    pub fn render_in_viewport_with_overlay(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
        overlay_bottom: usize,
    ) -> RenderBuffer {
        self.render_buffer_with_overlay(x, y, width, rows, overlay_bottom)
    }

    /// Render output and cursor metadata without host calls.
    pub fn render_buffer(&mut self, x: usize, y: usize, width: usize) -> RenderBuffer {
        self.render_rows(x, y, width, self.height(), None)
    }

    /// As [`Self::render_in_viewport`], returning output and the cursor update.
    pub fn render_buffer_in_viewport(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
    ) -> RenderBuffer {
        self.render_rows(x, y, width, rows, Some(y.saturating_add(rows)))
    }

    /// As [`Self::render_in_viewport_with_overlay`], without output delivery.
    pub fn render_buffer_with_overlay(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
        overlay_bottom: usize,
    ) -> RenderBuffer {
        self.render_rows(
            x,
            y,
            width,
            rows,
            Some(overlay_bottom.max(y.saturating_add(rows))),
        )
    }

    fn render_rows(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        rows: usize,
        overlay_bottom: Option<usize>,
    ) -> RenderBuffer {
        let mut buffer = RenderBuffer::default();
        // `render` asks for every row it has, so this is 0 there: the scroll and
        // the overlay's bottom edge are separate questions.
        let start = self.viewport_start(rows);
        let end = start.saturating_add(rows).min(self.height());

        for field in &mut self.fields {
            field.last_rendered = None;
            match &mut field.widget {
                Widget::Select(select) => select.clear_rendered(),
                Widget::Combo(combo) => combo.clear_rendered(),
                Widget::Radio(radio) => radio.clear_rendered(),
                Widget::Checkbox(checkbox) => checkbox.clear_rendered(),
                _ => {},
            }
        }
        for (_, button) in &mut self.buttons {
            button.clear_rendered();
        }

        // 1. Work out where the cursor goes, printing nothing yet.
        // Read out here: `self` is borrowed for the field below.
        let typing = self.editing;
        let cursor = match self.focus {
            Some(Focus::Field(i)) if i >= start && i < end => {
                self.fields.get(i).and_then(|field| {
                    let prompt = field.prompt();
                    let column = match &field.widget {
                        // A field that is not in input mode is not being typed
                        // into, and a cursor sitting in one says it is.
                        Widget::Text(_) if !typing => None,
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
                        // A combo is typed into whether its list is open or
                        // not, so it keeps the caret the whole time - but only
                        // once the field is in input mode, like any text field.
                        Widget::Combo(_) if !typing => None,
                        Widget::Combo(combo) => Some(combo.cursor_column(&prompt, width)),
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
        buffer.cursor = Some(cursor);
        if width == 0 || rows == 0 {
            buffer.cursor = Some(None);
            return buffer;
        }

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
            let focused = self.focus == Some(Focus::Field(i));
            match &mut field.widget {
                Widget::Text(input) => {
                    let (line, _) = viewport(
                        &prompt,
                        input.get_text(),
                        input.get_cursor_position(),
                        width,
                    );
                    let prompt_len = prompt.chars().count().min(line.chars().count());
                    let mut text = Text::new(&line).color_range(2, 0..prompt_len);
                    if focused {
                        // Dimming the value of a focused-but-not-editing field
                        // is what a collapsed select already does with an
                        // unchosen one: the row is where you are, but nothing
                        // you type is going into it yet.
                        if typing {
                            text = text.color_range(0, prompt_len..);
                        } else {
                            text = text.dim_range(prompt_len..);
                        }
                        text = text.selected();
                    }
                    buffer.text(text, x, row_y, Some(width), None);
                },
                Widget::Select(select) => {
                    select.render_row(&mut buffer, &prompt, x, row_y, width, focused)
                },
                Widget::Combo(combo) => {
                    combo.render_row(&mut buffer, &prompt, x, row_y, width, focused, typing)
                },
                Widget::Checkbox(checkbox) => {
                    checkbox.render_row(&mut buffer, &prompt, x, row_y, width, focused)
                },
                Widget::Radio(radio) => radio.render_row(
                    &mut buffer,
                    &prompt,
                    x,
                    row_y,
                    width,
                    focused,
                    self.radio_active,
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
            // Keep the focused button visible when the row is wider than the pane.
            let mut first = self.first_button;
            // Mouse-down changes focus; keep the row under the pointer until release.
            if !matches!(self.pressed, Some(PressTarget::Button(_))) {
                first = 0;
                if let Some(Focus::Button(focused)) = self.focus {
                    let mut used = self.buttons[focused].1.width();
                    first = focused;
                    while first > 0 && used + 1 + self.buttons[first - 1].1.width() <= width {
                        first -= 1;
                        used += 1 + self.buttons[first].1.width();
                    }
                }
            }
            self.first_button = first;
            for (_, button) in self.buttons.iter_mut().skip(first) {
                let room = (x + width).saturating_sub(button_x);
                if room == 0 {
                    break;
                }
                buffer
                    .output
                    .push_str(&button.serialize(button_x, button_y, Some(room)));
                button_x += button.width().min(room);
                button_x += 1; // one column between buttons
            }
        }

        // 3. The dropdown last, so it lands on top of the rows it covers.
        for field in self.fields.iter_mut() {
            if let Some(dropdown) = field.dropdown_mut() {
                let max_rows = dropdown
                    .row_rect()
                    .map(|row| {
                        overlay_bottom
                            .map(|bottom| bottom.saturating_sub(row.y + 1))
                            .unwrap_or(usize::MAX)
                    })
                    .unwrap_or(0);
                dropdown.render_overlay(&mut buffer, max_rows);
            }
        }
        buffer
    }

    /// The first row of the form the viewport shows: the least scroll that keeps
    /// the focused row on screen.
    ///
    /// An open dropdown does not enter into it. Scrolling to make room for one
    /// would move every other field - the row the user just opened jumps to the
    /// top of the viewport and whatever sat above it falls off - and the list is
    /// an overlay precisely so that nothing has to move. A dropdown with no room
    /// under it is the caller's problem to solve with
    /// [`render_in_viewport_with_overlay`](Self::render_in_viewport_with_overlay).
    fn viewport_start(&self, rows: usize) -> usize {
        if rows == 0 {
            return 0;
        }
        let max_start = self.height().saturating_sub(rows);
        let Some(focused_row) = self.focus_index() else {
            return 0;
        };
        focused_row
            .saturating_add(1)
            .saturating_sub(rows)
            .min(max_start)
    }
}

#[cfg(test)]
#[path = "unit/form.rs"]
mod tests;
