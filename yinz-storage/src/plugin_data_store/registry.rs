//! The `store_entities` registry: the rows that name every entity table.
//!
//! An entity's table is `e_<row_id>`, named after its row here, so the table can only be created
//! once the row exists — and so no plugin-supplied string is ever interpolated into a table name.
//! Each backend creates the registry table itself when it opens its database; what is here is
//! everything that reads and writes it afterwards, which the engines share.

use yinz_types::plugin_data::{
    AttrDef, AttrKind, DataEntityInfo, DataError, DataSchemaInfo, DataValue, EntityDef,
};

use super::ddl::{create_attr_indexes, create_table, rebuild_full_text};
use super::dialect::{Dialect, Params};
use super::executor::{query_all, query_first, Executor};
use super::schema::EntitySchema;
use super::sql::{storage_err, StoreContext};

pub(super) fn load_entity(
    exec: &mut dyn Executor,
    namespace: &str,
    entity: &str,
) -> Result<Option<EntitySchema>, DataError> {
    let mut params = Params::new(exec.dialect());
    let ns = params.bind(DataValue::Text(namespace.to_owned()));
    let name = params.bind(DataValue::Text(entity.to_owned()));
    let sql = format!(
        "SELECT id, definition, owner_plugin FROM store_entities WHERE namespace = {} AND entity = {}",
        ns, name
    );
    let row = query_first(
        exec,
        &sql,
        &params.into_values(),
        &[AttrKind::Int, AttrKind::Text, AttrKind::Text],
    )
    .context("failed to read the entity registry")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (row_id, definition, owner_plugin) = registry_row(row)?;
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

/// Every entity in the registry, or only those of one namespace.
///
/// Skips a row whose stored definition will not parse rather than failing the whole listing: a
/// catalog is what a plugin reaches for when something is already wrong, and one unreadable row
/// should not hide the rest of them.
pub(super) fn catalog(
    exec: &mut dyn Executor,
    namespace: Option<&str>,
) -> Result<Vec<DataEntityInfo>, DataError> {
    let mut params = Params::new(exec.dialect());
    // Two statements rather than `(?1 IS NULL OR namespace = ?1)`: Postgres cannot infer the type
    // of a parameter that is only ever compared with NULL.
    let filter = match namespace {
        Some(namespace) => format!(
            "WHERE namespace = {}",
            params.bind(DataValue::Text(namespace.to_owned()))
        ),
        None => String::new(),
    };
    let sql = format!(
        "SELECT namespace, entity, definition, owner_plugin FROM store_entities {} \
         ORDER BY namespace, entity",
        filter
    );
    let rows = query_all(
        exec,
        &sql,
        &params.into_values(),
        &[
            AttrKind::Text,
            AttrKind::Text,
            AttrKind::Text,
            AttrKind::Text,
        ],
    )
    .context("failed to list the entity registry")?;
    let mut entities = Vec::new();
    for row in rows {
        let mut texts = row.into_iter().map(|value| match value {
            DataValue::Text(s) => Ok(s),
            other => Err(DataError::Storage(format!(
                "the entity registry holds {:?} where text was expected",
                other
            ))),
        });
        let (namespace, entity, definition, owner_plugin) = (
            texts.next().transpose()?.unwrap_or_default(),
            texts.next().transpose()?.unwrap_or_default(),
            texts.next().transpose()?.unwrap_or_default(),
            texts.next().transpose()?.unwrap_or_default(),
        );
        match serde_json::from_str::<EntityDef>(&definition) {
            Ok(def) => entities.push(DataEntityInfo {
                namespace,
                owner_plugin,
                def,
            }),
            Err(e) => log::error!(
                "the stored definition of `{}` in `{}` is unreadable, skipping it: {}",
                entity,
                namespace,
                e
            ),
        }
    }
    Ok(entities)
}

pub(super) fn require_entity(
    exec: &mut dyn Executor,
    namespace: &str,
    entity: &str,
) -> Result<EntitySchema, DataError> {
    load_entity(exec, namespace, entity)?.ok_or_else(|| {
        DataError::UnknownEntity(format!(
            "`{}` has not been declared in namespace `{}`; call define_entity first",
            entity, namespace
        ))
    })
}

pub(super) fn create_entity(
    exec: &mut dyn Executor,
    namespace: &str,
    owner_plugin: &str,
    def: &EntityDef,
) -> Result<DataSchemaInfo, DataError> {
    let definition = serde_json::to_string(def)
        .map_err(|e| storage_err("failed to serialize the entity definition", e))?;
    let mut params = Params::new(exec.dialect());
    let values = [
        params.bind(DataValue::Text(namespace.to_owned())),
        params.bind(DataValue::Text(def.name.clone())),
        params.bind(DataValue::Int(def.version as i64)),
        params.bind(DataValue::Text(definition)),
        params.bind(DataValue::Text(owner_plugin.to_owned())),
    ];
    let sql = format!(
        "INSERT INTO store_entities (namespace, entity, version, definition, owner_plugin) \
         VALUES ({}) RETURNING id",
        values.join(", ")
    );
    let row_id = exec
        .query_i64(&sql, &params.into_values())
        .context("failed to register the entity")?;
    // The table is named after the registry row, so it can only be created once the row exists.
    let schema = EntitySchema {
        row_id,
        def: def.clone(),
        owner_plugin: owner_plugin.to_owned(),
    };
    create_table(exec, &schema)?;

    let mut indexes_created = Vec::new();
    for attr in &def.attrs {
        if create_attr_indexes(exec, &schema, attr)? {
            indexes_created.push(attr.name.clone());
        }
    }
    let full_text_rebuilt = rebuild_full_text(exec, &schema)?;
    Ok(DataSchemaInfo {
        indexes_created,
        full_text_rebuilt,
        ..schema_info(namespace, def, true)
    })
}

/// Record a migrated definition against its registry row.
pub(super) fn store_definition(
    exec: &mut dyn Executor,
    row_id: i64,
    def: &EntityDef,
) -> Result<(), DataError> {
    let definition = serde_json::to_string(def)
        .map_err(|e| storage_err("failed to serialize the entity definition", e))?;
    let mut params = Params::new(exec.dialect());
    let version = params.bind(DataValue::Int(def.version as i64));
    let definition = params.bind(DataValue::Text(definition));
    let id = params.bind(DataValue::Int(row_id));
    let sql = format!(
        "UPDATE store_entities SET version = {}, definition = {} WHERE id = {}",
        version, definition, id
    );
    exec.execute(&sql, &params.into_values())
        .context("failed to record the new definition")?;
    Ok(())
}

/// An empty report for one entity, for `create_entity` and `migrate_entity` to fill in.
///
/// The two differ only in `created` and in which of the vectors they end up populating.
pub(super) fn schema_info(namespace: &str, def: &EntityDef, created: bool) -> DataSchemaInfo {
    DataSchemaInfo {
        namespace: namespace.to_owned(),
        entity: def.name.clone(),
        version: def.version,
        created,
        attrs_added: Vec::new(),
        indexes_created: Vec::new(),
        full_text_rebuilt: false,
    }
}

pub(super) fn column_definition(
    dialect: &dyn Dialect,
    attr: &AttrDef,
) -> Result<String, DataError> {
    let mut sql = format!(
        "{} {}",
        super::sql::quote_ident(&attr.name),
        dialect.column_type(attr.kind)
    );
    if attr.required {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = &attr.default {
        sql.push_str(&format!(" DEFAULT {}", dialect.literal(default)?));
    }
    Ok(sql)
}

fn registry_row(row: Vec<DataValue>) -> Result<(i64, String, String), DataError> {
    let mut values = row.into_iter();
    let unexpected = |what: &str, value: Option<DataValue>| {
        DataError::Storage(format!(
            "the entity registry holds {:?} where {} was expected",
            value, what
        ))
    };
    let row_id = match values.next() {
        Some(DataValue::Int(id)) => id,
        other => return Err(unexpected("an id", other)),
    };
    let definition = match values.next() {
        Some(DataValue::Text(s)) => s,
        other => return Err(unexpected("a definition", other)),
    };
    let owner_plugin = match values.next() {
        Some(DataValue::Text(s)) => s,
        other => return Err(unexpected("an owner", other)),
    };
    Ok((row_id, definition, owner_plugin))
}
