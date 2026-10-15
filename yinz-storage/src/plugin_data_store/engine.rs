//! The store's five operations, written once against [`Executor`].
//!
//! A backend opens its connection and its transaction, wraps them in its executor, and calls
//! in here. Nothing below this line knows which database it is talking to.

use std::collections::BTreeMap;

use yinz_types::plugin_data::{
    ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
    Mutation,
};

use super::executor::Executor;
use super::migrate::migrate_entity;
use super::query::{run_count_with_search_fallback, run_fetch_with_search_fallback};
use super::registry::{catalog as registry_catalog, create_entity, load_entity, require_entity};
use super::schema::EntitySchema;
use super::write::apply_one;

/// Declare an entity, creating or migrating its table to match. Runs inside the backend's
/// transaction.
///
/// Idempotent: re-declaring an identical definition touches nothing. The host diffs the stored
/// definition against this one rather than comparing versions, so a plugin that forgets to bump
/// `version` still gets its new attributes.
pub(super) fn define_entity(
    exec: &mut dyn Executor,
    namespace: &str,
    owner_plugin: &str,
    def: &EntityDef,
    allow_destructive: bool,
) -> Result<DataSchemaInfo, DataError> {
    exec.lock_schema()?;
    match load_entity(exec, namespace, &def.name)? {
        Some(existing) => migrate_entity(
            exec,
            namespace,
            owner_plugin,
            &existing,
            def,
            allow_destructive,
        ),
        None => create_entity(exec, namespace, owner_plugin, def),
    }
}

/// Every entity the store holds, or only those of one namespace.
///
/// The seam behind `DataCatalog`. Filtering to what a plugin may see is the caller's job: this
/// is the engine, and it has no idea who is asking.
pub(super) fn catalog(
    exec: &mut dyn Executor,
    namespace: Option<&str>,
) -> Result<Vec<DataEntityInfo>, DataError> {
    registry_catalog(exec, namespace)
}

pub(super) fn fetch(
    exec: &mut dyn Executor,
    namespace: &str,
    request: &FetchRequest,
) -> Result<DataRows, DataError> {
    let schema = require_entity(exec, namespace, &request.entity)?;
    run_fetch_with_search_fallback(exec, &schema, request)
}

/// Count the rows a fetch would match, ignoring its limit, offset and cursor.
pub(super) fn count(
    exec: &mut dyn Executor,
    namespace: &str,
    request: &FetchRequest,
) -> Result<u64, DataError> {
    let schema = require_entity(exec, namespace, &request.entity)?;
    run_count_with_search_fallback(exec, &schema, request)
}

/// Apply a batch of mutations. Runs inside the backend's transaction, so it is all or nothing.
pub(super) fn apply(
    exec: &mut dyn Executor,
    namespace: &str,
    mutations: &[Mutation],
    now: i64,
) -> Result<ApplyReport, DataError> {
    let mut report = ApplyReport::default();
    // Schemas are looked up once per entity per batch. Not cached across calls: another yinz
    // server sharing this database could have migrated an entity since, and one indexed lookup
    // is nothing beside the wasm and protobuf hops this call already cost.
    let mut schemas: BTreeMap<String, EntitySchema> = BTreeMap::new();
    for mutation in mutations {
        let entity = mutation.entity();
        if !schemas.contains_key(entity) {
            schemas.insert(entity.to_owned(), require_entity(exec, namespace, entity)?);
        }
    }
    if mutations
        .iter()
        .any(|mutation| matches!(mutation, Mutation::AssertRows { .. }))
    {
        let mut tables: Vec<_> = schemas.values().map(EntitySchema::table).collect();
        tables.sort();
        exec.lock_writes(&tables)?;
    }
    for mutation in mutations {
        let schema = &schemas[mutation.entity()];
        apply_one(exec, schema, mutation, now, &mut report)?;
    }
    Ok(report)
}
