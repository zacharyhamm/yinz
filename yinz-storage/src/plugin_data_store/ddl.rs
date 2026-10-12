//! The DDL behind an entity: its table, its secondary indexes, and its full-text index.
//!
//! Tables and indexes use standard SQL; full-text index syntax comes
//! from the dialect.

use yinz_types::plugin_data::{AttrDef, DataError};

use super::executor::Executor;
use super::registry::column_definition;
use super::schema::EntitySchema;
use super::sql::{quote_ident, StoreContext};

/// Create the entity's table with the store's own columns and one per attribute, plus the
/// timestamp indexes every entity has because every list, sync and paginate query orders or
/// filters on them.
pub(super) fn create_table(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
) -> Result<(), DataError> {
    let dialect = exec.dialect();
    let mut columns: Vec<String> = dialect.store_columns().to_vec();
    for attr in &schema.def.attrs {
        columns.push(column_definition(dialect, attr)?);
    }
    let table_q = quote_ident(&schema.table());
    exec.execute_batch(&format!("CREATE TABLE {table_q} ({});", columns.join(", ")))
        .with_context(|| format!("failed to create the table for `{}`", schema.def.name))?;
    let [created_idx, updated_idx] = schema.timestamp_indexes().map(|i| quote_ident(&i));
    exec.execute_batch(&format!(
        "CREATE INDEX {created_idx} ON {table_q}(created_at);
         CREATE INDEX {updated_idx} ON {table_q}(updated_at);"
    ))
    .context("failed to create the timestamp indexes")
}

/// Create whatever indexes `attr` declares. Returns whether anything was created.
pub(super) fn create_attr_indexes(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    attr: &AttrDef,
) -> Result<bool, DataError> {
    let table = quote_ident(&schema.table());
    let column = quote_ident(&attr.name);
    let create = |exec: &mut dyn Executor, unique: &str, name: String| {
        exec.execute_batch(&format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON {table}({column});",
            quote_ident(&exec.dialect().index_name(&name))
        ))
    };

    let mut created = false;
    if attr.indexed {
        create(exec, "", schema.attr_index(&attr.name))
            .with_context(|| format!("failed to index `{}`", attr.name))?;
        created = true;
    }
    if attr.unique {
        create(exec, "UNIQUE ", schema.unique_index(&attr.name)).with_context(|| {
            format!(
                "failed to make `{}` unique (existing rows may already hold duplicates)",
                attr.name
            )
        })?;
        created = true;
    }
    Ok(created)
}

pub(super) fn drop_attr_indexes(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
    attr: &str,
) -> Result<(), DataError> {
    exec.execute_batch(&format!(
        "DROP INDEX IF EXISTS {}; DROP INDEX IF EXISTS {};",
        quote_ident(&exec.dialect().index_name(&schema.attr_index(attr))),
        quote_ident(&exec.dialect().index_name(&schema.unique_index(attr)))
    ))
    .with_context(|| format!("failed to drop the indexes on `{}`", attr))
}

/// Remove the entity's full-text index, whether or not it has one.
///
/// Run before dropping a column because SQLite revalidates the full-text triggers.
pub(super) fn drop_full_text(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
) -> Result<(), DataError> {
    let sql = exec.dialect().full_text_drop(schema);
    exec.execute_batch(&sql)
        .context("failed to clear the full-text index")
}

/// Drop and recreate the entity's full-text index, then backfill it.
///
/// Returns whether the entity has a full-text index at all.
pub(super) fn rebuild_full_text(
    exec: &mut dyn Executor,
    schema: &EntitySchema,
) -> Result<bool, DataError> {
    drop_full_text(exec, schema)?;
    let ft_attrs: Vec<&str> = schema
        .def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    if ft_attrs.is_empty() {
        return Ok(false);
    }
    let sql = exec.dialect().full_text_create(schema, &ft_attrs);
    exec.execute_batch(&sql)
        .context("failed to build the full-text index")?;
    Ok(true)
}
