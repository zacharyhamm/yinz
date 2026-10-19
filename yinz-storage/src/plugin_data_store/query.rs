//! Building and running fetches: the `SELECT` half of the store.

use yinz_types::internal_ids::DataRowId;
use yinz_types::plugin_data::{
    AttrKind, DataError, DataRows, DataValue, FetchRequest, SortDir, MAX_FETCH_BYTES,
};

use super::dialect::{Dialect, Params};
use super::executor::Executor;
use super::predicate::compile_pred;
use super::schema::{EntitySchema, STORE_COLUMNS};
use super::sql::{qualify, quote_ident, StoreContext};

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

/// The search term as it is bound. NUL is stripped rather than rejected: a search is something a
/// person pastes, and NUL is not part of a text search.
pub(super) fn search_term(request: &FetchRequest) -> Option<String> {
    request.search.as_ref().map(|s| s.replace('\0', ""))
}

pub(super) struct FetchSql {
    pub(super) sql: String,
    pub(super) params: Vec<DataValue>,
    pub(super) output_columns: Vec<String>,
}

/// The `WHERE` clause of a fetch, the join it may need, its parameters, and the ranking term a
/// search contributes to the ordering.
pub(super) struct WhereSql<'d> {
    pub(super) clause: String,
    pub(super) join: String,
    pub(super) params: Params<'d>,
    pub(super) rank: Option<String>,
}

pub(super) fn build_fetch_sql(
    dialect: &dyn Dialect,
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
    let WhereSql {
        clause,
        join,
        mut params,
        rank,
    } = build_where(dialect, schema, request, search)?;
    let order = build_order(schema, request, rank.as_deref())?;
    // One row past the limit, so that hitting it is distinguishable from reaching the end.
    let limit = params.bind(DataValue::Int(request.effective_limit() as i64 + 1));
    let offset = params.bind(DataValue::Int(request.offset as i64));
    let sql = format!(
        "SELECT {} FROM {}{} {} ORDER BY {} LIMIT {} OFFSET {}",
        selected.join(", "),
        quote_ident(&table),
        join,
        clause,
        order,
        limit,
        offset
    );
    Ok(FetchSql {
        sql,
        params: params.into_values(),
        output_columns: columns,
    })
}

pub(super) fn build_count_sql(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<(String, Vec<DataValue>), DataError> {
    let mut request = request.clone();
    request.after_id = None;
    let WhereSql {
        clause,
        join,
        params,
        ..
    } = build_where(dialect, schema, &request, search)?;
    Ok((
        format!(
            "SELECT COUNT(*) FROM {}{} {}",
            quote_ident(&schema.table()),
            join,
            clause
        ),
        params.into_values(),
    ))
}

pub(super) fn build_where<'d>(
    dialect: &'d dyn Dialect,
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<WhereSql<'d>, DataError> {
    let table = schema.table();
    let mut conditions: Vec<String> = Vec::new();
    let mut params = Params::new(dialect);
    let mut join = String::new();
    let mut rank = None;

    if let Some(search) = search {
        if schema.def.full_text_attrs().is_empty() {
            return Err(DataError::InvalidQuery(format!(
                "`{}` has no full-text attributes to search; declare one with .full_text()",
                schema.def.name
            )));
        }
        let placeholder = params.bind(DataValue::Text(search.to_owned()));
        let pieces = dialect.search(schema, &placeholder);
        join = pieces.join;
        conditions.push(pieces.condition);
        rank = Some(pieces.rank);
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
        let placeholder = params.bind(DataValue::from(after_id));
        conditions.push(format!("{} > {}", qualify(&table, "id"), placeholder));
    }

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    Ok(WhereSql {
        clause,
        join,
        params,
        rank,
    })
}

/// The `ORDER BY` terms: the request's sort, or the search ranking when there is one, and always
/// `id` last.
///
/// Explicit null placement keeps pagination ordering consistent across backends.
pub(super) fn build_order(
    schema: &EntitySchema,
    request: &FetchRequest,
    rank: Option<&str>,
) -> Result<String, DataError> {
    let table = schema.table();
    let mut terms: Vec<String> = Vec::new();
    if request.sort.is_empty() {
        if let Some(rank) = rank {
            terms.push(rank.to_owned());
        }
    } else {
        for (attr, dir) in &request.sort {
            schema.require_column(attr)?;
            terms.push(order_term(&qualify(&table, attr), *dir));
        }
    }
    // Always break ties on `id`, so that paging through a set sorted on a column with duplicates
    // returns each row once instead of reshuffling between pages.
    terms.push(format!("{} ASC", qualify(&table, "id")));
    Ok(terms.join(", "))
}

pub(super) fn order_term(column: &str, dir: SortDir) -> String {
    match dir {
        SortDir::Asc => format!("{} ASC NULLS FIRST", column),
        SortDir::Desc => format!("{} DESC NULLS LAST", column),
    }
}

/// Run a search, retrying one the engine rejects as bad syntax as a literal phrase.
///
/// Punctuation and unbalanced quotes are FTS5 syntax errors, and are also exactly what gets pasted
/// in from a log line. On a second failure
/// the *first* error is returned, since that is the one describing what the plugin actually asked.
/// An engine whose search syntax accepts anything gets the first result back untouched.
pub(super) fn with_search_fallback<T>(
    dialect: &dyn Dialect,
    request: &FetchRequest,
    mut run: impl FnMut(Option<&str>) -> Result<T, DataError>,
) -> Result<T, DataError> {
    let search = search_term(request);
    let first = run(search.as_deref());
    let (Err(first_error), Some(search)) = (&first, &search) else {
        return first;
    };
    if !dialect.retries_search_as_phrase() {
        return first;
    }
    match run(Some(&fts_phrase(search))) {
        Ok(value) => Ok(value),
        Err(_) => Err(first_error.clone()),
    }
}

pub(super) fn run_fetch_with_search_fallback(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    request: &FetchRequest,
) -> Result<DataRows, DataError> {
    let dialect = exec.dialect();
    with_search_fallback(dialect, request, |search| {
        run_fetch(exec, schema, request, search)
    })
}

pub(super) fn run_count_with_search_fallback(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    request: &FetchRequest,
) -> Result<u64, DataError> {
    let dialect = exec.dialect();
    with_search_fallback(dialect, request, |search| {
        let (sql, params) = build_count_sql(dialect, schema, request, search)?;
        let count = exec
            .query_i64(&sql, &params)
            .context("failed to count rows")?;
        Ok(count.max(0) as u64)
    })
}

pub(super) fn run_fetch(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<DataRows, DataError> {
    let FetchSql {
        sql,
        params,
        output_columns,
    } = build_fetch_sql(exec.dialect(), schema, request, search)?;
    let limit = request.effective_limit();

    // The leading `id`, then the selected columns.
    let mut kinds: Vec<AttrKind> = Vec::with_capacity(output_columns.len() + 1);
    kinds.push(AttrKind::Int);
    kinds.extend(output_columns.iter().map(|c| schema.any_column_kind(c)));

    let mut rows: Vec<Vec<DataValue>> = Vec::new();
    let mut truncated = false;
    let mut last_id: Option<DataRowId> = None;
    let mut bytes = 0usize;
    exec.query_rows(&sql, &params, &kinds, &mut |mut values| {
        if rows.len() as u32 >= limit {
            truncated = true;
            return Ok(false);
        }
        let id = match values.first() {
            Some(DataValue::Int(id)) => DataRowId::new(*id),
            other => {
                return Err(DataError::Storage(format!(
                    "failed to read a row id: got {:?}",
                    other
                )))
            },
        };
        values.remove(0);
        let row_bytes: usize = values.iter().map(DataValue::encoded_size).sum();
        // Stop before exceeding the budget rather than after, but never return zero rows for it:
        // a single row over the budget still has to come back, or the plugin cannot make progress.
        if bytes + row_bytes > MAX_FETCH_BYTES && !rows.is_empty() {
            truncated = true;
            return Ok(false);
        }
        bytes += row_bytes;
        last_id = Some(id);
        rows.push(values);
        Ok(true)
    })
    .context("failed to run the fetch")?;

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

fn fts_phrase(query: &str) -> String {
    let without_nul = query.replace('\0', "");
    format!("\"{}\"", without_nul.replace('"', "\"\""))
}
