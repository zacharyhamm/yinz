//! Applying mutations: the `INSERT`, `UPDATE` and `DELETE` half of the store.

use crate::plugin_data::{ApplyReport, AttrKind, DataError, DataValue, Mutation};

use super::dialect::{Dialect, Params};
use super::executor::Executor;
use super::predicate::compile_pred;
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
        if declared.kind == AttrKind::Json {
            if let DataValue::Text(text) = value {
                serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                    DataError::InvalidQuery(format!(
                        "attribute `{}` of `{}` is Json but was set to something that isn't: {}",
                        attr, schema.def.name, e
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// The `INSERT` for a put. With a uid this is an upsert; `created_at` is deliberately left out of
/// the update, because the row is the same row and when it was first written is still true.
pub(super) fn build_put(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    uid: Option<&str>,
    values: &[(String, DataValue)],
    now: i64,
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
    let conflict = if uid.is_some() {
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
            // Read off the stored row rather than `excluded`: the incoming row carries the
            // column's default, so `excluded."rev"` would reset every upsert to 1. Qualified
            // because in a `DO UPDATE` an unqualified name is the *target* row's on Postgres but
            // reads better said outright, and both engines accept the qualified form.
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

pub(super) fn build_update(
    dialect: &dyn Dialect,
    schema: &EntitySchema,
    filter: &crate::plugin_data::Pred,
    values: &[(String, DataValue)],
    now: i64,
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
    filter: &crate::plugin_data::Pred,
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
    match mutation {
        Mutation::Put { uid, values, .. } => {
            check_values(schema, values)?;
            let (sql, params) = build_put(dialect, schema, uid.as_deref(), values, now);
            let id = exec
                .query_i64(&sql, &params)
                .with_context(|| format!("failed to write a row of `{}`", schema.def.name))?;
            report.rows_written += 1;
            report.ids.push(id);
        },
        Mutation::Update { filter, values, .. } => {
            check_values(schema, values)?;
            let (sql, params) = build_update(dialect, schema, filter, values, now)?;
            let changed = exec
                .execute(&sql, &params)
                .with_context(|| format!("failed to update rows of `{}`", schema.def.name))?;
            report.rows_written += changed;
        },
        Mutation::Delete { filter, .. } => {
            let (sql, params) = build_delete(dialect, schema, filter)?;
            let changed = exec
                .execute(&sql, &params)
                .with_context(|| format!("failed to delete rows of `{}`", schema.def.name))?;
            report.rows_deleted += changed;
        },
    }
    Ok(())
}
