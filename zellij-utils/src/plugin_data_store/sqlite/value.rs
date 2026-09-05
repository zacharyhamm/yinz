//! Carrying values across the `rusqlite` boundary.

use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::ToSql;

use crate::plugin_data::{AttrKind, DataError, DataValue};
use crate::plugin_data_store::sql::{SqlContext, StorageError};

impl StorageError for rusqlite::Error {}

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

/// Read one column, guided by its declared kind where the storage class is ambiguous.
pub(super) fn read_value(
    row: &rusqlite::Row<'_>,
    index: usize,
    kind: AttrKind,
) -> Result<DataValue, DataError> {
    let value = row.get_ref(index).ctx("failed to read a value")?;
    Ok(match (value, kind) {
        (ValueRef::Null, _) => DataValue::Null,
        // Only a column declared Bool comes back as one; SQLite stores both in an INTEGER.
        (ValueRef::Integer(i), AttrKind::Bool) => DataValue::Bool(i != 0),
        // REAL affinity converts integers on the way in, so this arm is belt and braces: a
        // Real attribute reads back as a Real whatever SQLite chose to keep on disk.
        (ValueRef::Integer(i), AttrKind::Real) => DataValue::Real(i as f64),
        (ValueRef::Integer(i), _) => DataValue::Int(i),
        (ValueRef::Real(f), _) => DataValue::Real(f),
        (ValueRef::Text(t), _) => DataValue::Text(String::from_utf8_lossy(t).into_owned()),
        (ValueRef::Blob(b), _) => DataValue::Blob(b.to_vec()),
    })
}
