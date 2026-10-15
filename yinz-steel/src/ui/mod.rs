//! Widget bindings retain only interpreter-owned state and never call the host dispatcher.
mod diagnostics;
mod pure;
pub mod widgets;

use anyhow::{anyhow, bail, Context, Result};
use pure::StyledText;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use steel::{
    rvals::{Custom, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};
use widgets::{build, Descriptor, Markdown, Widget};
use yinz_tile::ui_components::{forms::*, *};
use yinz_types::data::{KeyWithModifier, Mouse};

const MAX: usize = crate::MAX_OUTPUT_BYTES;
const MAX_VIEWPORT_CELLS: usize = 262_144;

pub(crate) fn register(engine: &mut Engine) {
    engine.register_fn(
        "yinz-format-layout-error",
        |value: SteelVal| -> steel::rvals::Result<String> {
            diagnostics::format(crate::from_scheme(&value).map_err(crate::scheme_error)?)
                .map_err(crate::scheme_error)
        },
    );
    engine.register_fn(
        "yinz-ui-create",
        |value: SteelVal| -> steel::rvals::Result<NativeUi> {
            create(value).map_err(crate::scheme_error)
        },
    );
    engine.register_fn(
        "yinz-ui-call",
        |native: &NativeUi, value: SteelVal| -> steel::rvals::Result<SteelVal> {
            let operation =
                serde_json::from_value(crate::from_scheme(&value).map_err(crate::scheme_error)?)
                    .map_err(crate::scheme_error)?;
            let output = call(native, operation).map_err(crate::scheme_error)?;
            crate::to_scheme(&output).map_err(crate::scheme_error)
        },
    );
    engine.register_fn(
        "yinz-ui-image-png",
        |id: SteelVal, value: SteelVal| -> steel::rvals::Result<String> {
            let id = serde_json::from_value(crate::from_scheme(&id).map_err(crate::scheme_error)?)
                .context("invalid image id")
                .map_err(crate::scheme_error)?;
            if let SteelVal::ByteVector(bytes) = &value {
                return bytes
                    .with_bytes(|bytes| pure::image_png(id, bytes))
                    .map_err(crate::scheme_error);
            }
            let png: Vec<u8> =
                serde_json::from_value(crate::from_scheme(&value).map_err(crate::scheme_error)?)
                    .context("invalid PNG bytes")
                    .map_err(crate::scheme_error)?;
            pure::image_png(id, &png).map_err(crate::scheme_error)
        },
    );
    engine.register_fn(
        "yinz-ui-pure",
        |value: SteelVal| -> steel::rvals::Result<SteelVal> {
            let output = pure::run(crate::from_scheme(&value).map_err(crate::scheme_error)?)
                .map_err(crate::scheme_error)?;
            crate::to_scheme(&output).map_err(crate::scheme_error)
        },
    );
}

#[derive(Clone)]
struct NativeUi(Arc<Mutex<State>>);
impl Custom for NativeUi {}

fn create(value: SteelVal) -> Result<NativeUi> {
    let request =
        serde_json::from_value(crate::from_scheme(&value)?).context("invalid UI arguments")?;
    let state = match request {
        Create::Form { fields, dialog } => {
            anyhow::ensure!(
                fields.iter().all(|f| f.max_visible_options <= MAX / 256),
                "dropdown height exceeds transport budget"
            );
            let form = build(fields, 0).map_err(|error| anyhow!(error))?;
            State::Widget(if let Some(options) = dialog {
                let mut dialog = Dialog::new(options.title.build()?, form);
                if let Some(message) = options.message {
                    dialog = dialog.message(message.build()?);
                }
                if let Some(help) = options.help {
                    dialog = dialog.help(help.build()?);
                }
                Widget::Dialog(dialog)
            } else {
                Widget::Form(form)
            })
        },
        Create::Markdown {
            source,
            compact,
            dim,
        } => State::Markdown(Markdown::new(&source, compact, dim)),
    };
    Ok(NativeUi(Arc::new(Mutex::new(state))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coordinates {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    #[serde(default)]
    scroll: usize,
}
impl Coordinates {
    fn check(&self) -> Result<()> {
        anyhow::ensure!(
            self.x <= MAX
                && self.y <= MAX
                && self.width <= MAX / 256
                && self.height <= MAX / 256
                && self.width.saturating_mul(self.height) <= MAX_VIEWPORT_CELLS,
            "UI viewport exceeds transport budget"
        );
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Create {
    Form {
        fields: Vec<Descriptor>,
        dialog: Option<DialogOptions>,
    },
    Markdown {
        source: String,
        #[serde(default)]
        compact: bool,
        #[serde(default)]
        dim: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DialogOptions {
    title: TextOption,
    message: Option<TextOption>,
    help: Option<TextOption>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum TextOption {
    Plain(String),
    Styled(StyledText),
}
impl TextOption {
    fn build(self) -> Result<Text> {
        match self {
            Self::Plain(text) => Ok(Text::new(text)),
            Self::Styled(text) => text.build(),
        }
    }
}
enum State {
    Widget(Widget),
    Markdown(Markdown),
}
#[derive(Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
enum Operation {
    Value(String),
    Values,
    SetValue((String, Value)),
    SetOptions((String, Vec<String>)),
    Focus(String),
    FocusedId,
    MoveFocus(bool),
    Key(KeyWithModifier),
    Mouse(Mouse),
    Paste(String),
    Render(Coordinates),
    Lines(usize),
    SetStatus(Option<TextOption>),
    SetHelp(Option<TextOption>),
    TextValue(String),
    FieldInfo(String),
    Height,
    OverlayRect,
    Input((String, InputOperation)),
}
#[derive(Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
enum InputOperation {
    CursorPosition,
    SetCursorPosition(usize),
    ReplaceText((String, usize)),
    History,
    SetHistory(Vec<String>),
    DrainText,
    Key(KeyWithModifier),
    InsertText(String),
    Undo,
    Redo,
}
pub(super) fn action(event: FormEvent) -> Value {
    let render = event.should_render();
    let (kind, id) = match event {
        FormEvent::FieldChanged(id) => ("fieldChanged", Some(id)),
        FormEvent::Complete(id) => ("complete", Some(id)),
        FormEvent::ButtonPressed(id) => ("buttonPressed", Some(id)),
        FormEvent::FocusMoved => ("focusMoved", None),
        FormEvent::Cancel => ("cancel", None),
        FormEvent::Ignored => ("ignored", None),
    };
    let mut result = json!({"kind":kind, "shouldRender":render});
    if let Some(id) = id {
        result["id"] = json!(id);
    }
    result
}
fn field_value(field: &Field) -> Value {
    if let Some(c) = field.checkbox() {
        json!(c.is_checked())
    } else if let Some(r) = field.radio() {
        json!(r.selected_index())
    } else if let Some(s) = field.select() {
        json!(s.selected_index())
    } else {
        json!(field.text())
    }
}
fn call(native: &NativeUi, op: Operation) -> Result<Value> {
    let mut state = native
        .0
        .lock()
        .map_err(|_| anyhow!("UI state lock poisoned"))?;
    if let Operation::Lines(width) = op {
        anyhow::ensure!(width <= MAX / 256, "text width exceeds transport budget");
        let State::Markdown(md) = &mut *state else {
            bail!("lines requires Markdown");
        };
        let lines: Vec<_> = md
            .styled_lines(width)
            .map(|line| StyledText {
                text: line.content().to_owned(),
                levels: line.indices().to_vec(),
                selected: false,
                opaque: false,
                disabled: false,
            })
            .collect();
        let result = json!(lines);
        anyhow::ensure!(
            result.to_string().len() <= MAX,
            "UI output exceeds transport budget"
        );
        return Ok(result);
    }
    if let Operation::Render(c) = op {
        c.check()?;
        let result = match &mut *state {
            State::Markdown(md) => {
                let (text, rows) = md.render(c.x, c.y, c.width, c.height, c.scroll);
                json!({"text":text,"totalRows":rows})
            },
            State::Widget(w) => {
                let buffer = match w {
                    Widget::Form(f) => f.render_buffer_in_viewport(c.x, c.y, c.width, c.height),
                    Widget::Dialog(d) => d.render_buffer(Rect::new(c.x, c.y, c.width, c.height)),
                };
                json!({"text":buffer.output,"updateCursor":buffer.cursor.is_some(),"cursor":buffer.cursor.flatten()})
            },
        };
        anyhow::ensure!(
            result.to_string().len() <= MAX,
            "UI output exceeds transport budget"
        );
        return Ok(result);
    }
    let State::Widget(widget) = &mut *state else {
        bail!("Markdown supports only render and lines");
    };
    if let Operation::SetStatus(status) = op {
        let Widget::Dialog(dialog) = widget else {
            bail!("setStatus requires a dialog");
        };
        dialog.set_status(status.map(TextOption::build).transpose()?);
        return Ok(Value::Null);
    }
    if let Operation::SetHelp(help) = op {
        let Widget::Dialog(dialog) = widget else {
            bail!("setHelp requires a dialog");
        };
        dialog.set_help(help.map(TextOption::build).transpose()?);
        return Ok(Value::Null);
    }
    if let Operation::Height = op {
        return Ok(json!(match widget {
            Widget::Form(form) => form.height(),
            Widget::Dialog(dialog) => dialog.height(),
        }));
    }
    let form = widget.form_mut();
    Ok(match op {
        Operation::TextValue(id) => json!(form.text(&id)),
        Operation::FieldInfo(id) => {
            let Some(field) = form.field_by_id(&id) else {
                return Ok(Value::Null);
            };
            let (kind, open) = if let Some(select) = field.select() {
                ("select", select.is_open())
            } else if let Some(combo) = field.combo() {
                ("combo", combo.is_open())
            } else if field.checkbox().is_some() {
                ("checkbox", false)
            } else if field.radio().is_some() {
                ("radio_group", false)
            } else {
                ("text_input", false)
            };
            json!({"kind":kind,"label":field.label().content(),"text":field.text(),"value":field_value(field),
                "open":open,"editing":form.is_editing(),"radio_active":form.is_radio_active()})
        },
        Operation::OverlayRect => form.overlay_rect().map_or(
            Value::Null,
            |rect| json!({"x":rect.x,"y":rect.y,"rows":rect.rows,"columns":rect.columns}),
        ),
        Operation::Input((id, op)) => {
            let input = form
                .field_by_id_mut(&id)
                .and_then(Field::input_mut)
                .context("editor operation requires a text input")?;
            match op {
                InputOperation::CursorPosition => json!(input.cursor_position()),
                InputOperation::SetCursorPosition(cursor) => {
                    input.set_cursor_position(cursor);
                    Value::Null
                },
                InputOperation::ReplaceText((text, cursor)) => {
                    input.replace_text(text, cursor);
                    Value::Null
                },
                InputOperation::History => json!(input.history()),
                InputOperation::SetHistory(history) => {
                    input.set_history(history);
                    Value::Null
                },
                InputOperation::DrainText => json!(input.drain_text()),
                InputOperation::Key(key) => json!(match input.handle_key(key) {
                    InputAction::Submit => "submit",
                    InputAction::Cancel => "cancel",
                    InputAction::Continue => "continue",
                    InputAction::Complete => "complete",
                    InputAction::NoAction => "ignored",
                }),
                InputOperation::InsertText(text) => {
                    input.insert_text(&text);
                    Value::Null
                },
                InputOperation::Undo => json!(input.undo()),
                InputOperation::Redo => json!(input.redo()),
            }
        },
        Operation::Value(id) => field_value(form.field_by_id(&id).context("unknown field ID")?),
        Operation::Values => Value::Object(
            form.fields()
                .iter()
                .map(|f| (f.id().to_owned(), field_value(f)))
                .collect(),
        ),
        Operation::FocusedId => json!(form.focused_id()),
        Operation::Focus(id) => {
            anyhow::ensure!(form.focus_on(&id), "unknown widget ID");
            Value::Null
        },
        Operation::MoveFocus(forward) => action(form.move_focus(forward)),
        Operation::Key(key) => action(form.handle_key(key)),
        Operation::Mouse(mouse) => {
            if let Some((row, col)) = mouse.position() {
                anyhow::ensure!(row <= MAX && col <= MAX, "mouse coordinates out of bounds");
            } else {
                let count = match mouse {
                    Mouse::ScrollUp(n)
                    | Mouse::ScrollDown(n)
                    | Mouse::ScrollLeft(n)
                    | Mouse::ScrollRight(n) => n,
                    _ => 0,
                };
                anyhow::ensure!(count <= MAX / 256, "mouse scroll out of bounds");
            }
            action(form.handle_mouse(mouse))
        },
        Operation::Paste(text) => action(
            form.insert_pasted_text(&text)
                .map(FormEvent::FieldChanged)
                .unwrap_or(FormEvent::Ignored),
        ),
        Operation::SetValue((id, value)) => {
            let f = form.field_by_id_mut(&id).context("unknown field ID")?;
            if let Some(c) = f.checkbox_mut() {
                c.set_checked(
                    serde_json::from_value(value).context("checkbox value must be boolean")?,
                );
            } else if let Some(t) = f.input_mut() {
                t.set_text(serde_json::from_value(value).context("input value must be a string")?);
            } else if let Some(c) = f.combo_mut() {
                c.set_text(
                    serde_json::from_value::<String>(value)
                        .context("combo value must be a string")?,
                );
            } else if value.is_null() {
                if let Some(s) = f.select_mut() {
                    s.clear_selection();
                } else if let Some(r) = f.radio_mut() {
                    r.clear_selection();
                } else {
                    bail!("field has no selection");
                }
            } else {
                let index: usize = serde_json::from_value(value)
                    .context("value must be a zero-based option index")?;
                if let Some(s) = f.select_mut() {
                    anyhow::ensure!(index < s.options().len(), "option index out of range");
                    s.select_index(index);
                } else if let Some(r) = f.radio_mut() {
                    anyhow::ensure!(index < r.options().len(), "option index out of range");
                    r.select_index(index);
                }
            }
            Value::Null
        },
        Operation::SetOptions((id, options)) => {
            let f = form.field_by_id_mut(&id).context("unknown field ID")?;
            if let Some(s) = f.select_mut() {
                s.set_options(options);
            } else if let Some(c) = f.combo_mut() {
                c.set_suggestions(options);
            } else if let Some(r) = f.radio_mut() {
                let selected = r.selected_option().map(str::to_owned);
                *r = RadioGroup::new(options);
                if let Some(selected) = selected {
                    r.select_option(&selected);
                }
            } else {
                bail!("setOptions requires select, combo or radioGroup");
            }
            Value::Null
        },
        Operation::Render(_)
        | Operation::Lines(_)
        | Operation::SetStatus(_)
        | Operation::SetHelp(_)
        | Operation::Height => unreachable!(),
    })
}

#[cfg(test)]
#[path = "../unit/ui.rs"]
mod tests;
