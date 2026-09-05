//! The parts of value handling both engines share.
//!
//! [`DataValue`] is defined in [`crate::plugin_data`] so that plugins can speak it too; each
//! backend's driver conversions live beside that backend, and what is left here is the clock and
//! the literal rendering the dialects agree on.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::plugin_data::{DataError, DataValue};

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Render the defaults both engines spell the same way, or `None` for one they differ on.
///
/// Text is escaped by doubling quotes, which is the standard SQL rule and holds on both engines
/// (Postgres with `standard_conforming_strings`, its default since 9.1). Booleans and blobs are
/// left to [`super::dialect::Dialect::literal`].
pub(super) fn common_literal(value: &DataValue) -> Option<Result<String, DataError>> {
    Some(Ok(match value {
        DataValue::Null => "NULL".to_owned(),
        DataValue::Int(i) => i.to_string(),
        DataValue::Real(f) => {
            if f.is_finite() {
                format!("{:?}", f)
            } else {
                return Some(Err(DataError::InvalidSchema(format!(
                    "{} is not a usable column default",
                    f
                ))));
            }
        },
        DataValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
        DataValue::Bool(_) | DataValue::Blob(_) => return None,
    }))
}

/// A blob's bytes as lowercase hex, for the engines' differing blob literal syntaxes.
pub(super) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}
