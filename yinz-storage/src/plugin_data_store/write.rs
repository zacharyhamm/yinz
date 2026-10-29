//! Applying mutations: the `INSERT`, `UPDATE` and `DELETE` half of the store.

use yinz_types::internal_ids::DataRevision;
use yinz_types::internal_ids::DataRowId;
use yinz_types::plugin_data::{
    ApplyReport, AttrKind, DataError, DataValue, FetchRequest, Mutation, REV_ABSENT,
};

use super::dialect::{Dialect, Params};
use super::executor::{query_first, Executor};
use super::predicate::compile_pred;
use super::query::run_count_with_search_fallback;
use super::schema::{EntitySchema, STORE_COLUMNS};
use super::sql::{quote_ident, StoreContext};

/// Check the values of a mutation against the schema, rejecting undeclared attributes, wrong kinds
/// and malformed JSON.
pub(super) fn check_values(
    schema: &EntitySchema,
    values: &[(String, DataValue)],
) -> Result<(), DataError> {
    for (attr, value) in values {
        let Some(declared) = schema.attr(attr) else {
            return Err(DataError::InvalidQuery(format!(
                "`{}` is not a declared attribute of `{}`{}",
                attr,
                schema.def.name,
                if STORE_COLUMNS.contains(&attr.as_str()) {
                    "; it is maintained by the store and cannot be written directly"
                } else {
                    ""
                }
            )));
        };
        if !value.fits(declared.kind) {
            return Err(DataError::InvalidQuery(format!(
                "attribute `{}` of `{}` is {:?} but is being set to {}",
                attr,
                schema.def.name,
                declared.kind,
                value.kind_name()
            )));
        }
        // A Json attribute is worth having as a distinct kind only if it is actually JSON when it
        // comes back out, so it is validated on the way in.
        if declared.kind == AttrKind::Json
            && let DataValue::Text(text) = value
        {
            serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                DataError::InvalidQuery(format!(
                    "attribute `{}` of `{}` is Json but was set to something that isn't: {}",
                    attr, schema.def.name, e
                ))
            })?;
        }
    }
    Ok(())
}

/// The statement for a put, which depends on what the caller is guarding on.
///
/// Unguarded, and with a uid, this is the upsert it has always been; `created_at` is deliberately
/// left out of the update, because the row is the same row and when it was first written is still
/// true. Guarded on [`REV_ABSENT`] it becomes `DO NOTHING`, so it inserts or reports back that
/// something was already there.
///
/// Guarded on a real revision it stops being an insert at all — see [`build_guarded_put`].
pub(super) fn build_put(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    uid: Option<&str>,
    values: &[(String, DataValue)],
    now: i64,
    expect_rev: Option<DataRevision>,
) -> (String, Vec<DataValue>) {
    let mut params = Params::new(dialect);
    let mut columns = vec![
        "uid".to_owned(),
        "created_at".to_owned(),
        "updated_at".to_owned(),
    ];
    let mut placeholders = vec![
        params.bind(
            uid.map(|u| DataValue::Text(u.to_owned()))
                .unwrap_or(DataValue::Null),
        ),
        params.bind(DataValue::Int(now)),
        params.bind(DataValue::Int(now)),
    ];
    for (attr, value) in values {
        columns.push(attr.clone());
        placeholders.push(params.bind(value.clone()));
    }
    let quoted = columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let conflict = if uid.is_some() && expect_rev == Some(REV_ABSENT) {
        // The caller says this row must not exist. Let the insert fall through rather than
        // updating, and the caller reads the absent `RETURNING` row as the conflict it is.
        " ON CONFLICT(\"uid\") DO NOTHING".to_owned()
    } else if uid.is_some() {
        let table_q = quote_ident(&schema.table());
        let assignments = values
            .iter()
            .map(|(attr, _)| {
                let col = quote_ident(attr);
                format!("{col} = excluded.{col}")
            })
            .chain(std::iter::once(
                "\"updated_at\" = excluded.\"updated_at\"".to_owned(),
            ))
            // Increment the stored revision, not the incoming row's initial revision.
            .chain(std::iter::once(format!("\"rev\" = {table_q}.\"rev\" + 1")))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" ON CONFLICT(\"uid\") DO UPDATE SET {}", assignments)
    } else {
        String::new()
    };
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}){} RETURNING \"id\"",
        quote_ident(&schema.table()),
        quoted,
        placeholders.join(", "),
        conflict
    );
    (sql, params.into_values())
}

/// The statement for a put guarded on a revision the row is expected to already be at.
///
/// An `UPDATE`, deliberately, and not the upsert an unguarded put compiles to. The caller read a
/// row at revision `expect_rev` and wants to write the next one; if that row is gone, somebody
/// deleted it after the read, and an upsert would put it back — losing their delete just as
/// surely as an unguarded write loses an edit. Refusing is the whole point of the guard, so the
/// statement is one that can only ever match a row that is still there.
///
/// `created_at` is untouched for the same reason it is in the upsert: the row is the same row.
pub(super) fn build_guarded_put(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    uid: &str,
    values: &[(String, DataValue)],
    now: i64,
    expect_rev: DataRevision,
) -> (String, Vec<DataValue>) {
    let mut params = Params::new(dialect);
    let mut assignments: Vec<String> = values
        .iter()
        .map(|(attr, value)| format!("{} = {}", quote_ident(attr), params.bind(value.clone())))
        .collect();
    assignments.push(format!(
        "\"updated_at\" = {}",
        params.bind(DataValue::Int(now))
    ));
    assignments.push("\"rev\" = \"rev\" + 1".to_owned());
    let uid_p = params.bind(DataValue::Text(uid.to_owned()));
    let rev_p = params.bind(DataValue::from(expect_rev));
    let sql = format!(
        "UPDATE {} SET {} WHERE \"uid\" = {} AND \"rev\" = {} RETURNING \"id\"",
        quote_ident(&schema.table()),
        assignments.join(", "),
        uid_p,
        rev_p
    );
    (sql, params.into_values())
}

pub(super) fn build_update(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    filter: &yinz_types::plugin_data::Pred,
    values: &[(String, DataValue)],
    now: i64,
    expect_rev: Option<DataRevision>,
) -> Result<(String, Vec<DataValue>), DataError> {
    let table = schema.table();
    let mut params = Params::new(dialect);
    let mut assignments: Vec<String> = Vec::new();
    for (attr, value) in values {
        let placeholder = params.bind(value.clone());
        assignments.push(format!("{} = {}", quote_ident(attr), placeholder));
    }
    let placeholder = params.bind(DataValue::Int(now));
    assignments.push(format!("\"updated_at\" = {}", placeholder));
    assignments.push("\"rev\" = \"rev\" + 1".to_owned());
    let mut where_sql = String::new();
    compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
    // Narrowing the caller's filter rather than replacing it: an update guarding on a revision
    // still means whatever its filter meant, and with a filter naming one row — the normal case —
    // the pair is a compare-and-set on that row.
    if let Some(rev) = expect_rev {
        let rev_p = params.bind(DataValue::from(rev));
        where_sql = format!(
            "({}) AND {}.\"rev\" = {}",
            where_sql,
            quote_ident(&table),
            rev_p
        );
    }
    let sql = format!(
        "UPDATE {} SET {} WHERE {}",
        quote_ident(&table),
        assignments.join(", "),
        where_sql
    );
    Ok((sql, params.into_values()))
}

pub(super) fn build_delete(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    filter: &yinz_types::plugin_data::Pred,
) -> Result<(String, Vec<DataValue>), DataError> {
    let table = schema.table();
    let mut params = Params::new(dialect);
    let mut where_sql = String::new();
    compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
    let sql = format!("DELETE FROM {} WHERE {}", quote_ident(&table), where_sql);
    Ok((sql, params.into_values()))
}

pub(super) fn apply_one(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    mutation: &Mutation,
    now: i64,
    report: &mut ApplyReport,
) -> Result<(), DataError> {
    let dialect = exec.dialect();
    let entity = &schema.def.name;
    match mutation {
        Mutation::AssertRows { filter, rows, .. } => {
            let table = schema.table();
            let mut params = Params::new(dialect);
            let mut predicate = String::new();
            compile_pred(filter, schema, &table, &mut predicate, &mut params)?;
            let sql = format!(
                "SELECT \"id\", \"rev\" FROM {} WHERE {}",
                quote_ident(&table),
                predicate
            );
            let mut expected: std::collections::BTreeMap<_, _> = rows.iter().copied().collect();
            let mismatch =
                || DataError::Conflict(format!("the rows of `{entity}` changed; reload and retry"));
            exec.query_rows(
                &sql,
                &params.into_values(),
                &[AttrKind::Int, AttrKind::Int],
                &mut |row| match row.as_slice() {
                    [DataValue::Int(id), DataValue::Int(rev)]
                        if expected.remove(&DataRowId::new(*id))
                            == Some(DataRevision::new(*rev)) =>
                    {
                        Ok(true)
                    },
                    _ => Err(mismatch()),
                },
            )?;
            if !expected.is_empty() {
                return Err(mismatch());
            }
            report.per_mutation.push(0);
        },
        Mutation::Put {
            uid,
            values,
            expect_rev,
            ..
        } => {
            check_values(schema, values)?;
            // A guard on a real revision is an update, not an insert. `validate` has already
            // refused an `expect_rev` without a uid, so the uid is there whenever one is.
            let (sql, params) = match (uid.as_deref(), *expect_rev) {
                (Some(uid), Some(rev)) if rev != REV_ABSENT => {
                    build_guarded_put(dialect, schema, uid, values, now, rev)
                },
                _ => build_put(dialect, schema, uid.as_deref(), values, now, *expect_rev),
            };
            // `query_first` rather than `query_i64`: a guarded write that matched nothing returns
            // no row at all, which is the conflict rather than a driver error.
            let row = query_first(exec, &sql, &params, &[AttrKind::Int])
                .with_context(|| format!("failed to write a row of `{}`", entity))?;
            match row.as_ref().and_then(|row| row.first()) {
                Some(DataValue::Int(id)) => {
                    report.rows_written += 1;
                    report.per_mutation.push(1);
                    report.ids.push(DataRowId::new(*id));
                },
                _ => {
                    conflict(expect_rev, entity)?;
                    // Unreachable: an unguarded put always writes a row, so a missing `RETURNING`
                    // with no guard means the driver lied about what it did.
                    return Err(DataError::Storage(format!(
                        "a put into `{}` reported no row and no conflict",
                        entity
                    )));
                },
            }
        },
        Mutation::Update {
            filter,
            values,
            expect_rev,
            ..
        } => {
            check_values(schema, values)?;
            let expected = expect_rev
                .map(|_| {
                    run_count_with_search_fallback(
                        exec,
                        schema,
                        &FetchRequest::entity(entity).filter(filter.clone()),
                    )
                })
                .transpose()?;
            let (sql, params) = build_update(dialect, schema, filter, values, now, *expect_rev)?;
            let changed = exec
                .execute(&sql, &params)
                .with_context(|| format!("failed to update rows of `{}`", entity))?;
            if changed == 0 || expected.is_some_and(|expected| expected != changed) {
                conflict(expect_rev, entity)?;
            }
            report.rows_written += changed;
            report.per_mutation.push(changed);
        },
        Mutation::Delete { filter, .. } => {
            let (sql, params) = build_delete(dialect, schema, filter)?;
            let changed = exec
                .execute(&sql, &params)
                .with_context(|| format!("failed to delete rows of `{}`", entity))?;
            report.rows_deleted += changed;
            report.per_mutation.push(changed);
        },
    }
    Ok(())
}

/// Report a guarded write that did not match every expected row. Unguarded writes cannot conflict.
fn conflict(expect_rev: &Option<DataRevision>, entity: &str) -> Result<(), DataError> {
    let Some(rev) = *expect_rev else {
        return Ok(());
    };
    Err(DataError::Conflict(if rev == REV_ABSENT {
        format!(
            "a row of `{}` this write expected not to exist is already there; re-read it and \
             decide again",
            entity
        )
    } else {
        format!(
            "a row of `{}` is missing or no longer at revision {}; something else wrote it first, so \
             re-read it and decide again",
            entity, rev
        )
    }))
}
