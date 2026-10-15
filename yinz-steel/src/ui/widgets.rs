//! Runtime-independent adapters for the shared Rust widgets.
use serde::Deserialize;
use yinz_tile::ui_components::{forms::*, serialize_text_with_coordinates};
use yinz_tile_markdown as markdown;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub kind: String,
    pub id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    placeholder: String,
    #[serde(default = "default_visible_options")]
    pub max_visible_options: usize,
    #[serde(default)]
    disabled: bool,
    #[serde(default = "default_button_style")]
    style: String,
}

fn default_visible_options() -> usize {
    6
}
fn default_button_style() -> String {
    "ribbon".into()
}

fn error(message: impl Into<String>) -> String {
    message.into()
}

pub fn build(fields: Vec<Descriptor>, index_base: usize) -> Result<Form, String> {
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
        let index = || -> Result<Option<usize>, String> {
            if d.value.is_null() {
                return Ok(None);
            }
            let n = d
                .value
                .as_u64()
                .filter(|n| {
                    *n >= index_base as u64 && *n - (index_base as u64) < d.options.len() as u64
                })
                .ok_or_else(|| error(format!("{}: value must be a valid option index", d.id)))?;
            Ok(Some(n as usize - index_base))
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
                } else if index_base == 0 {
                    radio.clear_selection();
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
                } else if index_base == 0 {
                    select.clear_selection();
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

pub enum Widget {
    Form(Form),
    Dialog(Dialog),
}
impl Widget {
    pub fn form_mut(&mut self) -> &mut Form {
        match self {
            Self::Form(f) => f,
            Self::Dialog(d) => d.form_mut(),
        }
    }
}

pub struct Markdown {
    blocks: Vec<markdown::MdBlock>,
    width: Option<usize>,
    lines: Vec<markdown::MdLine>,
    compact: bool,
    dim: bool,
}
impl Markdown {
    pub fn new(source: &str, compact: bool, dim: bool) -> Self {
        Self {
            blocks: markdown::parse(source),
            width: None,
            lines: vec![],
            compact,
            dim,
        }
    }
    /// Upper bound for wrapping storage, including temporary styled character vectors.
    pub fn wrapping_budget(&self, width: usize) -> usize {
        self.blocks.iter().fold(0usize, |total, block| {
            let prefix = block.prefix.len().max(block.continuation.len());
            let chars = if block.rule {
                width
            } else if let Some(table) = &block.table {
                let rows = std::iter::once(&table.head).chain(table.rows.iter());
                let cells = rows
                    .map(|row| {
                        row.iter()
                            .map(|cell| cell.len().saturating_add(1))
                            .sum::<usize>()
                    })
                    .sum::<usize>();
                // Narrow tables repeat headers as records; wide tables pad each wrapped band.
                let headers = table.head.iter().map(Vec::len).sum::<usize>();
                cells.saturating_add(1).saturating_mul(
                    width
                        .saturating_add(headers)
                        .saturating_add(prefix)
                        .saturating_add(16),
                )
            } else {
                block
                    .chars
                    .len()
                    .saturating_add(1)
                    .saturating_mul(prefix.saturating_add(8))
            };
            total.saturating_add(chars.saturating_mul(128).saturating_add(512))
        })
    }
    pub fn styled_lines(
        &mut self,
        width: usize,
    ) -> impl Iterator<Item = yinz_tile::ui_components::Text> + '_ {
        if self.width != Some(width) {
            self.lines = markdown::wrap(
                &self.blocks,
                width,
                markdown::Options {
                    compact_blocks: self.compact,
                },
            );
            self.width = Some(width);
        }
        let dim = self.dim;
        self.lines
            .iter()
            .map(move |line| markdown::style_text(&line.text, &line.styles, dim))
    }
    pub fn render(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        scroll: usize,
    ) -> (String, usize) {
        let mut output = String::new();
        let visible = if width > 0 { height } else { 0 };
        for (row, line) in self
            .styled_lines(width)
            .skip(scroll)
            .take(visible)
            .enumerate()
        {
            output.push_str(&serialize_text_with_coordinates(
                &line,
                x,
                y + row,
                Some(width),
                Some(1),
            ));
        }
        (output, self.lines.len())
    }
}
