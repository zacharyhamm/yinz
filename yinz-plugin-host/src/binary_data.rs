//! Encode typed data-store blobs before the general JSON bridge expands byte vectors.

use serde_json::{json, Value};
use yinz_runtime::errors::prelude::*;
use yinz_types::plugin_data::{DataEntityInfo, DataRows, DataValue};

pub(crate) fn rows(rows: &DataRows) -> Result<Value> {
    let mut total = 0usize;
    let values = rows
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| data_value(value, &mut total))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "columns": rows.columns,
        "rows": values,
        "truncated": rows.truncated,
        "next_cursor": rows.next_cursor,
    }))
}

pub(crate) fn catalog(mut entities: Vec<DataEntityInfo>) -> Result<Value> {
    let mut total = 0usize;
    let mut defaults = Vec::new();
    for (entity_index, entity) in entities.iter_mut().enumerate() {
        for (attr_index, attr) in entity.def.attrs.iter_mut().enumerate() {
            if matches!(attr.default, Some(DataValue::Blob(_))) {
                let value = attr.default.take().context("missing blob default")?;
                defaults.push((entity_index, attr_index, data_value(&value, &mut total)?));
            }
        }
    }
    let mut value = serde_json::to_value(entities).context("encode plugin data catalog")?;
    for (entity, attr, default) in defaults {
        value[entity]["def"]["attrs"][attr]["default"] = default;
    }
    Ok(value)
}

fn data_value(value: &DataValue, total: &mut usize) -> Result<Value> {
    match value {
        DataValue::Blob(bytes) => {
            *total = total.saturating_add(bytes.len());
            anyhow::ensure!(
                *total <= yinz_steel::binary::MAX_BYTES,
                "data binary payload exceeds 64 MiB"
            );
            Ok(json!({"Blob": yinz_steel::binary::encode(bytes)?}))
        },
        _ => serde_json::to_value(value).context("encode plugin data value"),
    }
}

#[cfg(test)]
#[path = "unit/binary_data.rs"]
mod tests;
