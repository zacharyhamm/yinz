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
//! should pick different traversal keys. Under
//! [`Form::enter_never_submits`] the same ordering is what lets `Tab` leave a
//! text field's input mode: it never reaches the field at all.

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
    /// Focus moved between widgets, or the focused widget's interaction state
    /// changed - a dropdown opened or closed, a text field entered or left
    /// input mode. Nothing a caller reads has a new value.
    FocusMoved,
    /// The form was submitted — Enter in a field. Never emitted by a form built
    /// with [`Form::enter_never_submits`], where a button is the only way out.
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
    enter_never_submits: bool,
    editing: bool,
    pressed: Option<PressTarget>,
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
    /// A select's own row: released on it, a collapsed list opens and an open
    /// one closes again.
    SelectRow(usize),
    /// Field index and the option in the open dropdown.
    SelectOption(usize, usize),
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
            enter_never_submits: false,
            editing: false,
            pressed: None,
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

    /// Enter never submits: it acts on the focused widget instead.
    ///
    /// A closed [`Select`] opens on Enter (and the open one commits on the
    /// next), a [`Checkbox`] toggles, a [`RadioGroup`] advances, and a
    /// [`TextInput`] enters and leaves an input mode - so [`FormEvent::Submit`]
    /// is never emitted and a button is the only way to submit.
    ///
    /// Note that a text field is not typed into until it is in input mode, so a
    /// form whose *first* widget is one opens with no cursor anywhere until the
    /// user presses Enter or starts typing.
    pub fn enter_never_submits(mut self) -> Self {
        self.enter_never_submits = true;
        self
    }

    /// Whether the focused text field is in input mode.
    ///
    /// Always false unless the form was built with
    /// [`enter_never_submits`](Self::enter_never_submits): every other form
    /// types into its focused field directly.
    pub fn is_editing(&self) -> bool {
        self.editing
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
        // Unguarded, unlike the dropdown above: `focus_on` onto the field that
        // already has focus is a request to start over on it, not a no-op.
        self.editing = false;
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
        if self.enter_never_submits {
            return self.handle_key_strict(key);
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

    /// Key handling for a form built with
    /// [`enter_never_submits`](Self::enter_never_submits): Enter acts on the
    /// focused widget, and only a button submits.
    fn handle_key_strict(&mut self, key: KeyWithModifier) -> FormEvent {
        match self.focus {
            Some(Focus::Field(i)) => {
                // Text is split out because it is the one widget whose Enter
                // changes form-level state, which cannot be done while a field
                // is borrowed out of `self.fields`.
                if self.fields.get(i).map(|field| field.input().is_some()) == Some(true) {
                    return self.handle_key_text(i, key);
                }
                let Some(field) = self.fields.get_mut(i) else {
                    return FormEvent::Ignored;
                };
                let id = field.id.clone();
                match &mut field.widget {
                    // Handled above; `input()` said this is not a text field.
                    Widget::Text(_) => FormEvent::Ignored,
                    Widget::Select(select) => match select.handle_key(key.clone()) {
                        SelectAction::Continue | SelectAction::Committed => {
                            FormEvent::FieldChanged(id)
                        },
                        SelectAction::Cancelled => FormEvent::FocusMoved,
                        // A closed select: Enter opens the list, which the next
                        // Enter commits from. Nothing here submits.
                        SelectAction::NoAction => match key.bare_key {
                            BareKey::Enter if key.has_no_modifiers() => {
                                select.open();
                                FormEvent::FocusMoved
                            },
                            BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                            _ => FormEvent::Ignored,
                        },
                    },
                    Widget::Checkbox(checkbox) => match key.bare_key {
                        BareKey::Char(' ') | BareKey::Enter if key.has_no_modifiers() => {
                            checkbox.toggle();
                            FormEvent::FieldChanged(id)
                        },
                        BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                        _ => FormEvent::Ignored,
                    },
                    Widget::Radio(radio) => {
                        if radio.handle_key(key.clone()) {
                            return FormEvent::FieldChanged(id);
                        }
                        match key.bare_key {
                            // A radio cannot be unset, so the nearest thing to
                            // toggling one is moving to the next option.
                            BareKey::Enter if key.has_no_modifiers() => {
                                if radio.advance(true) {
                                    FormEvent::FieldChanged(id)
                                } else {
                                    FormEvent::Ignored
                                }
                            },
                            BareKey::Esc if key.has_no_modifiers() => FormEvent::Cancel,
                            _ => FormEvent::Ignored,
                        }
                    },
                }
            },
            // Buttons are unchanged: this is the one place Enter still acts on
            // the form as a whole.
            Some(Focus::Button(_)) | None => self.handle_key_unfocused_or_button(key),
        }
    }

    /// A focused text field under
    /// [`enter_never_submits`](Self::enter_never_submits), where Enter is a
    /// mode switch rather than anything the form as a whole hears about.
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
        let Some(input) = field.input_mut() else {
            return FormEvent::Ignored;
        };
        match input.handle_key(key) {
            InputAction::Continue => FormEvent::FieldChanged(id),
            InputAction::Cancel => FormEvent::Cancel,
            InputAction::Complete => FormEvent::Complete(id),
            // Enter was dealt with above, whatever its modifiers.
            InputAction::Submit => FormEvent::FocusMoved,
            InputAction::NoAction => FormEvent::Ignored,
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
        field.input_mut()?.insert_text(text);
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
        if let Some(i) = self.open_select_index() {
            let select = self.fields[i]
                .select_mut()
                .expect("open selects are select fields");
            if let Some(option) = select.option_at(x, y) {
                self.pressed = Some(PressTarget::SelectOption(i, option));
                self.set_focus(Focus::Field(i));
                return FormEvent::FocusMoved;
            }
            if select.row_contains(x, y) {
                // Pressing the row that opened the list closes it again.
                self.pressed = Some(PressTarget::SelectRow(i));
                self.set_focus(Focus::Field(i));
                return FormEvent::FocusMoved;
            }
            // Anything else dismisses the list at once - a list left open under
            // a pointer that has already moved on is a list in the way - and
            // then the press lands where it was aimed.
            select.close();
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
                Widget::Select(_) => self.pressed = Some(PressTarget::SelectRow(i)),
                Widget::Checkbox(_) => self.pressed = Some(PressTarget::Checkbox(i)),
                Widget::Radio(radio) => {
                    if let Some(option) = radio.option_at(x, y) {
                        self.pressed = Some(PressTarget::Radio(i, option));
                    }
                },
            }
            self.set_focus(Focus::Field(i));
            // After `set_focus`, which clears input mode. Placing the caret
            // where the press landed and then hiding it would be pointless: a
            // click into a field is intent enough to start typing in it.
            if self.enter_never_submits && self.fields[i].input().is_some() {
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
            PressTarget::SelectRow(i) => {
                let on_row = self.field_row_contains(i, x, y);
                let Some(select) = self.fields.get_mut(i).and_then(Field::select_mut) else {
                    return FormEvent::Ignored;
                };
                if select.is_open() {
                    if !select.row_contains(x, y) {
                        return FormEvent::Ignored;
                    }
                    select.close();
                } else {
                    if !on_row {
                        return FormEvent::Ignored;
                    }
                    select.open();
                }
                FormEvent::FocusMoved
            },
            PressTarget::SelectOption(i, option) => {
                let field = &mut self.fields[i];
                let id = field.id.clone();
                let Some(select) = field.select_mut() else {
                    return FormEvent::Ignored;
                };
                if select.option_at(x, y) != Some(option) {
                    return FormEvent::Ignored;
                }
                select.commit_option(option);
                FormEvent::FieldChanged(id)
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
                Widget::Checkbox(checkbox) => checkbox.clear_rendered(),
                _ => {},
            }
        }
        for (_, button) in &mut self.buttons {
            button.clear_rendered();
        }

        // 1. Work out where the cursor goes, printing nothing yet.
        // Read out here: `self` is borrowed for the field below.
        let typing = !self.enter_never_submits || self.editing;
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
                        Widget::Checkbox(_) | Widget::Radio(_) => None,
                    };
                    column.map(|column| (x + column, (y + i - start).saturating_sub(1)))
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
                    print_text_with_coordinates(text, x, row_y, Some(width), None);
                },
                Widget::Select(select) => select.render_row(&prompt, x, row_y, width, focused),
                Widget::Checkbox(checkbox) => {
                    checkbox.render_row(&prompt, x, row_y, width, focused)
                },
                Widget::Radio(radio) => radio.render_row(&prompt, x, row_y, width, focused),
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

    /// A form of every widget type, built with `enter_never_submits`.
    fn a_strict_form() -> Form {
        Form::new()
            .field("name", "Name", TextInput::empty())
            .select("layout", "Layout", Select::new(["default", "compact"]))
            .checkbox("frames", "Frames", Checkbox::new(false))
            .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
            .button("save", Button::new("Save"))
            .enter_never_submits()
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
        use crate::shim::take_last_cursor_position;

        let mut form = a_strict_form();
        form.render(2, 5, 40);
        assert_eq!(take_last_cursor_position(), Some(None));

        form.handle_key(key(BareKey::Enter));
        form.render(2, 5, 40);
        assert_eq!(take_last_cursor_position(), Some(Some((8, 4))));
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
    fn enter_advances_a_radio_and_wraps() {
        let mut form = a_strict_form();
        form.focus_on("mode");
        assert_eq!(
            form.handle_key(key(BareKey::Enter)),
            FormEvent::FieldChanged("mode".to_string())
        );
        assert_eq!(form.text("mode"), "Two");
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.text("mode"), "One");
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
        use crate::shim::take_last_cursor_position;

        let mut form = Form::new()
            .select("kind", "Kind", Select::new(["One", "Two"]))
            .field("name", "Name", TextInput::empty())
            .field("path", "Path", TextInput::empty());

        form.handle_key(key(BareKey::Tab));
        form.render(2, 5, 40);
        let name_row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
        assert_eq!(take_last_cursor_position(), Some(Some((8, name_row - 1))));

        form.handle_key(key(BareKey::Tab));
        form.render_in_viewport(2, 5, 40, 1);
        let path_row = form.field_by_id("path").unwrap().last_rendered.unwrap().y;
        assert_eq!(take_last_cursor_position(), Some(Some((8, path_row - 1))));
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
            click(&mut form, 1, 0),
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
    fn enter_still_submits_from_choice_fields() {
        let mut form = a_form_with_choices();
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Submit);
        form.focus_on("mode");
        assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Submit);
    }
}
