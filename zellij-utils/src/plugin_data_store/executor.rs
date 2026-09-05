//! The one seam through which the engine touches a database.
//!
//! Everything above this trait composes SQL strings and [`DataValue`] parameters; everything
//! below it belongs to a driver. A backend implements it for its connection and its transaction,
//! and the engine in [`super::engine`] is written once against `&mut dyn Executor`.

use crate::plugin_data::{AttrKind, DataError, DataValue};

use super::dialect::Dialect;

pub(super) trait Executor {
    fn dialect(&self) -> &'static dyn Dialect;

    /// Serialize schema changes with every other process sharing the database.
    ///
    /// Called at the start of a `define_entity` transaction. SQLite has nothing to do — its
    /// `BEGIN IMMEDIATE` already took the write lock — while Postgres takes an advisory lock so
    /// two servers cannot both decide an entity does not exist yet.
    fn lock_schema(&mut self) -> Result<(), DataError>;

    /// Run one or more statements that take no parameters. DDL, mostly.
    fn execute_batch(&mut self, sql: &str) -> Result<(), DataError>;

    /// Run a statement and report how many rows it changed.
    fn execute(&mut self, sql: &str, params: &[DataValue]) -> Result<u64, DataError>;

    /// Run a statement that yields exactly one row of one integer: a `RETURNING id`, a `COUNT(*)`.
    fn query_i64(&mut self, sql: &str, params: &[DataValue]) -> Result<i64, DataError>;

    /// Run a query, handing each row to `sink` as it is read. `kinds[i]` is the declared kind of
    /// the `i`-th selected column, which the driver uses to decode values the engine stores in a
    /// wider type (SQLite keeps booleans in an integer column).
    ///
    /// `sink` returns whether to keep reading, which is how a fetch stops at its row or byte
    /// budget without pulling the rest of the result set through the driver.
    fn query_rows(
        &mut self,
        sql: &str,
        params: &[DataValue],
        kinds: &[AttrKind],
        sink: &mut dyn FnMut(Vec<DataValue>) -> Result<bool, DataError>,
    ) -> Result<(), DataError>;
}

/// Every row of a query, for the small registry reads that never need a budget.
pub(super) fn query_all(
    exec: &mut dyn Executor,
    sql: &str,
    params: &[DataValue],
    kinds: &[AttrKind],
) -> Result<Vec<Vec<DataValue>>, DataError> {
    let mut rows = Vec::new();
    exec.query_rows(sql, params, kinds, &mut |row| {
        rows.push(row);
        Ok(true)
    })?;
    Ok(rows)
}

/// The first row of a query, if any.
pub(super) fn query_first(
    exec: &mut dyn Executor,
    sql: &str,
    params: &[DataValue],
    kinds: &[AttrKind],
) -> Result<Option<Vec<DataValue>>, DataError> {
    let mut first = None;
    exec.query_rows(sql, params, kinds, &mut |row| {
        first = Some(row);
        Ok(false)
    })?;
    Ok(first)
}
