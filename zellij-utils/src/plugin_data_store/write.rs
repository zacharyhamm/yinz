//! Applying mutations: the `INSERT`, `UPDATE` and `DELETE` half of the store.

use rusqlite::Connection;

use crate::plugin_data::{ApplyReport, AttrKind, DataError, DataValue, Mutation};

use super::predicate::compile_pred;
use super::schema::{EntitySchema, STORE_COLUMNS};
use super::sql::storage_err;

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

pub(super) fn apply_one(
    conn: &Connection,
    schema: &EntitySchema,
    mutation: &Mutation,
    now: i64,
    report: &mut ApplyReport,
) -> Result<(), DataError> {
    let table = schema.table();
    match mutation {
        Mutation::Put { uid, values, .. } => {
            check_values(schema, values)?;
            let mut columns = vec![
                "uid".to_owned(),
                "created_at".to_owned(),
                "updated_at".to_owned(),
            ];
            let mut params: Vec<DataValue> = vec![
                uid.clone().map(DataValue::Text).unwrap_or(DataValue::Null),
                DataValue::Int(now),
                DataValue::Int(now),
            ];
            for (attr, value) in values {
                columns.push(attr.clone());
                params.push(value.clone());
            }
            let quoted = columns
                .iter()
                .map(|c| format!("\"{}\"", c))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; columns.len()].join(", ");
            // With a uid this is an upsert. `created_at` is deliberately left out of the update:
            // the row is the same row, and when it was first written is still true.
            let conflict = if uid.is_some() {
                let assignments = values
                    .iter()
                    .map(|(attr, _)| format!("\"{attr}\" = excluded.\"{attr}\""))
                    .chain(std::iter::once(
                        "\"updated_at\" = excluded.\"updated_at\"".to_owned(),
                    ))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" ON CONFLICT(\"uid\") DO UPDATE SET {}", assignments)
            } else {
                String::new()
            };
            let sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES ({}){} RETURNING \"id\"",
                table, quoted, placeholders, conflict
            );
            let id: i64 = conn
                .query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
                    row.get(0)
                })
                .map_err(|e| {
                    storage_err(
                        &format!("failed to write a row of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_written += 1;
            report.ids.push(id);
        },
        Mutation::Update { filter, values, .. } => {
            check_values(schema, values)?;
            let mut params: Vec<DataValue> = Vec::new();
            let mut assignments: Vec<String> = Vec::new();
            for (attr, value) in values {
                assignments.push(format!("\"{}\" = ?", attr));
                params.push(value.clone());
            }
            assignments.push("\"updated_at\" = ?".to_owned());
            params.push(DataValue::Int(now));
            let mut where_sql = String::new();
            compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
            let sql = format!(
                "UPDATE \"{}\" SET {} WHERE {}",
                table,
                assignments.join(", "),
                where_sql
            );
            let changed = conn
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(|e| {
                    storage_err(
                        &format!("failed to update rows of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_written += changed as u64;
        },
        Mutation::Delete { filter, .. } => {
            let mut params: Vec<DataValue> = Vec::new();
            let mut where_sql = String::new();
            compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
            let sql = format!("DELETE FROM \"{}\" WHERE {}", table, where_sql);
            let changed = conn
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(|e| {
                    storage_err(
                        &format!("failed to delete rows of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_deleted += changed as u64;
        },
    }
    Ok(())
}
