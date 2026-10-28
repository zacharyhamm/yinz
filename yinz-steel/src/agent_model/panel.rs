//! Pure shared panel typography; guest code owns placement and interaction.
use agent_proto::{agents, panel_text};
use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use yinz_tile::ui_components::forms;

use agent_proto::quota;

fn decode<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).context("invalid panel typography arguments")
}
pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    Ok(match op {
        "QuotaImage" => {
            let (value, color, terminal, width, height) = decode(args)?;
            let png = quota::render(value, color, terminal, width, height)?;
            json!(yinz_tile::ui_components::png_transmit_sequence(1, &png))
        },
        "Divider" => {
            let (label, above, below, cols): (String, usize, usize, usize) = decode(args)?;
            anyhow::ensure!(cols <= 16384, "panel width exceeds budget");
            json!(panel_text::divider_line(&label, above, below, cols))
        },
        "Readout" => {
            let (above, below) = decode(args)?;
            json!(panel_text::scroll_readout(above, below))
        },
        "WrapWords" => {
            let (text, width): (String, usize) = decode(args)?;
            json!(panel_text::wrap_words(&text, width))
        },
        "SingleLine" => {
            let (text,): (String,) = decode(args)?;
            json!(panel_text::single_line(&text))
        },
        "ShortAge" => {
            let (now, then) = decode(args)?;
            json!(agents::short_age(now, then))
        },
        "AgeGutter" => {
            let (now, then) = decode(args)?;
            json!(agents::age_gutter(now, then))
        },
        "FitLabel" => {
            let (label, width) = decode(args)?;
            json!(agents::fit_label(label, width))
        },
        "FitHintsMarked" => {
            let (segments, width, keep): (Vec<String>, usize, usize) = decode(args)?;
            let segments: Vec<_> = segments.iter().map(String::as_str).collect();
            json!(agents::fit_hints_marked(&segments, width, keep))
        },
        "Cut" => {
            let (text, width) = decode(args)?;
            json!(agents::cut_to_width(text, width))
        },
        "FitKeys" | "FitHints" => {
            let (segments, width): (Vec<String>, usize) = decode(args)?;
            let segments: Vec<_> = segments.iter().map(String::as_str).collect();
            json!(if op == "FitKeys" {
                agents::fit_keys(&segments, width)
            } else {
                agents::fit_hints(&segments, width)
            })
        },
        "KeySheetLines" => {
            let (keys, what, key_width, cols): (String, String, usize, usize) = decode(args)?;
            anyhow::ensure!(
                key_width <= 16384 && cols <= 16384,
                "panel width exceeds budget"
            );
            json!(panel_text::key_sheet_lines(&keys, &what, key_width, cols))
        },
        "InputWindow" => {
            let (prompt, text, cursor, width): (String, String, usize, usize) = decode(args)?;
            anyhow::ensure!(width <= 16384, "panel width exceeds budget");
            json!(forms::viewport_window(&prompt, &text, cursor, width))
        },
        _ => bail!("unknown panel typography operation {op}"),
    })
}
