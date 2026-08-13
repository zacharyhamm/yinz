//! Carrying values across the SQLite boundary.
//!
//! [`DataValue`] is defined in [`crate::plugin_data`] so that plugins can speak it too; the
//! `rusqlite` half of the conversion is host-only and lives here.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::ToSql;

use crate::plugin_data::{AttrKind, DataError, DataValue};

use super::sql::storage_err;

impl ToSql for DataValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            DataValue::Null => ToSqlOutput::Owned(Value::Null),
            DataValue::Int(i) => ToSqlOutput::Owned(Value::Integer(*i)),
            DataValue::Real(f) => ToSqlOutput::Owned(Value::Real(*f)),
            // SQLite has no boolean storage class.
            DataValue::Bool(b) => ToSqlOutput::Owned(Value::Integer(*b as i64)),
            DataValue::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            DataValue::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Render a default as a SQL literal.
///
/// Defaults are the one place a value cannot be a bound parameter — SQLite requires a literal in a
/// column definition — so text is escaped by doubling quotes and blobs are rendered as hex.
pub(super) fn sql_literal(value: &DataValue) -> Result<String, DataError> {
    Ok(match value {
        DataValue::Null => "NULL".to_owned(),
        DataValue::Int(i) => i.to_string(),
        DataValue::Bool(b) => (*b as i64).to_string(),
        DataValue::Real(f) => {
            if f.is_finite() {
                format!("{:?}", f)
            } else {
                return Err(DataError::InvalidSchema(format!(
                    "{} is not a usable column default",
                    f
                )));
            }
        },
        DataValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
        DataValue::Blob(b) => {
            let mut hex = String::with_capacity(b.len() * 2 + 3);
            hex.push_str("X'");
            for byte in b {
                hex.push_str(&format!("{:02x}", byte));
            }
            hex.push('\'');
            hex
        },
    })
}

pub(super) fn read_value(
    row: &rusqlite::Row<'_>,
    index: usize,
    kind: Option<AttrKind>,
) -> Result<DataValue, DataError> {
    let value = row
        .get_ref(index)
        .map_err(|e| storage_err("failed to read a value", e))?;
    Ok(match (value, kind) {
        (ValueRef::Null, _) => DataValue::Null,
        // Only a column declared Bool comes back as one; SQLite stores both in an INTEGER.
        (ValueRef::Integer(i), Some(AttrKind::Bool)) => DataValue::Bool(i != 0),
        (ValueRef::Integer(i), _) => DataValue::Int(i),
        (ValueRef::Real(f), _) => DataValue::Real(f),
        (ValueRef::Text(t), _) => DataValue::Text(String::from_utf8_lossy(t).into_owned()),
        (ValueRef::Blob(b), _) => DataValue::Blob(b.to_vec()),
    })
}
