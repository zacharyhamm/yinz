use super::{Coordinates, MAX};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_width::UnicodeWidthStr;
use yinz_tile::{
    text,
    ui_components::{forms::fuzzy_filter, *},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StyledText {
    pub(super) text: String,
    #[serde(default)]
    pub(super) selected: bool,
    #[serde(default)]
    pub(super) opaque: bool,
    #[serde(default)]
    pub(super) disabled: bool,
    #[serde(default)]
    pub(super) levels: Vec<Vec<usize>>,
}
impl StyledText {
    pub(super) fn build(self) -> Result<Text> {
        anyhow::ensure!(
            self.levels.len() <= 9,
            "style level must be between 0 and 8"
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
// Pure render commands preserve the SDK's optional clipping dimensions.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    x: usize,
    y: usize,
    width: Option<usize>,
    height: Option<usize>,
    #[serde(default)]
    scroll: usize,
}
impl Placement {
    fn check(&self) -> Result<()> {
        Coordinates {
            x: self.x,
            y: self.y,
            width: self.width.unwrap_or(0),
            height: self.height.unwrap_or(0),
            scroll: self.scroll,
        }
        .check()
    }
}
#[derive(Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
enum Pure {
    Width(String),
    Wrap((String, usize)),
    WrapInput((String, usize)),
    FuzzyFilter((Vec<String>, String)),
    Sanitize(String),
    Truncate((String, usize, String)),
    TruncateAnsi((String, usize)),
    Pad((String, usize)),
    PadLeft((String, usize)),
    Center((String, usize)),
    Text((StyledText, Option<Placement>)),
    Ribbon((StyledText, Option<Placement>)),
    Table((Vec<Vec<StyledText>>, Option<Placement>)),
    NestedList((Vec<(StyledText, usize)>, Option<Placement>)),
    ListWindow((usize, usize, Option<usize>)),
    ImagePng((u32, Vec<u8>)),
    ImagePlace((u32, usize, usize)),
    ImageRow((u32, usize, usize)),
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

pub(super) fn run(value: Value) -> Result<Value> {
    let op = serde_json::from_value(value).context("invalid pure UI arguments")?;
    let width_check = |width| -> Result<()> {
        anyhow::ensure!(width <= MAX / 256, "text width exceeds transport budget");
        Ok(())
    };
    let output = match op {
        Pure::TruncateAnsi((source, width)) => {
            width_check(width)?;
            let mut end = source.len();
            if width == 0 {
                end = 0;
            } else if strip_ansi_escapes::strip_str(&source).width() > width {
                let mut work = 0usize;
                end = 0;
                for (offset, character) in source.char_indices() {
                    let next = offset + character.len_utf8();
                    work = work.saturating_add(next);
                    anyhow::ensure!(work <= MAX * 4, "ANSI truncation exceeds work budget");
                    if strip_ansi_escapes::strip_str(&source[..next]).width() > width {
                        break;
                    }
                    end = next;
                }
            }
            format!("{}\x1b[0m", &source[..end])
        },
        Pure::ListWindow((count, rows, selected)) => {
            width_check(rows)?;
            return Ok(json!(yinz_tile::ui_components::forms::list_window(
                count, rows, selected
            )));
        },
        Pure::ImagePng((id, png)) => {
            image_id(id)?;
            anyhow::ensure!(png.len() <= MAX / 2, "image source exceeds render budget");
            png_transmit_sequence(id, &png)
        },
        Pure::ImagePlace((id, columns, rows)) => {
            image_id(id)?;
            let dimensions = yinz_types::kitty_placeholders::ROWCOLUMN_DIACRITICS.len();
            anyhow::ensure!(
                (1..=dimensions).contains(&columns) && (1..=dimensions).contains(&rows),
                "image dimensions exceed placeholder range"
            );
            place_image_sequence(id, columns, rows)
        },
        Pure::ImageRow((id, row, columns)) => {
            image_id(id)?;
            let dimensions = yinz_types::kitty_placeholders::ROWCOLUMN_DIACRITICS.len();
            anyhow::ensure!(
                row < dimensions && columns <= dimensions,
                "image row exceeds placeholder range"
            );
            image_row(id, row, columns)
        },
        Pure::Width(s) => return Ok(json!(s.width())),
        Pure::Wrap((s, width)) => {
            width_check(width)?;
            return Ok(json!(text::wrap(&s, width)));
        },
        Pure::WrapInput((s, width)) => {
            width_check(width)?;
            let lines: Vec<_> = text::wrap_input(&s, width)
                .into_iter()
                .map(|line| json!({"text":line.text,"start":line.start}))
                .collect();
            let output = json!(lines);
            anyhow::ensure!(
                output.to_string().len() <= MAX,
                "UI output exceeds transport budget"
            );
            return Ok(output);
        },
        Pure::FuzzyFilter((options, query)) => {
            // Bound the native matcher's quadratic work before entering it.
            let (cells, _) = options
                .iter()
                .try_fold((0usize, 0usize), |(total, peak), option| {
                    let cells = option
                        .len()
                        .checked_add(1)?
                        .checked_mul(query.len().checked_add(1)?)?;
                    Some((total.checked_add(cells)?, peak.max(cells)))
                })
                .context("fuzzy matching work overflow")?;
            anyhow::ensure!(cells <= MAX, "fuzzy matching exceeds work budget");
            let matches: Vec<_> = fuzzy_filter(&options, &query)
                .into_iter()
                .map(|m| json!({"index":m.index,"score":m.score,"indices":m.indices}))
                .collect();
            let output = json!(matches);
            anyhow::ensure!(
                output.to_string().len() <= MAX,
                "UI output exceeds transport budget"
            );
            return Ok(output);
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
            let s = if s.width() > width { fit(&s, width) } else { s };
            let n = width.saturating_sub(s.width());
            s + " ".repeat(n).as_str()
        },
        Pure::PadLeft((s, width)) => {
            width_check(width)?;
            let s = if s.width() > width { fit(&s, width) } else { s };
            " ".repeat(width.saturating_sub(s.width())) + s.as_str()
        },
        Pure::Center((s, width)) => {
            width_check(width)?;
            let s = if s.width() > width { fit(&s, width) } else { s };
            let n = width.saturating_sub(s.width());
            " ".repeat(n / 2) + s.as_str() + " ".repeat(n - n / 2).as_str()
        },
        Pure::Text((text, c)) => {
            let text = text.build()?;
            if let Some(c) = c {
                c.check()?;
                serialize_text_with_coordinates(&text, c.x, c.y, c.width, c.height)
            } else {
                serialize_text(&text)
            }
        },
        Pure::Ribbon((text, c)) => {
            let text = text.build()?;
            if let Some(c) = c {
                c.check()?;
                serialize_ribbon_with_coordinates(&text, c.x, c.y, c.width, c.height)
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
                serialize_table_with_coordinates(&table, c.x, c.y, c.width, c.height)
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
                    anyhow::ensure!(s.levels.len() <= 9, "style level out of range");
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
                serialize_nested_list_with_coordinates(&items, c.x, c.y, c.width, c.height)
            } else {
                serialize_nested_list(&items)
            }
        },
    };
    anyhow::ensure!(output.len() <= MAX, "UI output exceeds transport budget");
    Ok(json!(output))
}

fn image_id(id: u32) -> Result<()> {
    anyhow::ensure!(
        (1..=MAX_IMAGE_ID).contains(&id),
        "image id must be in 1..=0xffffff"
    );
    Ok(())
}
