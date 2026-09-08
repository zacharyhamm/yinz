//! Interpreter-local widget state. All rendering and interaction lives in zellij-tile.
use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};
use serde::Deserialize;
use zellij_tile::text;
use zellij_tile::ui_components::{forms::*, serialize_text_with_coordinates, Text};
use zellij_tile_markdown as markdown;
use zellij_types::data::{KeyWithModifier, Mouse};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    kind: String,
    id: String,
    label: String,
    value: serde_json::Value,
    options: Vec<String>,
    placeholder: String,
    max_visible_options: usize,
    disabled: bool,
    style: String,
}

fn error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(message.into())
}

fn build(fields: Vec<Descriptor>) -> mlua::Result<Form> {
    let mut form = Form::new();
    let mut ids = std::collections::HashSet::new();
    for d in fields {
        if d.id.is_empty() || !ids.insert(d.id.clone()) {
            return Err(error("widget IDs must be nonempty and unique"));
        }
        let string = || {
            d.value
                .as_str()
                .ok_or_else(|| error(format!("{}: value must be a string", d.id)))
        };
        let index = || -> mlua::Result<Option<usize>> {
            if d.value.is_null() {
                return Ok(None);
            }
            let n = d
                .value
                .as_u64()
                .filter(|n| *n > 0 && *n <= d.options.len() as u64)
                .ok_or_else(|| {
                    error(format!("{}: value must be a one-based option index", d.id))
                })?;
            Ok(Some(n as usize - 1))
        };
        form = match d.kind.as_str() {
            "text_input" => form.field(&d.id, &d.label, TextInput::new(string()?.to_owned())),
            "checkbox" => form.checkbox(
                &d.id,
                &d.label,
                Checkbox::new(
                    d.value
                        .as_bool()
                        .ok_or_else(|| error("checkbox value must be boolean"))?,
                ),
            ),
            "radio_group" => {
                let selected = index()?;
                let mut radio = RadioGroup::new(&d.options);
                if let Some(i) = selected {
                    radio.select_index(i);
                }
                form.radio(&d.id, &d.label, radio)
            },
            "select" => {
                let selected = index()?;
                let mut select = Select::new(&d.options)
                    .placeholder(&d.placeholder)
                    .max_visible_options(d.max_visible_options);
                if let Some(i) = selected {
                    select.select_index(i);
                }
                form.select(&d.id, &d.label, select)
            },
            "combo" => form.combo(
                &d.id,
                &d.label,
                Combo::new(&d.options)
                    .with_text(string()?)
                    .placeholder(&d.placeholder)
                    .max_visible_options(d.max_visible_options),
            ),
            "button" => {
                let style = match d.style.as_str() {
                    "ribbon" => ButtonStyle::Ribbon,
                    "text" => ButtonStyle::Text,
                    _ => return Err(error("button style must be ribbon or text")),
                };
                form.button(
                    &d.id,
                    Button::new(&d.label).disabled(d.disabled).style(style),
                )
            },
            _ => return Err(error(format!("unknown widget kind: {}", d.kind))),
        };
    }
    Ok(form)
}

enum Widget {
    Form(Form),
    Dialog(Dialog),
}
impl Widget {
    fn form(&self) -> &Form {
        match self {
            Self::Form(f) => f,
            Self::Dialog(d) => d.form(),
        }
    }
    fn form_mut(&mut self) -> &mut Form {
        match self {
            Self::Form(f) => f,
            Self::Dialog(d) => d.form_mut(),
        }
    }
}

fn action(lua: &Lua, event: FormEvent) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("should_render", event.should_render())?;
    let (kind, id) = match event {
        FormEvent::FieldChanged(id) => ("field_changed", Some(id)),
        FormEvent::Complete(id) => ("complete", Some(id)),
        FormEvent::ButtonPressed(id) => ("button_pressed", Some(id)),
        FormEvent::FocusMoved => ("focus_moved", None),
        FormEvent::Cancel => ("cancel", None),
        FormEvent::Ignored => ("ignored", None),
    };
    t.set("kind", kind)?;
    t.set("id", id)?;
    Ok(t)
}

impl UserData for Widget {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("key", |lua, this, value: Value| {
            let key: KeyWithModifier = lua.from_value(value)?;
            action(lua, this.form_mut().handle_key(key))
        });
        m.add_method_mut("mouse", |lua, this, value: Value| {
            let mouse: Mouse = lua.from_value(value)?;
            action(lua, this.form_mut().handle_mouse(mouse))
        });
        m.add_method_mut("paste", |lua, this, text: String| {
            let event = this
                .form_mut()
                .insert_pasted_text(&text)
                .map(FormEvent::FieldChanged)
                .unwrap_or(FormEvent::Ignored);
            action(lua, event)
        });
        m.add_method("focused_id", |_, this, ()| {
            Ok(this.form().focused_id().map(str::to_owned))
        });
        m.add_method_mut("focus", |_, this, id: String| {
            if !this.form_mut().focus_on(&id) {
                return Err(error(format!("unknown widget ID: {id}")));
            }
            Ok(())
        });
        m.add_method_mut("move_focus", |lua, this, forward: bool| {
            action(lua, this.form_mut().move_focus(forward))
        });
        m.add_method("value", |lua, this, id: String| {
            let f = this
                .form()
                .field_by_id(&id)
                .ok_or_else(|| error(format!("unknown field ID: {id}")))?;
            if let Some(c) = f.checkbox() {
                return Ok(Value::Boolean(c.is_checked()));
            }
            if let Some(r) = f.radio() {
                return Ok(r
                    .selected_index()
                    .map(|i| Value::Integer((i + 1) as i64))
                    .unwrap_or(Value::Nil));
            }
            if let Some(s) = f.select() {
                return Ok(s
                    .selected_index()
                    .map(|i| Value::Integer((i + 1) as i64))
                    .unwrap_or(Value::Nil));
            }
            Ok(Value::String(lua.create_string(f.text())?))
        });
        m.add_method_mut("set_value", |lua, this, (id, value): (String, Value)| {
            let f = this
                .form_mut()
                .field_by_id_mut(&id)
                .ok_or_else(|| error(format!("unknown field ID: {id}")))?;
            if let Some(c) = f.checkbox_mut() {
                c.set_checked(lua.from_value(value)?);
            } else if let Some(t) = f.input_mut() {
                t.set_text(lua.from_value(value)?);
            } else if let Some(c) = f.combo_mut() {
                c.set_text(lua.from_value::<String>(value)?);
            } else {
                let i: usize = lua.from_value(value)?;
                let i = i
                    .checked_sub(1)
                    .ok_or_else(|| error("option indices start at 1"))?;
                if let Some(s) = f.select_mut() {
                    if i >= s.options().len() {
                        return Err(error("option index out of range"));
                    }
                    s.select_index(i);
                } else if let Some(r) = f.radio_mut() {
                    if i >= r.options().len() {
                        return Err(error("option index out of range"));
                    }
                    r.select_index(i);
                }
            }
            Ok(())
        });
        m.add_method_mut(
            "set_options",
            |lua, this, (id, options): (String, Value)| {
                let options: Vec<String> = lua.from_value(options)?;
                let f = this
                    .form_mut()
                    .field_by_id_mut(&id)
                    .ok_or_else(|| error(format!("unknown field ID: {id}")))?;
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
                    return Err(error("set_options requires a select, combo or radio group"));
                }
                Ok(())
            },
        );
        m.add_method_mut(
            "render",
            |_, this, (x, y, width, height): (usize, usize, usize, usize)| {
                let buffer = match this {
                    Self::Form(f) => f.render_buffer_in_viewport(x, y, width, height),
                    Self::Dialog(d) => d.render_buffer(Rect::new(x, y, width, height)),
                };
                Ok((
                    buffer.output,
                    buffer.cursor.is_some(),
                    buffer.cursor.flatten().map(|(x, _)| x),
                    buffer.cursor.flatten().map(|(_, y)| y),
                ))
            },
        );
        m.add_method_mut("set_status", |_, this, text: Option<String>| {
            match this {
                Self::Dialog(d) => d.set_status(text.map(Text::new)),
                _ => return Err(error("set_status requires a dialog")),
            }
            Ok(())
        });
    }
}

struct Markdown {
    blocks: Vec<markdown::MdBlock>,
    width: Option<usize>,
    lines: Vec<markdown::MdLine>,
    compact: bool,
    dim: bool,
}
impl UserData for Markdown {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut(
            "render",
            |_, this, (x, y, width, height, scroll): (usize, usize, usize, usize, usize)| {
                if this.width != Some(width) {
                    this.lines = markdown::wrap(
                        &this.blocks,
                        width,
                        markdown::Options {
                            compact_blocks: this.compact,
                        },
                    );
                    this.width = Some(width);
                }
                let mut output = String::new();
                if width > 0 {
                    for (row, line) in this.lines.iter().skip(scroll).take(height).enumerate() {
                        output.push_str(&serialize_text_with_coordinates(
                            &markdown::style_text(&line.text, &line.styles, this.dim),
                            x,
                            y + row,
                            Some(width),
                            Some(1),
                        ));
                    }
                }
                Ok((output, this.lines.len()))
            },
        );
    }
}

pub(super) fn install(lua: &Lua, zellij: &Table) -> mlua::Result<()> {
    let native = lua.create_table()?;
    native.set(
        "form",
        lua.create_function(|lua, (fields, dialog): (Value, Option<Table>)| {
            let form = build(lua.from_value(fields)?)?;
            Ok(if let Some(t) = dialog {
                let mut d = Dialog::new(Text::new(t.get::<String>("title")?), form);
                if let Some(s) = t.get::<Option<String>>("message")? {
                    d = d.message(Text::new(s));
                }
                if let Some(s) = t.get::<Option<String>>("help")? {
                    d = d.help(Text::new(s));
                }
                Widget::Dialog(d)
            } else {
                Widget::Form(form)
            })
        })?,
    )?;
    native.set(
        "markdown",
        lua.create_function(|_, (source, compact, dim): (String, bool, bool)| {
            Ok(Markdown {
                blocks: markdown::parse(&source),
                width: None,
                lines: vec![],
                compact,
                dim,
            })
        })?,
    )?;
    let text_table: Table = zellij.get("text")?;
    text_table.set(
        "wrap",
        lua.create_function(|_, (source, width): (String, usize)| Ok(text::wrap(&source, width)))?,
    )?;
    text_table.set(
        "sanitize",
        lua.create_function(|_, source: String| Ok(text::sanitize(&source)))?,
    )?;
    lua.load(include_str!("ui.lua"))
        .set_name("zellij:ui")
        .call::<()>((native, zellij.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_types::data::BareKey;

    fn interpreter() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .set("__zellij_cancelled_by_host", "cancelled by host")
            .unwrap();
        let zellij: Table = lua.load(super::super::PRELUDE).eval().unwrap();
        zellij.set("null", lua.null()).unwrap();
        lua.globals().set("zellij", zellij.clone()).unwrap();
        install(&lua, &zellij).unwrap();
        lua.load("zellij.show_cursor = function(c) cursor = c end")
            .exec()
            .unwrap();
        lua
    }

    #[test]
    fn lua_forms_share_rust_editing_focus_dropdown_mouse_and_rendering() {
        let lua = interpreter();
        lua.load(
            r#"
            f = zellij.ui.form {
                {kind='text_input', id='name', label='Name', value='日'},
                {kind='select', id='layout', label='Layout', options={'default','compact'}},
                {kind='combo', id='branch', label='Branch', options={'main','feature'}},
                {kind='checkbox', id='frames', label='Frames'},
                {kind='radio_group', id='mode', label='Mode', options={'one','two'}},
                {kind='button', id='save', label='Save'},
            }
        "#,
        )
        .exec()
        .unwrap();
        let mut form = Form::new()
            .field("name", "Name", TextInput::new("日".into()))
            .select("layout", "Layout", Select::new(["default", "compact"]))
            .combo("branch", "Branch", Combo::new(["main", "feature"]))
            .checkbox("frames", "Frames", Checkbox::new(false))
            .radio("mode", "Mode", RadioGroup::new(["one", "two"]))
            .button("save", Button::new("Save"));
        let compare = |lua: &Lua, form: &mut Form, width, height| {
            lua.globals().set("width", width).unwrap();
            lua.globals().set("height", height).unwrap();
            let actual: String = lua
                .load("return f:render{x=2,y=3,width=width,height=height}")
                .eval()
                .unwrap();
            let buffer = form.render_buffer_in_viewport(2, 3, width, height);
            assert_eq!(actual, buffer.output);
            let cursor: Option<(usize, usize)> = lua
                .from_value(lua.globals().get("cursor").unwrap())
                .unwrap();
            assert_eq!(cursor, buffer.cursor.flatten());
        };
        compare(&lua, &mut form, 30, 8);
        lua.load(r"f:insert_pasted_text('本語\nhello')")
            .exec()
            .unwrap();
        form.insert_pasted_text("本語\nhello");
        let keys = [
            BareKey::Left,
            BareKey::Backspace,
            BareKey::Tab,
            BareKey::Enter,
            BareKey::Down,
            BareKey::Char('c'),
            BareKey::Enter,
            BareKey::Tab,
            BareKey::Char('f'),
            BareKey::Down,
            BareKey::Enter,
            BareKey::Tab,
            BareKey::Char(' '),
            BareKey::Tab,
            BareKey::Enter,
            BareKey::Right,
            BareKey::Tab,
            BareKey::Enter,
            BareKey::Esc,
        ];
        for bare_key in keys {
            let key = KeyWithModifier::new(bare_key);
            lua.globals()
                .set("key", lua.to_value(&key).unwrap())
                .unwrap();
            let actual: Table = lua.load("return f:handle_key(key)").eval().unwrap();
            let expected = action(&lua, form.handle_key(key)).unwrap();
            for name in ["kind", "id", "should_render"] {
                assert_eq!(
                    actual.get::<Value>(name).unwrap(),
                    expected.get::<Value>(name).unwrap()
                );
            }
            for (width, height) in [(30, 8), (10, 2), (1, 1), (0, 0), (30, 8)] {
                compare(&lua, &mut form, width, height);
            }
        }
        for mouse in [
            Mouse::LeftClick(3, 3),
            Mouse::Release(3, 3),
            Mouse::LeftClick(8, 3),
            Mouse::Release(8, 3),
        ] {
            lua.globals()
                .set("mouse", lua.to_value(&mouse).unwrap())
                .unwrap();
            let actual: Table = lua
                .load("return f:handle_mouse{value=mouse}")
                .eval()
                .unwrap();
            let expected = action(&lua, form.handle_mouse(mouse)).unwrap();
            assert_eq!(
                actual.get::<String>("kind").unwrap(),
                expected.get::<String>("kind").unwrap()
            );
            compare(&lua, &mut form, 30, 8);
        }
        assert_eq!(
            lua.load("return f:value('name')").eval::<String>().unwrap(),
            form.text("name")
        );
    }

    #[test]
    fn lua_dialog_output_matches_rust_at_every_viewport_size() {
        let lua = interpreter();
        lua.load(
            r#"d = zellij.ui.dialog{
            title='Confirm', message='Continue?', help='Esc: cancel',
            fields={{kind='button',id='yes',label='Yes'}, {kind='button',id='no',label='No'}},
        }"#,
        )
        .exec()
        .unwrap();
        let mut dialog = Dialog::new(
            Text::new("Confirm"),
            Form::new()
                .button("yes", Button::new("Yes"))
                .button("no", Button::new("No")),
        )
        .message(Text::new("Continue?"))
        .help(Text::new("Esc: cancel"));
        for (width, height) in [(40, 12), (10, 4), (1, 1), (0, 0), (40, 12)] {
            lua.globals().set("width", width).unwrap();
            lua.globals().set("height", height).unwrap();
            let output: String = lua
                .load("return d:render{x=2,y=3,width=width,height=height}")
                .eval()
                .unwrap();
            assert_eq!(
                output,
                dialog.render_buffer(Rect::new(2, 3, width, height)).output
            );
        }
    }

    #[test]
    fn lua_controls_activate_explicitly_and_submit_only_through_buttons() {
        interpreter()
            .load(
                r#"
            local ui = zellij.ui
            local input = ui.text_input{}
            assert(input:handle_key('Enter').kind == 'focus_moved')
            assert(input:handle_key('x').kind == 'field_changed')
            assert(input:handle_key('Enter').kind == 'focus_moved')
            assert(input:value() == 'x')
            local radio = ui.radio_group{options={'one','two'}}
            radio:handle_key('Right')
            assert(radio:value() == 1)
            radio:handle_key(' ')
            assert(radio:value() == 1)
            assert(radio:handle_key('Right').kind == 'field_changed')
            assert(radio:value() == 2)
            radio:handle_key('Esc')
            assert(radio:handle_key('Esc').kind == 'cancel')
            local select = ui.select{options={'one','two'}}
            select:handle_key('Enter')
            assert(select:handle_key('Down').kind == 'focus_moved')
            assert(select:value() == 1)
            select:handle_key('Esc')
            assert(select:value() == 1)
            local combo = ui.combo{options={'one','two'}}
            combo:handle_key('Enter')
            combo:handle_key('Down')
            assert(combo:handle_key('Enter').kind == 'field_changed')
            assert(combo:value() == 'one')
            assert(combo:handle_key('Esc').kind == 'cancel')
            local dialog = ui.dialog{title='Answer', fields={
                {kind='text_input',id='answer'}, {kind='button',id='submit',label='Submit'}
            }}
            dialog:insert_pasted_text('answer')
            assert(dialog:handle_key('Enter').kind == 'focus_moved')
            assert(dialog:handle_key('Right').kind == 'focus_moved')
            local action = dialog:handle_key('Enter')
            assert(action.kind == 'button_pressed' and action.id == 'submit')
            assert(dialog:value('answer') == 'answer')
        "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn lua_widgets_validate_descriptors_values_and_keep_state_per_interpreter() {
        for _ in 0..2 {
            let lua = interpreter();
            lua.load(r#"
                local ui = zellij.ui
                local input = ui.text_input{}
                assert(input:value() == '')
                input:set_value('日本語'):focus()
                assert(input:value() == '日本語')
                local select = ui.select{options={}}
                assert(select:value() == nil)
                select:set_options({'a','b'}):set_value(2)
                assert(select:value() == 2)
                local combo = ui.combo{options={'main'}}
                combo:set_options({}):set_value('new')
                assert(combo:value() == 'new')
                local checkbox = ui.checkbox{}
                checkbox:set_value(true)
                assert(checkbox:value())
                local radio = ui.radio_group{options={'a','b'}, value=2}
                radio:set_options({'b','c'})
                assert(radio:value() == 1)
                local button = ui.button{label='OK'}
                assert(button:handle_key('Enter').kind == 'button_pressed')
                local dialog = ui.dialog{title='Confirm', fields={{kind='button',id='yes',label='Yes'}}}
                dialog:set_status('Ready')
                assert(dialog:handle_key('Enter').id == 'yes')
                assert(dialog:handle_key('Esc').kind == 'cancel')
                assert(dialog:render{width=0,height=0} == '')
                for _, bad in ipairs({
                    function() ui.form{{kind='text_input'}} end,
                    function() ui.form{{kind='text_input',id='a'}, {kind='checkbox',id='a'}} end,
                    function() ui.text_input{value=1} end,
                    function() ui.checkbox{value='yes'} end,
                    function() ui.select{options={'a'},value=0} end,
                    function() ui.select{options={false}} end,
                    function() ui.select{options={ [2]='hole' }} end,
                    function() ui.text_input{typo=true} end,
                    function() local a={'a','b','c'}; a[2]=nil; ui.select{options=a} end,
                    function() local a={{kind='text_input',id='a'}, {}, {kind='text_input',id='b'}}; a[2]=nil; ui.form(a) end,
                    function() input:set_value(false) end,
                    function() input:focus('missing') end,
                    function() select:set_value(3) end,
                    function() input:render{width=-1,height=1} end,
                    function() input:render{width=1.5,height=1} end,
                    function() input:handle_key(false) end,
                    function() input:handle_mouse{wat=1} end,
                }) do
                    local ok, err = pcall(bad)
                    assert(not ok and #tostring(err) > 0)
                end
            "#).exec().unwrap();
        }
    }

    #[test]
    fn lua_markdown_reuses_parse_and_matches_shared_wrapping_after_resize() {
        let lua = interpreter();
        let source =
            "# 日本語\n\n**bold** ~~gone~~\n\n- [x] done\n\n\u{1b}[31mred\n\n|a|b|\n|-|-|\n|1|2|";
        lua.globals().set("source", source).unwrap();
        lua.load(
            "md = zellij.ui.markdown(source); dimmed = zellij.ui.markdown(source, {dim=true})",
        )
        .exec()
        .unwrap();
        for (width, dim) in [false, true]
            .into_iter()
            .flat_map(|dim| [30, 1, 0, 12, 30].map(|width| (width, dim)))
        {
            lua.globals().set("width", width).unwrap();
            lua.globals().set("dim", dim).unwrap();
            let (actual, count): (String, usize) = lua
                .load("return (dim and dimmed or md):render{x=0,y=0,width=width,height=3,scroll=1}")
                .eval()
                .unwrap();
            let lines = markdown::render(source, width, markdown::Options::default());
            let expected: String = if width == 0 {
                String::new()
            } else {
                lines
                    .iter()
                    .skip(1)
                    .take(3)
                    .enumerate()
                    .map(|(row, l)| {
                        serialize_text_with_coordinates(
                            &markdown::style_text(&l.text, &l.styles, dim),
                            0,
                            row,
                            Some(width),
                            Some(1),
                        )
                    })
                    .collect()
            };
            assert_eq!(actual, expected);
            assert_eq!(count, lines.len());
            let wrapped: Vec<String> = lua
                .load("return zellij.text.wrap(source,width)")
                .eval()
                .unwrap();
            assert_eq!(wrapped, text::wrap(source, width));
        }
    }
}
