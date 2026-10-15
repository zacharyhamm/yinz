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
            if let Some(dropdown) = field.dropdown_mut() {
                if dropdown.is_open() {
                    dropdown.close();
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
            if self.editing {
                if let Some(combo) = self.fields[i].combo_mut() {
                    if !combo.suggestions().is_empty() {
                        combo.open();
                    }
                }
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
mod tests {
    use super::*;

    #[test]
    fn inactive_arrows_traverse_every_control_and_skip_disabled_buttons() {
        let ids = ["name", "layout", "frames", "mode", "model", "save"];
        for (arrow, forward) in [
            (BareKey::Down, true),
            (BareKey::Right, true),
            (BareKey::Up, false),
            (BareKey::Left, false),
        ] {
            for (index, id) in ids.iter().enumerate() {
                let mut form = a_strict_form()
                    .combo("model", "Model", Combo::new(["one", "two"]))
                    .button("disabled", Button::new("Disabled").disabled(true));
                form.focus_on(id);
                assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
                let next = if forward {
                    (index + 1) % ids.len()
                } else {
                    (index + ids.len() - 1) % ids.len()
                };
                assert_eq!(form.focused_id(), Some(ids[next]), "{id}, {arrow:?}");
                assert_eq!(form.text("mode"), "One");
                assert!(!form.checked("frames"));
            }
        }
    }

    #[test]
    fn active_controls_keep_all_arrows_even_with_custom_traversal() {
        for id in ["name", "layout", "mode", "model"] {
            for arrow in [BareKey::Up, BareKey::Down, BareKey::Left, BareKey::Right] {
                let mut form = a_strict_form()
                    .combo("model", "Model", Combo::empty())
                    .with_traversal_keys(key(arrow), key(arrow).with_alt_modifier());
                form.focus_on(id);
                form.handle_key(key(BareKey::Enter));
                assert_ne!(
                    form.handle_key(key(arrow)),
                    FormEvent::Ignored,
                    "{id}, {arrow:?}"
                );
                assert_eq!(form.focused_id(), Some(id));
                assert_ne!(
                    form.handle_key(key(arrow).with_alt_modifier()),
                    FormEvent::Ignored
                );
                assert_eq!(form.focused_id(), Some(id));
            }
        }
    }

    #[test]
    fn traversal_ends_interaction_and_discards_dropdown_highlights() {
        for backward in [false, true] {
            for id in ["name", "layout", "mode", "model"] {
                let mut form = a_strict_form().combo("model", "Model", Combo::new(["one", "two"]));
                form.focus_on(id);
                form.handle_key(key(BareKey::Enter));
                if id == "name" {
                    form.insert_pasted_text("keep me");
                }
                form.handle_key(key(BareKey::Down));
                let value = form.text(id).to_owned();
                let tab = if backward {
                    key(BareKey::Tab).with_shift_modifier()
                } else {
                    key(BareKey::Tab)
                };
                assert_eq!(form.handle_key(tab), FormEvent::FocusMoved);
                assert!(!form.is_editing());
                assert!(!form.is_radio_active());
                assert!(form.open_dropdown_index().is_none());
                assert_eq!(form.text(id), value);
            }
        }
    }

    #[test]
    fn ctrl_c_cancels_every_control_in_every_state() {
        for active in [false, true] {
            for id in ["name", "layout", "frames", "mode", "model", "save"] {
                let mut form = a_strict_form().combo("model", "Model", Combo::new(["one"]));
                form.focus_on(id);
                if active {
                    form.handle_key(key(BareKey::Enter));
                }
                assert_eq!(
                    form.handle_key(key(BareKey::Char('c')).with_ctrl_modifier()),
                    FormEvent::Cancel
                );
            }
        }
    }

    #[test]
    fn dropdown_highlighting_and_unchanged_commits_only_report_interaction() {
        let mut form = a_strict_form();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
        assert_eq!(form.text("layout"), "default");
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
        assert_eq!(form.text("layout"), "default");
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        form.handle_key(key(BareKey::Enter));
        form.render(0, 0, 40);
        assert_eq!(click(&mut form, 2, 0), FormEvent::FocusMoved);
        assert_eq!(form.text("layout"), "default");
    }

    #[test]
    fn empty_and_single_control_forms_keep_navigation_bounded() {
        for arrow in [BareKey::Up, BareKey::Down, BareKey::Left, BareKey::Right] {
            assert_eq!(Form::new().handle_key(key(arrow)), FormEvent::Ignored);
            let mut form = Form::new().select("only", "Only", Select::new(["one"]));
            assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
            assert_eq!(form.focused_id(), Some("only"));
            form.handle_key(key(BareKey::Enter));
            assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
            form.handle_key(key(BareKey::Tab));
            assert!(!form.select_by_id("only").unwrap().is_open());
        }
        let mut form = Form::new().radio("only", "Only", RadioGroup::new(["one"]));
        form.handle_key(key(BareKey::Char(' ')));
        assert!(form.is_radio_active());
        assert_eq!(form.handle_key(key(BareKey::Right)), FormEvent::FocusMoved);
        form.handle_key(key(BareKey::Esc));
        assert!(!form.is_radio_active());
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn active_text_retains_modified_editing_and_only_value_changes_report_changed() {
        let mut form = a_form();
        form.insert_pasted_text("hello world");
        assert_eq!(
            form.handle_key(key(BareKey::Left).with_ctrl_modifier()),
            FormEvent::FocusMoved
        );
        assert_eq!(form.fields[0].input().unwrap().get_cursor_position(), 6);
        assert_eq!(
            form.handle_key(key(BareKey::Char('u')).with_ctrl_modifier()),
            FormEvent::FieldChanged("name".into())
        );
        assert_eq!(form.text("name"), "world");
        assert_eq!(form.focused_id(), Some("name"));
    }

    #[test]
    fn history_belongs_to_active_text_and_restores_the_draft() {
        let mut form = a_form();
        form.fields[0]
            .input_mut()
            .unwrap()
            .set_history(vec!["old".into()]);
        form.insert_pasted_text("draft");
        assert_eq!(
            form.handle_key(key(BareKey::Up)),
            FormEvent::FieldChanged("name".into())
        );
        assert_eq!(form.text("name"), "old");
        assert_eq!(
            form.handle_key(key(BareKey::Down)),
            FormEvent::FieldChanged("name".into())
        );
        assert_eq!(form.text("name"), "draft");
        form.handle_key(key(BareKey::Enter));
        form.handle_key(key(BareKey::Down));
        assert_eq!(form.focused_id(), Some("path"));
    }

    #[test]
    fn radio_focus_styles_distinguish_activation_and_small_viewports_keep_focus_visible() {
        let mut form = a_strict_form();
        form.focus_on("mode");
        let inactive = form.render_buffer_in_viewport(0, 0, 40, 1);
        assert_eq!(inactive.cursor, Some(None));
        assert_eq!(form.fields[3].last_rendered, Some(Rect::new(0, 0, 40, 1)));
        form.handle_key(key(BareKey::Enter));
        let active = form.render_buffer_in_viewport(0, 0, 40, 1);
        assert_ne!(inactive.output, active.output);
        assert_eq!(active.cursor, Some(None));
        form.handle_key(key(BareKey::Char(' ')));
        assert_eq!(
            form.render_buffer_in_viewport(0, 0, 40, 1).output,
            inactive.output
        );
        form.handle_key(key(BareKey::Enter));
        form.handle_key(key(BareKey::Right));
        form.render_buffer_in_viewport(0, 0, 12, 1);
        assert_eq!(form.fields[3].radio().unwrap().option_at(6, 0), Some(1));
        form.focus_on("name");
        form.handle_key(key(BareKey::Enter));
        assert!(form
            .render_buffer_in_viewport(0, 0, 8, 1)
            .cursor
            .flatten()
            .is_some());
        assert_eq!(
            form.render_buffer_in_viewport(0, 0, 0, 0).cursor,
            Some(None)
        );
    }

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

    /// A form of every widget type, with explicit activation.
    fn a_strict_form() -> Form {
        Form::new()
            .field("name", "Name", TextInput::empty())
            .select("layout", "Layout", Select::new(["default", "compact"]))
            .checkbox("frames", "Frames", Checkbox::new(false))
            .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
            .button("save", Button::new("Save"))
    }

    #[test]
    fn enter_toggles_input_mode_and_never_submits() {
        let mut form = a_strict_form();
        assert!(!form.is_editing());
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert!(form.is_editing());
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert!(!form.is_editing());
    }

    #[test]
    fn typing_auto_enters_input_mode() {
        let mut form = a_strict_form();
        assert_eq!(
            form.handle_key(key(BareKey::Char('a'))),
            FormEvent::FieldChanged("name".to_string())
        );
        assert!(form.is_editing());
        assert_eq!(form.text("name"), "a");
    }

    #[test]
    fn esc_leaves_input_mode_before_cancelling() {
        let mut form = a_strict_form();
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
        assert!(!form.is_editing());
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn tab_leaves_input_mode_and_moves_focus() {
        let mut form = a_strict_form();
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
        assert!(!form.is_editing());
        assert_eq!(form.focused_id(), Some("layout"));
    }

    #[test]
    fn moving_focus_leaves_input_mode() {
        let mut form = a_strict_form();
        form.handle_key(key(BareKey::Enter));
        // Even onto the field that already has it: that is a request to start
        // over on the field, not a no-op.
        form.focus_on("name");
        assert!(!form.is_editing());

        form.handle_key(key(BareKey::Enter));
        form.move_focus(true);
        assert!(!form.is_editing());
    }

    #[test]
    fn the_cursor_is_only_shown_while_editing() {
        let mut form = a_strict_form();
        let rendered = form.render(2, 5, 40);
        assert_eq!(rendered.cursor, Some(None));

        form.handle_key(key(BareKey::Enter));
        let rendered = form.render(2, 5, 40);
        // The row the focused field was drawn on, which is the row the form was
        // asked to start at.
        assert_eq!(rendered.cursor, Some(Some((8, 5))));
    }

    #[test]
    fn enter_opens_a_closed_dropdown_then_commits_it() {
        let mut form = a_strict_form();
        form.focus_on("layout");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert!(form.select_by_id("layout").unwrap().is_open());

        form.handle_key(key(BareKey::Down));
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("layout".to_string())
        );
        assert_eq!(form.text("layout"), "compact");
        assert!(!form.select_by_id("layout").unwrap().is_open());
    }

    #[test]
    fn enter_toggles_a_checkbox() {
        let mut form = a_strict_form();
        form.focus_on("frames");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("frames".to_string())
        );
        assert!(form.checked("frames"));
        form.handle_key(key(BareKey::Enter));
        assert!(!form.checked("frames"));
    }

    #[test]
    fn enter_activates_a_radio_without_changing_it() {
        let mut form = a_strict_form();
        form.focus_on("mode");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert_eq!(form.text("mode"), "One");
        assert_eq!(
            form.handle_key(key(BareKey::Right)),
            FormEvent::FieldChanged("mode".into())
        );
        assert_eq!(form.text("mode"), "Two");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert_eq!(form.text("mode"), "Two");
    }

    #[test]
    fn enter_on_the_button_is_the_only_submit() {
        let mut form = a_strict_form();
        form.focus_on("save");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::ButtonPressed("save".to_string())
        );
    }

    /// `TextInput`'s bare-key arm ignores modifiers, so an unguarded Alt-Enter
    /// comes back as the `Submit` this form promises never to emit.
    #[test]
    fn a_modified_enter_is_left_for_the_plugin() {
        let alt_enter = KeyWithModifier::new(BareKey::Enter).with_alt_modifier();
        for id in ["name", "layout", "frames", "mode"] {
            let mut form = a_strict_form();
            form.focus_on(id);
            assert_eq!(
                form.handle_key(alt_enter.clone()),
                FormEvent::Ignored,
                "{id}"
            );
        }

        let mut editing = a_strict_form();
        editing.handle_key(key(BareKey::Enter));
        assert_eq!(editing.handle_key(alt_enter), FormEvent::Ignored);
        assert!(
            editing.is_editing(),
            "a key it did not take changes nothing"
        );
    }

    #[test]
    fn clicking_a_text_field_starts_editing() {
        let mut form = a_strict_form();
        form.focus_on("layout");
        form.render(0, 0, 40);
        let row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(row as isize, 8)),
            FormEvent::FocusMoved
        );
        assert_eq!(form.focused_id(), Some("name"));
        assert!(form.is_editing());
    }

    #[test]
    fn pasted_text_lands_in_the_focused_field_and_starts_editing() {
        let mut form = a_strict_form();
        assert_eq!(form.insert_pasted_text("branch"), Some("name".to_string()));
        assert_eq!(form.text("name"), "branch");
        assert!(form.is_editing());

        form.focus_on("frames");
        assert_eq!(form.insert_pasted_text("nope"), None);
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
        let mut form = Form::new()
            .select("kind", "Kind", Select::new(["One", "Two"]))
            .field("name", "Name", TextInput::empty())
            .field("path", "Path", TextInput::empty());

        form.handle_key(key(BareKey::Tab));
        form.handle_key(key(BareKey::Enter));
        let rendered = form.render(2, 5, 40);
        let name_row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
        assert_eq!(rendered.cursor, Some(Some((8, name_row))));

        form.handle_key(key(BareKey::Tab));
        form.handle_key(key(BareKey::Enter));
        let rendered = form.render_in_viewport(2, 5, 40, 1);
        let path_row = form.field_by_id("path").unwrap().last_rendered.unwrap().y;
        assert_eq!(rendered.cursor, Some(Some((8, path_row))));
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
    fn custom_traversal_keeps_tab_traversal() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .with_traversal_keys(
                KeyWithModifier::new(BareKey::Down),
                KeyWithModifier::new(BareKey::Up),
            );
        assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
    }

    #[test]
    fn unhandled_keys_come_back_as_ignored() {
        let mut form = a_form();
        let event = form.handle_key(KeyWithModifier::new(BareKey::F(5)));
        assert_eq!(event, FormEvent::Ignored);
        assert!(!event.should_render());
    }

    #[test]
    fn enter_edits_and_esc_leaves_editing_before_cancelling() {
        let mut form = a_form();
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
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
    fn a_narrow_form_keeps_each_focused_button_clickable() {
        let mut form = a_form();
        for id in ["save", "cancel"] {
            form.focus_on(id);
            form.render_in_viewport(0, 0, 5, 1);
            form.handle_mouse(Mouse::LeftClick(0, 1));
            form.render_in_viewport(0, 0, 5, 1);
            assert_eq!(
                form.handle_mouse(Mouse::Release(0, 1)),
                FormEvent::ButtonPressed(id.into())
            );
        }
    }

    #[test]
    fn a_partially_visible_button_stays_under_the_pointer_until_release() {
        for scrolled in [false, true] {
            for release_inside in [false, true] {
                let mut form = Form::new();
                if scrolled {
                    form = form.button("earlier", Button::new("Earlier"));
                }
                form = form
                    .button("restart", Button::new("Restart"))
                    .button("duplicate", Button::new("Duplicate"))
                    .button("fork", Button::new("Fork"));
                form.focus_on("duplicate");
                form.render_buffer(0, 0, 24);
                assert_eq!(form.first_button, usize::from(scrolled));
                let fork = form.buttons.last().unwrap().1.last_rendered().unwrap();
                assert!(fork.columns < form.buttons.last().unwrap().1.width());
                assert_eq!(
                    form.handle_mouse(Mouse::LeftClick(0, fork.x)),
                    FormEvent::FocusMoved
                );
                assert_eq!(form.focused_id(), Some("fork"));
                form.render_buffer(0, 0, 24);
                assert_eq!(form.buttons.last().unwrap().1.last_rendered(), Some(fork));
                let release_x = if release_inside { fork.x } else { 0 };
                assert_eq!(
                    form.handle_mouse(Mouse::Release(0, release_x)),
                    if release_inside {
                        FormEvent::ButtonPressed("fork".into())
                    } else {
                        FormEvent::Ignored
                    }
                );
                form.render_buffer(0, 0, 24);
                assert_eq!(
                    form.buttons
                        .last()
                        .unwrap()
                        .1
                        .last_rendered()
                        .unwrap()
                        .columns,
                    form.buttons.last().unwrap().1.width()
                );
                form.handle_key(key(BareKey::Left));
                form.render_buffer(0, 0, 24);
                assert_eq!(form.focused_id(), Some("duplicate"));
                assert_eq!(form.first_button, usize::from(scrolled));
            }
        }
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
        assert_eq!(event, FormEvent::FocusMoved);
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
            FormEvent::FocusMoved,
            "with the dropdown closed, Enter opens it again"
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
        // Name, then the select, then the button row.
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));

        // Only the button row is left underneath, so only one option fits.
        form.render_in_viewport(0, 0, 40, 3);
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 1)));

        // Nothing is left underneath: the list is not drawn rather than the
        // form being scrolled to make space for it.
        form.render_in_viewport(0, 0, 40, 2);
        assert_eq!(form.overlay_rect(), None);
    }

    #[test]
    fn a_bottom_row_dropdown_uses_the_rows_the_caller_lends_it() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .select("layout", "Layout", Select::new(["default", "compact"]));
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));

        // Two rows of form, and two more below it that the caller owns.
        form.render_in_viewport_with_overlay(0, 0, 40, 2, 4);

        assert_eq!(
            form.fields[0].last_rendered,
            Some(Rect::new(0, 0, 40, 1)),
            "opening the list is not a reason to scroll the field above it away"
        );
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 2)));
        assert_eq!(
            click(&mut form, 3, 0),
            FormEvent::FieldChanged("layout".to_string())
        );
        assert_eq!(form.text("layout"), "compact");
    }

    /// The bug this replaced: the form scrolled a row to make space for the
    /// list, taking the field above the select off the top of the viewport.
    #[test]
    fn opening_a_dropdown_does_not_scroll_a_form_that_already_fits() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .select("layout", "Layout", Select::new(["default", "compact"]))
            .field("path", "Path", TextInput::empty())
            .button("save", Button::new("Save"));
        form.focus_on("layout");
        let rows = form.height();

        assert_eq!(form.viewport_start(rows), 0);
        form.handle_key(key(BareKey::Char(' ')));
        assert!(form.select_by_id("layout").unwrap().is_open());
        assert_eq!(form.viewport_start(rows), 0);

        form.render_in_viewport(0, 0, 40, rows);
        assert_eq!(form.fields[0].last_rendered, Some(Rect::new(0, 0, 40, 1)));
        assert_eq!(form.fields[1].last_rendered, Some(Rect::new(0, 1, 40, 1)));
    }

    #[test]
    fn the_original_render_keeps_dropdowns_unbounded() {
        let mut form = a_form_with_a_select();
        form.focus_on("layout");
        form.handle_key(key(BareKey::Char(' ')));
        form.render(0, 0, 40);
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 3)));
    }

    /// A whole click - press and release on the same spot - the way the host
    /// sends one.
    fn click(form: &mut Form, line: isize, column: usize) -> FormEvent {
        form.handle_mouse(Mouse::LeftClick(line, column));
        form.handle_mouse(Mouse::Release(line, column))
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
    fn active_arrows_change_a_radio_and_text_reads_its_option() {
        let mut form = a_form_with_choices();
        form.focus_on("mode");
        form.handle_key(key(BareKey::Enter));
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
    fn active_radio_clicks_survive_a_render_between_press_and_release() {
        for render_between in [false, true] {
            for release_column in [14, 6, 20] {
                let mut form = Form::new().radio(
                    "mode",
                    "Mode",
                    RadioGroup::new(["One", "Two", "Three"]).with_selected(1),
                );
                form.handle_key(key(BareKey::Enter));
                form.render_buffer(0, 0, 20);
                assert_eq!(form.radio_by_id("mode").unwrap().option_at(14, 0), Some(2));
                assert_eq!(
                    form.handle_mouse(Mouse::LeftClick(0, 14)),
                    FormEvent::FocusMoved
                );
                assert_eq!(form.text("mode"), "Two");
                if render_between {
                    form.render_buffer(0, 0, 20);
                }
                let event = form.handle_mouse(Mouse::Release(0, release_column));
                if release_column == 14 {
                    assert_eq!(event, FormEvent::FieldChanged("mode".into()));
                    assert_eq!(form.text("mode"), "Three");
                } else {
                    assert_eq!(event, FormEvent::Ignored);
                    assert_eq!(form.text("mode"), "Two");
                }
                assert!(form.is_radio_active());
                assert!(form.focus_on("mode"));
                assert!(!form.is_radio_active());
            }
        }
    }

    #[test]
    fn clicking_an_inactive_radio_does_not_activate_it() {
        let mut form = Form::new()
            .radio("first", "First", RadioGroup::new(["One", "Two"]))
            .radio("mode", "Mode", RadioGroup::new(["One", "Two", "Three"]));
        form.handle_key(key(BareKey::Enter));
        form.render_buffer(0, 0, 20);
        form.handle_mouse(Mouse::LeftClick(1, 14));
        assert_eq!(form.focused_id(), Some("mode"));
        assert!(!form.is_radio_active());
        assert_eq!(form.text("mode"), "One");
        form.render_buffer(0, 0, 20);
        assert_eq!(
            form.handle_mouse(Mouse::Release(1, 14)),
            FormEvent::FieldChanged("mode".into())
        );
        assert_eq!(form.text("mode"), "Two");
        assert!(!form.is_radio_active());
    }

    #[test]
    fn clicking_choices_changes_only_when_the_value_changes() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        assert_eq!(
            click(&mut form, 0, 0),
            FormEvent::FieldChanged("frames".to_string())
        );
        assert!(form.checked("frames"));

        assert_eq!(
            click(&mut form, 1, 14),
            FormEvent::FieldChanged("mode".to_string())
        );
        assert_eq!(form.text("mode"), "Two");
        assert_eq!(
            click(&mut form, 1, 14),
            FormEvent::Ignored,
            "releasing on the option that is already selected changes nothing"
        );
    }

    /// A press is not a click. Everything below goes through `click`, which
    /// sends both halves the way the host does; these are the cases where the
    /// two halves disagree.
    #[test]
    fn a_press_alone_moves_focus_and_nothing_else() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        assert_eq!(
            form.handle_mouse(Mouse::LeftClick(0, 0)),
            FormEvent::FocusMoved
        );
        assert!(!form.checked("frames"), "the press alone toggled nothing");
        assert_eq!(form.focused_id(), Some("frames"));

        assert_eq!(
            form.handle_mouse(Mouse::Release(0, 0)),
            FormEvent::FieldChanged("frames".to_string())
        );
        assert!(form.checked("frames"));
    }

    #[test]
    fn releasing_off_the_widget_takes_the_press_back() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        // Pressed on the checkbox, released on the radio row below it.
        form.handle_mouse(Mouse::LeftClick(0, 0));
        assert_eq!(form.handle_mouse(Mouse::Release(1, 0)), FormEvent::Ignored);
        assert!(!form.checked("frames"));

        // And the release does not act on what it landed on either.
        assert_eq!(form.text("mode"), "One");
    }

    #[test]
    fn a_press_dragged_off_a_button_and_back_still_fires() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);
        let button_y = form.buttons[0].1.last_rendered().unwrap().y as isize;

        form.handle_mouse(Mouse::LeftClick(button_y, 1));
        // Off the button, then back onto it: only where the release lands counts.
        form.handle_mouse(Mouse::Hold(button_y, 30));
        form.handle_mouse(Mouse::Hold(button_y, 1));
        assert_eq!(
            form.handle_mouse(Mouse::Release(button_y, 1)),
            FormEvent::ButtonPressed("save".to_string())
        );

        form.handle_mouse(Mouse::LeftClick(button_y, 1));
        assert_eq!(
            form.handle_mouse(Mouse::Release(button_y, 30)),
            FormEvent::Ignored,
            "released past the end of the button"
        );
    }

    #[test]
    fn a_release_without_a_press_does_nothing() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);
        assert_eq!(form.handle_mouse(Mouse::Release(0, 0)), FormEvent::Ignored);
        assert!(!form.checked("frames"));
    }

    /// The pointer leaving the pane arrives as buttonless motion, which means
    /// the button came up somewhere this form never heard about.
    #[test]
    fn a_hover_between_press_and_release_cancels_the_press() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        form.handle_mouse(Mouse::LeftClick(0, 0));
        form.handle_mouse(Mouse::Hover(0, u16::MAX as usize));
        assert_eq!(form.handle_mouse(Mouse::Release(0, 0)), FormEvent::Ignored);
        assert!(!form.checked("frames"));
    }

    #[test]
    fn a_disabled_button_refuses_the_mouse() {
        let mut form = Form::new()
            .field("name", "Name", TextInput::empty())
            .button("save", Button::new("Save").disabled(true));
        form.render(0, 0, 40);
        let button_y = form.buttons[0].1.last_rendered().unwrap().y as isize;
        assert_eq!(click(&mut form, button_y, 1), FormEvent::Ignored);
    }

    #[test]
    fn hovering_highlights_the_choice_row_under_the_pointer() {
        let mut form = a_form_with_choices();
        form.render(0, 0, 40);

        assert_eq!(
            form.handle_mouse(Mouse::Hover(0, 0)),
            FormEvent::FocusMoved,
            "the checkbox row took the highlight"
        );
        assert_eq!(
            form.handle_mouse(Mouse::Hover(0, 0)),
            FormEvent::Ignored,
            "nothing changed the second time"
        );
        assert_eq!(
            form.handle_mouse(Mouse::Hover(1, 14)),
            FormEvent::FocusMoved
        );
        // The leave sentinel the host sends when the pointer goes elsewhere.
        assert_eq!(
            form.handle_mouse(Mouse::Hover(0, u16::MAX as usize)),
            FormEvent::FocusMoved
        );
        assert_eq!(
            form.handle_mouse(Mouse::Hover(0, u16::MAX as usize)),
            FormEvent::Ignored
        );
    }

    #[test]
    fn enter_activates_choice_fields() {
        let mut form = a_form_with_choices();
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("frames".into())
        );
        form.focus_on("mode");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    }

    fn a_form_with_a_combo() -> Form {
        Form::new()
            .field("name", "Name", TextInput::empty())
            .combo("model", "Model", Combo::new(["opus", "sonnet", "haiku"]))
            .button("save", Button::new("Save"))
    }

    #[test]
    fn a_combo_field_reads_back_as_its_text() {
        let mut form = a_form_with_a_combo();
        assert_eq!(form.text("model"), "");
        form.combo_by_id_mut("model").unwrap().set_text("gpt-5");
        assert_eq!(
            form.text("model"),
            "gpt-5",
            "a combo holds text, not an option index"
        );
        assert!(form.combo_by_id("model").is_some());
        assert!(form.field_by_id("model").unwrap().is_combo());
        assert!(form.combo_by_id("name").is_none());
    }

    #[test]
    fn typing_into_a_focused_combo_fills_it_in_and_offers_the_matches() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        assert_eq!(
            form.handle_key(key(BareKey::Char('o'))),
            FormEvent::FieldChanged("model".to_string())
        );
        assert_eq!(form.text("model"), "o");
        assert!(form.combo_by_id("model").unwrap().is_open());
    }

    /// The guarantee the widget exists for: a value nobody picked off the list
    /// is still the value.
    #[test]
    fn enter_on_a_combo_accepts_what_was_typed() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Char('o')));
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert_eq!(form.text("model"), "o");
    }

    #[test]
    fn arrowing_into_the_list_makes_enter_take_a_suggestion() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Char('o')));
        assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("model".to_string())
        );
        assert_eq!(form.text("model"), "opus");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FocusMoved,
            "with the list closed, Enter starts editing again"
        );
    }

    #[test]
    fn esc_closes_a_combo_list_before_it_cancels_the_form() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
        assert!(!form.combo_by_id("model").unwrap().is_open());
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
        assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn traversing_away_closes_an_open_combo_list() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Enter));
        assert!(form.combo_by_id("model").unwrap().is_open());
        form.handle_key(key(BareKey::Tab));
        assert_eq!(form.focused_id(), Some("save"));
        assert!(!form.combo_by_id("model").unwrap().is_open());
    }

    #[test]
    fn a_paste_into_a_combo_lands_in_it_and_offers_the_matches() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        assert_eq!(form.insert_pasted_text("son"), Some("model".to_string()));
        assert_eq!(form.text("model"), "son");
        assert!(form.combo_by_id("model").unwrap().is_open());
    }

    #[test]
    fn an_open_combo_list_is_an_overlay_like_a_dropdown() {
        let mut form = a_form_with_a_combo();
        let closed_height = form.height();
        form.focus_on("model");
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.height(), closed_height);
        assert_eq!(
            form.overlay_rect(),
            None,
            "a list that has never rendered covers nothing"
        );
        form.render(0, 0, 40);
        assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 3)));
    }

    #[test]
    fn clicking_a_suggestion_takes_it() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Enter));
        form.render(0, 0, 40);
        assert_eq!(
            click(&mut form, 3, 0),
            FormEvent::FieldChanged("model".to_string())
        );
        assert_eq!(form.text("model"), "sonnet");
        assert!(!form.combo_by_id("model").unwrap().is_open());
    }

    /// The marker is the only part of the row that is a control: everywhere else
    /// is text, and a click there is a click into text.
    #[test]
    fn the_marker_toggles_the_list_and_the_rest_of_the_row_takes_the_caret() {
        let mut form = a_form_with_a_combo();
        form.combo_by_id_mut("model").unwrap().set_text("sonnet");
        form.render(0, 0, 40);

        assert_eq!(click(&mut form, 1, 39), FormEvent::FocusMoved);
        assert!(form.combo_by_id("model").unwrap().is_open());
        form.render(0, 0, 40);
        assert_eq!(click(&mut form, 1, 39), FormEvent::FocusMoved);
        assert!(!form.combo_by_id("model").unwrap().is_open());

        assert_eq!(
            click(&mut form, 1, 9),
            FormEvent::Ignored,
            "a click into the text is not a click on anything to activate"
        );
        assert!(!form.combo_by_id("model").unwrap().is_open());
        assert_eq!(
            form.field_by_id("model")
                .and_then(Field::input)
                .map(TextInput::get_cursor_position),
            Some(2),
            "the caret goes where the click landed"
        );
    }

    #[test]
    fn a_combo_accepts_a_suggestion_and_ends_editing() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        assert_eq!(
            form.handle_key(key(BareKey::Char('o'))),
            FormEvent::FieldChanged("model".to_string())
        );
        assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("model".to_string())
        );
        assert_eq!(form.text("model"), "opus");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FocusMoved,
            "with the list closed, Enter is the input mode's again"
        );
    }

    #[test]
    fn enter_on_a_combo_list_with_nothing_highlighted_leaves_the_input_mode() {
        let mut form = a_form_with_a_combo();
        form.focus_on("model");
        form.handle_key(key(BareKey::Char('o')));
        assert!(form.combo_by_id("model").unwrap().is_open());
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
        assert_eq!(form.text("model"), "o");
        assert!(!form.editing, "the second Enter left the input mode");
    }
}
