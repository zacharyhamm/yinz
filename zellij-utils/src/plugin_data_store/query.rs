//! Building and running fetches: the `SELECT` half of the store.

use rusqlite::Connection;

use crate::plugin_data::{AttrKind, DataError, DataRows, DataValue, FetchRequest, MAX_FETCH_BYTES};
use crate::session_recording::fts_phrase;

use super::predicate::compile_pred;
use super::schema::{EntitySchema, STORE_COLUMNS};
use super::sql::{qualify, quote_ident, SqlContext};
use super::value::read_value;

/// The columns a fetch returns, in order.
pub(super) fn output_columns(
    schema: &EntitySchema,
    request: &FetchRequest,
) -> Result<Vec<String>, DataError> {
    if request.select.is_empty() {
        let mut columns: Vec<String> = STORE_COLUMNS.iter().map(|c| (*c).to_owned()).collect();
        columns.extend(schema.def.attrs.iter().map(|a| a.name.clone()));
        return Ok(columns);
    }
    for column in &request.select {
        schema.require_column(column)?;
    }
    Ok(request.select.clone())
}

/// Whether the fetch is ordered by `id` ascending — the only ordering `after_id` can page through.
pub(super) fn is_default_ordering(request: &FetchRequest) -> bool {
    request.sort.is_empty() && request.search.is_none()
}

pub(super) struct FetchSql {
    sql: String,
    params: Vec<DataValue>,
    output_columns: Vec<String>,
}

pub(super) fn build_fetch_sql(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<FetchSql, DataError> {
    let table = schema.table();
    let columns = output_columns(schema, request)?;
    // `id` is always selected first, whether or not it was asked for, so that a truncated fetch
    // can hand back a cursor even when the plugin only wanted some other column.
    let mut selected = vec![qualify(&table, "id")];
    for column in &columns {
        selected.push(qualify(&table, column));
    }
    let (where_clause, params, join) = build_where(schema, request, search)?;
    let order = build_order(schema, request, search)?;
    let sql = format!(
        "SELECT {} FROM \"{}\"{} {} ORDER BY {} LIMIT ? OFFSET ?",
        selected.join(", "),
        table,
        join,
        where_clause,
        order
    );
    Ok(FetchSql {
        sql,
        params,
        output_columns: columns,
    })
}

pub(super) fn build_count_sql(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<(String, Vec<DataValue>), DataError> {
    let table = schema.table();
    let (where_clause, params, join) = build_where(schema, request, search)?;
    Ok((
        format!(
            "SELECT COUNT(*) FROM \"{}\"{} {}",
            table, join, where_clause
        ),
        params,
    ))
}

/// Build the `WHERE` clause, its parameters, and the FTS join it may need.
pub(super) fn build_where(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<(String, Vec<DataValue>, String), DataError> {
    let table = schema.table();
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<DataValue> = Vec::new();
    let mut join = String::new();

    if let Some(search) = search {
        if schema.def.full_text_attrs().is_empty() {
            return Err(DataError::InvalidQuery(format!(
                "`{}` has no full-text attributes to search; declare one with .full_text()",
                schema.def.name
            )));
        }
        let fts = schema.fts_table();
        join = format!(
            " JOIN \"{fts}\" ON \"{fts}\".rowid = \"{table}\".\"id\"",
            fts = fts,
            table = table
        );
        conditions.push(format!("{} MATCH ?", quote_ident(&fts)));
        params.push(DataValue::Text(search.to_owned()));
    }

    if let Some(filter) = &request.filter {
        let mut sql = String::new();
        compile_pred(filter, schema, &table, &mut sql, &mut params)?;
        conditions.push(sql);
    }

    if let Some(after_id) = request.after_id {
        if !is_default_ordering(request) {
            return Err(DataError::InvalidQuery(
                "after_id pages through the default id ordering, so it cannot be combined with a \
                 sort or a search; use offset for those"
                    .to_owned(),
            ));
        }
        conditions.push(format!("{} > ?", qualify(&table, "id")));
        params.push(DataValue::Int(after_id));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    Ok((where_clause, params, join))
}

pub(super) fn build_order(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<String, DataError> {
    let table = schema.table();
    let mut terms: Vec<String> = Vec::new();
    if request.sort.is_empty() {
        if search.is_some() {
            // bm25 returns a more negative score the better the match, so ascending is best-first.
            terms.push(format!("bm25(\"{}\") ASC", schema.fts_table()));
        }
    } else {
        for (attr, dir) in &request.sort {
            schema.require_column(attr)?;
            terms.push(format!("{} {}", qualify(&table, attr), dir.sql()));
        }
    }
    // Always break ties on `id`, so that paging through a set sorted on a column with duplicates
    // returns each row once instead of reshuffling between pages.
    terms.push(format!("{} ASC", qualify(&table, "id")));
    Ok(terms.join(", "))
}

/// Run a fetch, retrying a search that isn't valid FTS5 as a literal phrase.
///
/// Punctuation and unbalanced quotes are FTS5 syntax errors, and are also exactly what gets pasted
/// in from a log line — the same reason [`crate::session_recording`] does this. On a second failure
/// the *first* error is returned, since that is the one describing what the plugin actually asked.
pub(super) fn run_fetch_with_search_fallback(
    conn: &Connection,
    schema: &EntitySchema,
    request: &FetchRequest,
) -> Result<DataRows, DataError> {
    let first = run_fetch(conn, schema, request, request.search.as_deref());
    let (Err(first_error), Some(search)) = (&first, &request.search) else {
        return first;
    };
    match run_fetch(conn, schema, request, Some(&fts_phrase(search))) {
        Ok(rows) => Ok(rows),
        Err(_) => Err(first_error.clone()),
    }
}

pub(super) fn run_fetch(
    conn: &Connection,
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<DataRows, DataError> {
    let FetchSql {
        sql,
        mut params,
        output_columns,
    } = build_fetch_sql(schema, request, search)?;
    let limit = request.effective_limit();
    // One row past the limit, so that hitting it is distinguishable from reaching the end.
    params.push(DataValue::Int(limit as i64 + 1));
    params.push(DataValue::Int(request.offset as i64));

    let kinds: Vec<Option<AttrKind>> = output_columns
        .iter()
        .map(|c| schema.column_kind(c))
        .collect();

    let mut stmt = conn.prepare(&sql).ctx("failed to prepare the fetch")?;
    let mut sqlite_rows = stmt
        .query(rusqlite::params_from_iter(params.iter()))
        .ctx("failed to run the fetch")?;

    let mut rows: Vec<Vec<DataValue>> = Vec::new();
    let mut truncated = false;
    let mut last_id: Option<i64> = None;
    let mut bytes = 0usize;
    while let Some(row) = sqlite_rows.next().ctx("failed to read a row")? {
        if rows.len() as u32 >= limit {
            truncated = true;
            break;
        }
        let id: i64 = row.get(0).ctx("failed to read a row id")?;
        let mut values = Vec::with_capacity(output_columns.len());
        let mut row_bytes = 0usize;
        for (offset, kind) in kinds.iter().enumerate() {
            let value = read_value(row, offset + 1, *kind)?;
            row_bytes += value.encoded_size();
            values.push(value);
        }
        // Stop before exceeding the budget rather than after, but never return zero rows for it:
        // a single row over the budget still has to come back, or the plugin cannot make progress.
        if bytes + row_bytes > MAX_FETCH_BYTES && !rows.is_empty() {
            truncated = true;
            break;
        }
        bytes += row_bytes;
        last_id = Some(id);
        rows.push(values);
    }

    let next_cursor = if truncated && is_default_ordering(request) {
        last_id
    } else {
        None
    };
    Ok(DataRows {
        columns: output_columns,
        rows,
        truncated,
        next_cursor,
    })
}
