//! Carrying values across the `postgres` boundary.

use bytes::BytesMut;
use tokio_postgres::types::{to_sql_checked, IsNull, ToSql, Type};

use crate::plugin_data_store::sql::StorageError;
use crate::plugin_data_store::value::SqlValue;
use yinz_types::plugin_data::{DataError, DataValue};

impl StorageError for tokio_postgres::Error {
    fn into_data_error(self, context: &str) -> DataError {
        DataError::Storage(format!("{context}: {:#}", anyhow::Error::new(self)))
    }
}

impl ToSql for SqlValue<'_> {
    /// Encode for the column's actual type. That is what lets an `Int` go into a `DOUBLE
    /// PRECISION` column: [`DataValue::fits`] allows it so that a plugin writing `0` to a float
    /// attribute need not write `0.0`, and Postgres, unlike SQLite, does not coerce at bind time.
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self.0 {
            DataValue::Null => Ok(IsNull::Yes),
            DataValue::Int(i) if *ty == Type::FLOAT8 => (*i as f64).to_sql(ty, out),
            DataValue::Int(i) => i.to_sql(ty, out),
            DataValue::Real(f) => f.to_sql(ty, out),
            DataValue::Text(s) => s.as_str().to_sql(ty, out),
            DataValue::Bool(b) => b.to_sql(ty, out),
            DataValue::Blob(b) => b.as_slice().to_sql(ty, out),
        }
    }

    fn accepts(ty: &Type) -> bool {
        matches!(
            *ty,
            Type::INT8 | Type::FLOAT8 | Type::TEXT | Type::VARCHAR | Type::BOOL | Type::BYTEA
        )
    }

    to_sql_checked!();
}

/// Read one column by the type Postgres reports for it. The declared kind is not needed here:
/// every kind has a column type of its own, so nothing comes back ambiguous.
pub(super) fn read_value(row: &tokio_postgres::Row, index: usize) -> Result<DataValue, DataError> {
    let ty = row.columns()[index].type_().clone();
    let read_err =
        |e: tokio_postgres::Error| e.into_data_error(&format!("failed to read a {} value", ty));
    Ok(match ty {
        Type::INT8 => row
            .try_get::<_, Option<i64>>(index)
            .map_err(read_err)?
            .map_or(DataValue::Null, DataValue::Int),
        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(index)
            .map_err(read_err)?
            .map_or(DataValue::Null, DataValue::Real),
        Type::BOOL => row
            .try_get::<_, Option<bool>>(index)
            .map_err(read_err)?
            .map_or(DataValue::Null, DataValue::Bool),
        Type::TEXT | Type::VARCHAR => row
            .try_get::<_, Option<String>>(index)
            .map_err(read_err)?
            .map_or(DataValue::Null, DataValue::Text),
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(index)
            .map_err(read_err)?
            .map_or(DataValue::Null, DataValue::Blob),
        other => {
            return Err(DataError::Storage(format!(
                "the store holds a {} column, which it never creates",
                other
            )))
        },
    })
}
