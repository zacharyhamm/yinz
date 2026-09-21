//! The same Kitty sequences used by Wasm plugins, composed into a guest render string.
use super::{limits::Limits, limits::NativeCharge, transport};
use deno_core::{op2, v8, OpState};
use deno_error::JsErrorBox;
use serde::Deserialize;
use std::sync::Arc;
use zellij_runtime::errors::prelude::*;
use zellij_tile::ui_components::{
    image_row, place_image_sequence, png_transmit_sequence, MAX_IMAGE_ID,
};
use zellij_types::kitty_placeholders::ROWCOLUMN_DIACRITICS;

#[derive(Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
enum Image {
    TransmitPng((u32, Vec<u8>)),
    Place((u32, usize, usize)),
    Row((u32, usize, usize)),
}

#[op2]
#[string]
pub(super) fn op_zellij_image<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &OpState,
    value: v8::Local<'s, v8::Value>,
) -> std::result::Result<String, JsErrorBox> {
    (|| -> Result<String> {
        let limits = state.borrow::<Arc<Limits>>();
        limits.check()?;
        let value = transport::from_v8(scope, value)?;
        let mut temporary = NativeCharge::new(limits.clone());
        // Account for the JSON byte vector and base64/terminal sequence copies.
        temporary.grow(value.to_string().len().saturating_mul(16))?;
        let request: Image = serde_json::from_value(value).context("invalid image arguments")?;
        let id = match &request {
            Image::TransmitPng((id, _)) | Image::Place((id, _, _)) | Image::Row((id, _, _)) => *id,
        };
        anyhow::ensure!(
            (1..=MAX_IMAGE_ID).contains(&id),
            "image id must be in 1..=0xffffff"
        );
        let output = match request {
            Image::TransmitPng((id, png)) => {
                anyhow::ensure!(png.len() <= 8 * 1024 * 1024, "image exceeds 8 MiB budget");
                png_transmit_sequence(id, &png)
            },
            Image::Place((id, columns, rows)) => {
                anyhow::ensure!(
                    (1..=ROWCOLUMN_DIACRITICS.len()).contains(&columns)
                        && (1..=ROWCOLUMN_DIACRITICS.len()).contains(&rows),
                    "image dimensions exceed placeholder range"
                );
                place_image_sequence(id, columns, rows)
            },
            Image::Row((id, row, columns)) => {
                anyhow::ensure!(
                    row < ROWCOLUMN_DIACRITICS.len() && columns <= ROWCOLUMN_DIACRITICS.len(),
                    "image row exceeds placeholder range"
                );
                image_row(id, row, columns)
            },
        };
        anyhow::ensure!(
            output.len() <= zellij_ipc::MAX_IPC_FRAME_LEN,
            "image output exceeds transport budget"
        );
        Ok(output)
    })()
    .map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}
