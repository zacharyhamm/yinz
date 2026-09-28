//! Compiling a [`Pred`] into a SQL fragment and the parameters bound to it.

use yinz_types::plugin_data::{AttrKind, DataError, DataValue, Pred};

use super::dialect::Params;
use super::schema::EntitySchema;
use super::sql::qualify;

/// Compile a predicate into a SQL fragment plus its bound parameters.
pub(super) fn compile_pred(
    pred: &Pred,
    schema: &EntitySchema,
    table: &str,
    sql: &mut String,
    params: &mut Params<'_>,
) -> Result<(), DataError> {
    /// `attr op ?`, with the value's kind checked against the column's.
    fn binary(
        schema: &EntitySchema,
        table: &str,
        attr: &str,
        op: &str,
        value: &DataValue,
        sql: &mut String,
        params: &mut Params<'_>,
    ) -> Result<(), DataError> {
        schema.require_column(attr)?;
        check_value_kind(schema, attr, value)?;
        let placeholder = params.bind(value.clone());
        sql.push_str(&format!("{} {} {}", qualify(table, attr), op, placeholder));
        Ok(())
    }

    /// A pattern match with the pattern's wildcards escaped, so a literal `%` in the needle stays
    /// literal.
    fn like(
        schema: &EntitySchema,
        table: &str,
        attr: &str,
        pattern: String,
        sql: &mut String,
        params: &mut Params<'_>,
    ) -> Result<(), DataError> {
        schema.require_column(attr)?;
        require_text_column(schema, attr)?;
        let placeholder = params.bind(DataValue::Text(pattern));
        sql.push_str(&params.dialect().like(&qualify(table, attr), &placeholder));
        Ok(())
    }

    match pred {
        Pred::Eq(a, v) => binary(schema, table, a, "=", v, sql, params)?,
        Pred::Ne(a, v) => binary(schema, table, a, "!=", v, sql, params)?,
        Pred::Gt(a, v) => binary(schema, table, a, ">", v, sql, params)?,
        Pred::Gte(a, v) => binary(schema, table, a, ">=", v, sql, params)?,
        Pred::Lt(a, v) => binary(schema, table, a, "<", v, sql, params)?,
        Pred::Lte(a, v) => binary(schema, table, a, "<=", v, sql, params)?,
        Pred::In(a, values) => {
            schema.require_column(a)?;
            for value in values {
                check_value_kind(schema, a, value)?;
            }
            let placeholders = values
                .iter()
                .map(|value| params.bind(value.clone()))
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!("{} IN ({})", qualify(table, a), placeholders));
        },
        Pred::IsNull(a) => {
            schema.require_column(a)?;
            sql.push_str(&format!("{} IS NULL", qualify(table, a)));
        },
        Pred::NotNull(a) => {
            schema.require_column(a)?;
            sql.push_str(&format!("{} IS NOT NULL", qualify(table, a)));
        },
        Pred::Prefix(a, v) => like(
            schema,
            table,
            a,
            format!("{}%", escape_like(v)),
            sql,
            params,
        )?,
        Pred::Contains(a, v) => like(
            schema,
            table,
            a,
            format!("%{}%", escape_like(v)),
            sql,
            params,
        )?,
        Pred::And(preds) | Pred::Or(preds) => {
            let joiner = if matches!(pred, Pred::And(_)) {
                " AND "
            } else {
                " OR "
            };
            sql.push('(');
            for (i, inner) in preds.iter().enumerate() {
                if i > 0 {
                    sql.push_str(joiner);
                }
                compile_pred(inner, schema, table, sql, params)?;
            }
            sql.push(')');
        },
        Pred::Not(inner) => {
            sql.push_str("NOT (");
            compile_pred(inner, schema, table, sql, params)?;
            sql.push(')');
        },
    }
    Ok(())
}

/// Escape the wildcards `LIKE` would otherwise interpret.
///
/// Backslash is the escape character on both engines: SQLite says so in the `ESCAPE` clause the
/// dialect adds, and it is Postgres's default.
pub(super) fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Reject a `Prefix` or `Contains` against a column that holds no text.
///
/// SQLite would coerce the value to its text form and match against that, so `Prefix("count", "1")`
/// would quietly match 1, 10 and 100. Every other predicate checks the kind it is comparing
/// against; leaving these two out made them the one place a type mistake produced results instead
/// of an error.
pub(super) fn require_text_column(schema: &EntitySchema, attr: &str) -> Result<(), DataError> {
    let kind = schema.any_column_kind(attr);
    if matches!(kind, AttrKind::Text | AttrKind::Json) {
        return Ok(());
    }
    Err(DataError::InvalidQuery(format!(
        "`{}` of `{}` is {:?}, which holds no text to match a prefix or substring against",
        attr, schema.def.name, kind
    )))
}

/// Reject comparing a column against a value of the wrong kind.
///
/// SQLite would happily compare a string to an integer column and simply never match, which reads
/// as "no results" rather than as the mistake it is.
pub(super) fn check_value_kind(
    schema: &EntitySchema,
    attr: &str,
    value: &DataValue,
) -> Result<(), DataError> {
    let kind = schema.any_column_kind(attr);
    if value.fits(kind) {
        return Ok(());
    }
    // A declared attribute is named as one; the store's own columns are not, since a plugin did
    // not declare them and would not recognise the word.
    let noun = if schema.column_kind(attr).is_some() {
        "attribute "
    } else {
        ""
    };
    Err(DataError::InvalidQuery(format!(
        "{}`{}` is {:?} but it is compared against {}",
        noun,
        attr,
        kind,
        value.kind_name()
    )))
}
