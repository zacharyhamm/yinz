//! The secondary indexes and the FTS5 full-text index that back an entity's table.

use rusqlite::Connection;

use crate::plugin_data::{AttrDef, DataError};

use super::schema::EntitySchema;
use super::sql::{join_idents, quote_ident, SqlContext};

/// Create whatever indexes `attr` declares. Returns whether anything was created.
pub(super) fn create_attr_indexes(
    conn: &Connection,
    schema: &EntitySchema,
    attr: &AttrDef,
) -> Result<bool, DataError> {
    let table = quote_ident(&schema.table());
    let column = quote_ident(&attr.name);
    let create = |unique: &str, name: String| {
        conn.execute_batch(&format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON {table}({column});",
            quote_ident(&name)
        ))
    };

    let mut created = false;
    if attr.indexed {
        create("", schema.attr_index(&attr.name))
            .with_ctx(|| format!("failed to index `{}`", attr.name))?;
        created = true;
    }
    if attr.unique {
        create("UNIQUE ", schema.unique_index(&attr.name)).with_ctx(|| {
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
    conn: &Connection,
    schema: &EntitySchema,
    attr: &str,
) -> Result<(), DataError> {
    conn.execute_batch(&format!(
        "DROP INDEX IF EXISTS {}; DROP INDEX IF EXISTS {};",
        quote_ident(&schema.attr_index(attr)),
        quote_ident(&schema.unique_index(attr))
    ))
    .with_ctx(|| format!("failed to drop the indexes on `{}`", attr))
}

/// Remove the entity's full-text table and the triggers that maintain it.
///
/// Has to run *before* any `ALTER TABLE ... DROP COLUMN`: SQLite revalidates a table's triggers
/// when a column goes away, and a trigger still naming the dropped column fails the whole
/// statement.
pub(super) fn drop_full_text(conn: &Connection, schema: &EntitySchema) -> Result<(), DataError> {
    let [ai, ad, au] = schema.fts_triggers().map(|t| quote_ident(&t));
    let fts = quote_ident(&schema.fts_table());
    conn.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS {ai};
         DROP TRIGGER IF EXISTS {ad};
         DROP TRIGGER IF EXISTS {au};
         DROP TABLE IF EXISTS {fts};"
    ))
    .ctx("failed to clear the full-text index")
}

/// Drop and recreate the entity's full-text index and its triggers, then backfill it.
///
/// Returns whether the entity has a full-text index at all.
pub(super) fn rebuild_full_text(
    conn: &Connection,
    schema: &EntitySchema,
) -> Result<bool, DataError> {
    let table = schema.table();
    let fts = schema.fts_table();
    drop_full_text(conn, schema)?;

    let ft_attrs: Vec<&str> = schema
        .def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    if ft_attrs.is_empty() {
        return Ok(false);
    }
    // The same column list three times over: bare for the table and the trigger targets, and
    // behind `new.`/`old.` for the values the triggers read out of the row.
    let quoted = join_idents("", &ft_attrs);
    let new_values = join_idents("new.", &ft_attrs);
    let old_values = join_idents("old.", &ft_attrs);
    let [ai, ad, au] = schema.fts_triggers().map(|t| quote_ident(&t));
    let fts_q = quote_ident(&fts);
    let table_q = quote_ident(&table);
    // External content: the FTS table holds only the index and reads the columns back from the
    // entity table, so the text is not stored twice. The triggers are what keep it exact.
    //
    // `content=` takes the table name as a string literal rather than an identifier, so that one
    // stays unquoted.
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {fts_q} USING fts5(
             {quoted}, content='{table}', content_rowid='id', tokenize='unicode61'
         );
         CREATE TRIGGER {ai} AFTER INSERT ON {table_q} BEGIN
             INSERT INTO {fts_q}(rowid, {quoted}) VALUES (new.id, {new_values});
         END;
         CREATE TRIGGER {ad} AFTER DELETE ON {table_q} BEGIN
             INSERT INTO {fts_q}({fts_q}, rowid, {quoted})
                 VALUES('delete', old.id, {old_values});
         END;
         CREATE TRIGGER {au} AFTER UPDATE ON {table_q} BEGIN
             INSERT INTO {fts_q}({fts_q}, rowid, {quoted})
                 VALUES('delete', old.id, {old_values});
             INSERT INTO {fts_q}(rowid, {quoted}) VALUES (new.id, {new_values});
         END;
         INSERT INTO {fts_q}({fts_q}) VALUES('rebuild');"
    ))
    .ctx("failed to build the full-text index")?;
    Ok(true)
}
