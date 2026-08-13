//! The `store_entities` registry: the file it lives in, and the tables it names.
//!
//! An entity's table is `e_<row_id>`, named after its row here, so the table can only be created
//! once the row exists — and so no plugin-supplied string is ever interpolated into a table name.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::plugin_data::{AttrDef, DataError, DataSchemaInfo, EntityDef};

use super::index::{create_attr_indexes, rebuild_full_text};
use super::schema::EntitySchema;
use super::sql::{quote_ident, storage_err, SqlContext};
use super::value::sql_literal;

/// Open the store's file, set its pragmas, and make sure the registry table exists.
pub(super) fn open_db(db_path: &Path) -> Result<Connection, DataError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| storage_err(&format!("failed to create {:?}", parent), e))?;
    }
    let conn = Connection::open(db_path).with_ctx(|| format!("failed to open {:?}", db_path))?;
    // The same pragma set as the recording store. WAL and a busy timeout matter more here than
    // there: this file lives in the data dir rather than a session cache dir, so two zellij
    // servers running at once share it.
    conn.pragma_update(None, "journal_mode", "WAL")
        .ctx("failed to enable WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .ctx("failed to set busy_timeout")?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .ctx("failed to enable foreign keys")?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .ctx("failed to set synchronous")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(db_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(db_path, perms);
        }
    }
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS store_entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            namespace TEXT NOT NULL,
            entity TEXT NOT NULL,
            version INTEGER NOT NULL,
            definition TEXT NOT NULL,
            owner_plugin TEXT NOT NULL,
            UNIQUE(namespace, entity)
        );
        "#,
    )
    .ctx("failed to create the entity registry")?;
    Ok(conn)
}

pub(super) fn load_entity(
    conn: &Connection,
    namespace: &str,
    entity: &str,
) -> Result<Option<EntitySchema>, DataError> {
    let row = conn
        .query_row(
            "SELECT id, definition, owner_plugin FROM store_entities
             WHERE namespace = ?1 AND entity = ?2",
            rusqlite::params![namespace, entity],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .ctx("failed to read the entity registry")?;
    let Some((row_id, definition, owner_plugin)) = row else {
        return Ok(None);
    };
    let def: EntityDef = serde_json::from_str(&definition).map_err(|e| {
        storage_err(
            &format!("the stored definition of `{}` is unreadable", entity),
            e,
        )
    })?;
    Ok(Some(EntitySchema {
        row_id,
        def,
        owner_plugin,
    }))
}

pub(super) fn require_entity(
    conn: &Connection,
    namespace: &str,
    entity: &str,
) -> Result<EntitySchema, DataError> {
    load_entity(conn, namespace, entity)?.ok_or_else(|| {
        DataError::UnknownEntity(format!(
            "`{}` has not been declared in namespace `{}`; call define_entity first",
            entity, namespace
        ))
    })
}

pub(super) fn create_entity(
    conn: &Connection,
    namespace: &str,
    owner_plugin: &str,
    def: &EntityDef,
) -> Result<DataSchemaInfo, DataError> {
    let definition = serde_json::to_string(def)
        .map_err(|e| storage_err("failed to serialize the entity definition", e))?;
    conn.execute(
        "INSERT INTO store_entities (namespace, entity, version, definition, owner_plugin)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![namespace, def.name, def.version, definition, owner_plugin],
    )
    .ctx("failed to register the entity")?;
    // The table is named after the registry row, so it can only be created once the row exists.
    let schema = EntitySchema {
        row_id: conn.last_insert_rowid(),
        def: def.clone(),
        owner_plugin: owner_plugin.to_owned(),
    };
    let table = schema.table();
    let mut columns = vec![
        "id INTEGER PRIMARY KEY AUTOINCREMENT".to_owned(),
        "uid TEXT UNIQUE".to_owned(),
        "created_at INTEGER NOT NULL".to_owned(),
        "updated_at INTEGER NOT NULL".to_owned(),
    ];
    for attr in &def.attrs {
        columns.push(column_definition(attr)?);
    }
    conn.execute_batch(&format!(
        "CREATE TABLE \"{}\" ({});",
        table,
        columns.join(", ")
    ))
    .with_ctx(|| format!("failed to create the table for `{}`", def.name))?;
    // Always indexed, because every list, sync and paginate query orders or filters on them.
    conn.execute_batch(&format!(
        "CREATE INDEX \"{table}_created\" ON \"{table}\"(created_at);
         CREATE INDEX \"{table}_updated\" ON \"{table}\"(updated_at);"
    ))
    .ctx("failed to create the timestamp indexes")?;

    let mut indexes_created = Vec::new();
    for attr in &def.attrs {
        if create_attr_indexes(conn, &schema, attr)? {
            indexes_created.push(attr.name.clone());
        }
    }
    let full_text_rebuilt = rebuild_full_text(conn, &schema)?;
    Ok(DataSchemaInfo {
        namespace: namespace.to_owned(),
        entity: def.name.clone(),
        version: def.version,
        created: true,
        attrs_added: Vec::new(),
        indexes_created,
        full_text_rebuilt,
    })
}

pub(super) fn column_definition(attr: &AttrDef) -> Result<String, DataError> {
    let mut sql = format!("{} {}", quote_ident(&attr.name), attr.kind.sql_type());
    if attr.required {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = &attr.default {
        sql.push_str(&format!(" DEFAULT {}", sql_literal(default)?));
    }
    Ok(sql)
}
