//! Pure, quota-accounted adapters. Host commands remain in the SDK dispatcher.
use super::super::ui::{build, Descriptor, Markdown, Widget};
use super::{
    limits::{Limits, NativeCharge},
    transport,
};
use deno_core::{cppgc::GarbageCollected, op2, v8, OpState};
use deno_error::JsErrorBox;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use unicode_width::UnicodeWidthStr;
use zellij_runtime::errors::prelude::*;
use zellij_tile::{
    text,
    ui_components::{forms::*, *},
};
use zellij_types::data::{KeyWithModifier, Mouse};

const MAX: usize = zellij_ipc::MAX_IPC_FRAME_LEN;
fn error(e: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::generic(e.to_string())
}
fn input<'s, T: DeserializeOwned>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<(T, usize)> {
    let value = transport::from_v8(scope, value)?;
    let bytes = value.to_string().len();
    Ok((
        serde_json::from_value(value).context("invalid UI arguments")?,
        bytes,
    ))
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
                && self.width.saturating_mul(self.height) <= MAX / 256,
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
        compact: bool,
        #[serde(default)]
        dim: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DialogOptions {
    title: String,
    message: Option<String>,
    help: Option<String>,
}
enum State {
    Widget(Widget),
    Markdown(Markdown),
}
pub(super) struct NativeUi {
    state: RefCell<State>,
    charge: RefCell<NativeCharge>,
    cache_budget: Cell<usize>,
}
// SAFETY: all retained values are owned Rust data, with no V8 or CppGC references.
unsafe impl GarbageCollected for NativeUi {
    fn trace(&self, _: &mut v8::cppgc::Visitor) {}
    fn get_name(&self) -> &'static std::ffi::CStr {
        c"ZellijUi"
    }
}
#[op2]
#[cppgc]
pub(super) fn op_zellij_ui_create<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &OpState,
    value: v8::Local<'s, v8::Value>,
) -> std::result::Result<NativeUi, JsErrorBox> {
    (|| -> Result<NativeUi> {
        let (request, bytes) = input::<Create>(scope, value)?;
        let mut charge = NativeCharge::new(state.borrow::<Arc<Limits>>().clone());
        // Covers Rust object/vector overhead, parsed Markdown and its wrapping cache.
        charge.grow(
            bytes
                .checked_mul(256)
                .context("UI allocation overflow")?
                .max(4096),
        )?;
        let state = match request {
            Create::Form { fields, dialog } => {
                anyhow::ensure!(
                    fields.iter().all(|f| f.max_visible_options <= MAX / 256),
                    "dropdown height exceeds transport budget"
                );
                let form = build(fields, 0).map_err(|e| anyhow!(e))?;
                State::Widget(if let Some(options) = dialog {
                    let mut dialog = Dialog::new(Text::new(options.title), form);
                    if let Some(message) = options.message {
                        dialog = dialog.message(Text::new(message));
                    }
                    if let Some(help) = options.help {
                        dialog = dialog.help(Text::new(help));
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
        Ok(NativeUi {
            state: RefCell::new(state),
            charge: RefCell::new(charge),
            cache_budget: Cell::new(0),
        })
    })()
    .map_err(error)
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
    SetStatus(Option<String>),
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
#[op2]
#[serde]
pub(super) fn op_zellij_ui_call<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[cppgc] native: &NativeUi,
    value: v8::Local<'s, v8::Value>,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    input::<Operation>(scope, value)
        .and_then(|(op, bytes)| call(native, op, bytes))
        .map_err(error)
}

fn call(native: &NativeUi, op: Operation, bytes: usize) -> Result<Value> {
    if matches!(
        op,
        Operation::SetValue(_)
            | Operation::SetOptions(_)
            | Operation::Paste(_)
            | Operation::Key(_)
            | Operation::SetStatus(_)
    ) {
        native
            .charge
            .borrow_mut()
            .grow(bytes.checked_mul(256).context("UI allocation overflow")?)?;
    }
    let mut state = native.state.borrow_mut();
    if let Operation::Render(c) = op {
        c.check()?;
        let result = match &mut *state {
            State::Markdown(md) => {
                let budget = md.wrapping_budget(c.width).saturating_mul(2);
                if budget > native.cache_budget.get() {
                    native
                        .charge
                        .borrow_mut()
                        .grow(budget - native.cache_budget.get())?;
                    native.cache_budget.set(budget);
                }
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
        bail!("Markdown supports only render");
    };
    if let Operation::SetStatus(status) = op {
        let Widget::Dialog(dialog) = widget else {
            bail!("setStatus requires a dialog");
        };
        dialog.set_status(status.map(Text::new));
        return Ok(Value::Null);
    }
    let form = widget.form_mut();
    Ok(match op {
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
        Operation::Render(_) | Operation::SetStatus(_) => unreachable!(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyledText {
    text: String,
    selected: bool,
    opaque: bool,
    disabled: bool,
    levels: Vec<Vec<usize>>,
}
impl StyledText {
    fn build(self) -> Result<Text> {
        anyhow::ensure!(
            self.levels.len() <= 8,
            "style level must be between 0 and 7"
        );
        let len = self.text.chars().count();
        let mut text = Text::new(self.text);
        if self.selected {
            text = text.selected();
        }
        if self.opaque {
            text = text.opaque();
        }
        if self.disabled {
            text = text.disabled();
        }
        for (level, indices) in self.levels.into_iter().enumerate() {
            anyhow::ensure!(indices.iter().all(|i| *i < len), "style index out of range");
            text = text.color_indices(level, indices);
        }
        Ok(text)
    }
}
#[derive(Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
enum Pure {
    Width(String),
    Wrap((String, usize)),
    Sanitize(String),
    Truncate((String, usize, String)),
    Pad((String, usize)),
    PadLeft((String, usize)),
    Center((String, usize)),
    Text((StyledText, Option<Coordinates>)),
    Ribbon((StyledText, Option<Coordinates>)),
    Table((Vec<Vec<StyledText>>, Option<Coordinates>)),
    NestedList((Vec<(StyledText, usize)>, Option<Coordinates>)),
}
fn fit(s: &str, columns: usize) -> String {
    if columns == 0 {
        return String::new();
    }
    let mut end = 0;
    for (offset, c) in s.char_indices() {
        let next = offset + c.len_utf8();
        if s[..next].width() > columns {
            break;
        }
        end = next;
    }
    s[..end].to_owned()
}
#[op2]
#[serde]
pub(super) fn op_zellij_ui_pure<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &OpState,
    value: v8::Local<'s, v8::Value>,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    (|| -> Result<Value> {
        let (op, bytes) = input::<Pure>(scope, value)?;
        let mut temporary = NativeCharge::new(state.borrow::<Arc<Limits>>().clone());
        temporary.grow(bytes.checked_mul(256).context("UI allocation overflow")?)?;
        let width_check = |width| -> Result<()> {
            anyhow::ensure!(width <= MAX / 256, "text width exceeds transport budget");
            Ok(())
        };
        let output = match op {
            Pure::Width(s) => return Ok(json!(s.width())),
            Pure::Wrap((s, width)) => {
                width_check(width)?;
                return Ok(json!(text::wrap(&s, width)));
            },
            Pure::Sanitize(s) => text::sanitize(&s),
            Pure::Truncate((s, width, ellipsis)) => {
                width_check(width)?;
                if s.width() <= width {
                    s
                } else if ellipsis.width() > width {
                    fit(&ellipsis, width)
                } else {
                    fit(&s, width - ellipsis.width()) + ellipsis.as_str()
                }
            },
            Pure::Pad((s, width)) => {
                width_check(width)?;
                let s = fit(&s, width);
                let n = width.saturating_sub(s.width());
                s + " ".repeat(n).as_str()
            },
            Pure::PadLeft((s, width)) => {
                width_check(width)?;
                let s = fit(&s, width);
                " ".repeat(width.saturating_sub(s.width())) + s.as_str()
            },
            Pure::Center((s, width)) => {
                width_check(width)?;
                let s = fit(&s, width);
                let n = width.saturating_sub(s.width());
                " ".repeat(n / 2) + s.as_str() + " ".repeat(n - n / 2).as_str()
            },
            Pure::Text((text, c)) => {
                let text = text.build()?;
                if let Some(c) = c {
                    c.check()?;
                    serialize_text_with_coordinates(&text, c.x, c.y, Some(c.width), Some(c.height))
                } else {
                    serialize_text(&text)
                }
            },
            Pure::Ribbon((text, c)) => {
                let text = text.build()?;
                if let Some(c) = c {
                    c.check()?;
                    serialize_ribbon_with_coordinates(
                        &text,
                        c.x,
                        c.y,
                        Some(c.width),
                        Some(c.height),
                    )
                } else {
                    serialize_ribbon(&text)
                }
            },
            Pure::Table((rows, c)) => {
                let columns = rows.first().map_or(0, Vec::len);
                let mut table = Table::new();
                for row in rows {
                    anyhow::ensure!(row.len() == columns, "table rows must have equal lengths");
                    table = table.add_styled_row(
                        row.into_iter()
                            .map(StyledText::build)
                            .collect::<Result<_>>()?,
                    );
                }
                if let Some(c) = c {
                    c.check()?;
                    serialize_table_with_coordinates(
                        &table,
                        c.x,
                        c.y,
                        Some(c.width),
                        Some(c.height),
                    )
                } else {
                    serialize_table(&table)
                }
            },
            Pure::NestedList((items, c)) => {
                let indentation = items
                    .iter()
                    .try_fold(0usize, |sum, (_, n)| sum.checked_add(*n))
                    .context("list indentation overflow")?;
                anyhow::ensure!(
                    indentation <= MAX / 256,
                    "list output exceeds transport budget"
                );
                temporary.grow(indentation.saturating_mul(256))?;
                let items = items
                    .into_iter()
                    .map(|(s, indent)| -> Result<NestedListItem> {
                        anyhow::ensure!(
                            indent <= MAX / 256,
                            "list indentation exceeds transport budget"
                        );
                        let mut item = NestedListItem::new(&s.text).indent(indent);
                        if s.selected {
                            item = item.selected();
                        }
                        if s.opaque {
                            item = item.opaque();
                        }
                        if s.disabled {
                            item = item.disabled();
                        }
                        anyhow::ensure!(s.levels.len() <= 8, "style level out of range");
                        let len = s.text.chars().count();
                        for (level, indices) in s.levels.into_iter().enumerate() {
                            anyhow::ensure!(
                                indices.iter().all(|i| *i < len),
                                "style index out of range"
                            );
                            item = item.color_indices(level, indices);
                        }
                        Ok(item)
                    })
                    .collect::<Result<Vec<_>>>()?;
                if let Some(c) = c {
                    c.check()?;
                    serialize_nested_list_with_coordinates(
                        &items,
                        c.x,
                        c.y,
                        Some(c.width),
                        Some(c.height),
                    )
                } else {
                    serialize_nested_list(&items)
                }
            },
        };
        anyhow::ensure!(output.len() <= MAX, "UI output exceeds transport budget");
        Ok(json!(output))
    })()
    .map_err(error)
}
