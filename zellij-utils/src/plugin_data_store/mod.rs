//! The SQLite engine behind the plugin data store.
//!
//! Host-only: `rusqlite` cannot build for `wasm32-wasip1`, and even if it could, plugins run under
//! `wasmi` — a pure interpreter with a 16 MiB linear memory configured to trap rather than fail
//! when it can't grow. SQLite's page cache does not belong there. The types plugins speak in live
//! in [`crate::plugin_data`], which is compiled for both sides; this module turns them into SQL.
//!
//! # Layout
//!
//! One real table per `(namespace, entity)`, named `e_<n>` after its row in the `store_entities`
//! registry. No plugin-supplied string is ever interpolated into a table name. Attribute names
//! *are* interpolated as column names, which is why [`crate::plugin_data::validate_name`] is the
//! boundary that keeps a plugin from writing SQL, and why every entry point runs it before
//! composing anything.
//!
//! Full-text search uses an external-content FTS5 table kept in sync by triggers. That differs
//! deliberately from [`crate::session_recording`], whose watermark table exists only because
//! indexing terminal output needs a `vte` replay that cannot run inside a trigger. Here the
//! indexed text is already a column, so triggers keep the index exact for free.
//!
//! # Modules
//!
//! [`registry`] owns the `store_entities` table and the file itself; [`schema`] is what one entity
//! looks like once read back, and the source of every name derived from it; [`migrate`] brings a
//! stored table in line with a new definition, with [`index`] maintaining the secondary and
//! full-text indexes underneath it. [`predicate`] compiles a `Pred` to SQL, [`query`] builds and
//! runs fetches around it, [`write`] applies mutations, and [`watch`] re-runs standing fetches.
//! [`sql`] and [`value`] hold the quoting and the value conversions the rest of them share.

mod index;
mod migrate;
mod predicate;
mod query;
mod registry;
mod schema;
mod sql;
mod value;
mod watch;
mod write;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, TransactionBehavior};

use crate::data::ClientId;
use crate::plugin_data::{
    ApplyReport, DataError, DataRows, DataSchemaInfo, DataWatchHandle, EntityDef, FetchRequest,
    Mutation, WatchMode, SHARED_NAMESPACE_PREFIX,
};

use self::migrate::migrate_entity;
use self::query::{build_count_sql, run_fetch_with_search_fallback, with_search_fallback};
use self::registry::{create_entity, load_entity, open_db, require_entity};
use self::schema::EntitySchema;
use self::sql::SqlContext;
use self::value::now_ms;
use self::watch::{apply_watch_mode, WatchRegistry};
use self::write::apply_one;

pub use self::watch::WatchChange;

/// Filename under the zellij data dir.
const DB_FILENAME: &str = "plugin-data.db";

/// What a batch of mutations did, plus which entities it touched.
///
/// The touched set is what watch re-evaluation filters on, so that a write to one entity doesn't
/// re-run every watch in the process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    pub report: ApplyReport,
    pub touched_entities: BTreeSet<String>,
}

/// The namespace a plugin gets when it doesn't name one.
///
/// Derived from the plugin's URL the same way `plugin_own_cache_dir` is, so it is stable across
/// instances, across clients and across sessions — and so a debug and a release build of the same
/// plugin get different namespaces, exactly as they get different permission entries.
pub fn private_namespace(plugin_url: &str) -> String {
    let sanitized: String = plugin_url
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("plugin:{}", sanitized)
}

/// Whether a resolved namespace is one shared between plugins.
pub fn is_shared_namespace(namespace: &str) -> bool {
    namespace.starts_with(SHARED_NAMESPACE_PREFIX)
}

/// The plugin data store: one SQLite file shared by every plugin, partitioned by namespace.
///
/// Cheap to construct and does no IO until first use, so a session whose plugins never touch the
/// store never creates the file.
pub struct PluginDataStore {
    db_path: PathBuf,
    connection: Mutex<Option<Connection>>,
    watches: WatchRegistry,
}

impl std::fmt::Debug for PluginDataStore {
    /// Shows the path and whether the file has been opened. Deliberately does not touch the
    /// connection mutex: this is reached through `LoadingContext`'s derived `Debug`, and a
    /// formatter that can block on a lock held by a running query is a hazard in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDataStore")
            .field("db_path", &self.db_path)
            .field(
                "open",
                &self
                    .connection
                    .try_lock()
                    .map(|guard| guard.is_some())
                    .unwrap_or(true),
            )
            .finish()
    }
}

impl PluginDataStore {
    /// Point the store at a data dir. Opens nothing.
    pub fn new(data_dir: &Path) -> Self {
        PluginDataStore {
            db_path: data_dir.join(DB_FILENAME),
            connection: Mutex::new(None),
            watches: WatchRegistry::new(),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Run `f` against the connection, opening it on first use.
    ///
    /// One connection behind one mutex. Handlers run on the calling plugin's pinned executor
    /// thread where blocking is expected, and the alternative — a connection per thread — would
    /// multiply the writers contending for the same file without making any single call faster.
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| DataError::Storage("the plugin data store lock is poisoned".to_owned()))?;
        if guard.is_none() {
            *guard = Some(open_db(&self.db_path)?);
        }
        let conn = guard
            .as_mut()
            .ok_or_else(|| DataError::Storage("the plugin data store is not open".to_owned()))?;
        f(conn)
    }
    // ---------------------------------------------------------------- schema

    /// Declare an entity, creating or migrating its table to match.
    ///
    /// Idempotent: re-declaring an identical definition touches nothing. The host diffs the stored
    /// definition against this one rather than comparing versions, so a plugin that forgets to bump
    /// `version` still gets its new attributes.
    pub fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        def.validate()?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .ctx("failed to begin a schema transaction")?;
            let info = match load_entity(&tx, namespace, &def.name)? {
                Some(existing) => migrate_entity(
                    &tx,
                    namespace,
                    owner_plugin,
                    &existing,
                    def,
                    allow_destructive,
                )?,
                None => create_entity(&tx, namespace, owner_plugin, def)?,
            };
            tx.commit().ctx("failed to commit the schema change")?;
            Ok(info)
        })
    }

    /// The definition currently stored for an entity, if it has been declared.
    pub fn entity_definition(
        &self,
        namespace: &str,
        entity: &str,
    ) -> Result<Option<EntityDef>, DataError> {
        self.with_conn(|conn| Ok(load_entity(conn, namespace, entity)?.map(|schema| schema.def)))
    }

    // ----------------------------------------------------------------- reads

    /// Run a fetch.
    pub fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
        request.validate()?;
        self.with_conn(|conn| {
            let schema = require_entity(conn, namespace, &request.entity)?;
            run_fetch_with_search_fallback(conn, &schema, request)
        })
    }

    /// Count the rows a fetch would match, ignoring its limit, offset and cursor.
    pub fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
        request.validate()?;
        self.with_conn(|conn| {
            let schema = require_entity(conn, namespace, &request.entity)?;
            with_search_fallback(request, |search| {
                let (sql, params) = build_count_sql(&schema, request, search)?;
                let count: i64 = conn
                    .query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
                        row.get(0)
                    })
                    .ctx("failed to count rows")?;
                Ok(count.max(0) as u64)
            })
        })
    }

    // ---------------------------------------------------------------- writes

    /// Apply a batch of mutations in one transaction, all or nothing.
    pub fn apply(
        &self,
        namespace: &str,
        mutations: &[Mutation],
    ) -> Result<ApplyOutcome, DataError> {
        for mutation in mutations {
            mutation.validate()?;
        }
        if mutations.is_empty() {
            return Ok(ApplyOutcome::default());
        }
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .ctx("failed to begin a write transaction")?;
            let now = now_ms();
            let mut outcome = ApplyOutcome::default();
            // Schemas are looked up once per entity per batch. Not cached across calls: another
            // zellij server sharing this file could have migrated an entity since, and one
            // indexed lookup is nothing beside the wasm and protobuf hops this call already cost.
            let mut schemas: BTreeMap<String, EntitySchema> = BTreeMap::new();
            for mutation in mutations {
                let entity = mutation.entity();
                if !schemas.contains_key(entity) {
                    schemas.insert(entity.to_owned(), require_entity(&tx, namespace, entity)?);
                }
                let schema = &schemas[entity];
                apply_one(&tx, schema, mutation, now, &mut outcome.report)?;
                outcome.touched_entities.insert(entity.to_owned());
            }
            tx.commit().ctx("failed to commit the write")?;
            Ok(outcome)
        })
    }

    // --------------------------------------------------------------- watches

    /// Register a standing fetch and return its current results.
    ///
    /// The initial rows come back with the handle so that a plugin cannot miss a change in the gap
    /// between fetching and watching.
    pub fn register_watch(
        &self,
        namespace: &str,
        plugin_id: u32,
        client_id: ClientId,
        request: FetchRequest,
        mode: WatchMode,
    ) -> Result<DataWatchHandle, DataError> {
        request.validate()?;
        let request = apply_watch_mode(request, mode);
        self.with_conn(|conn| {
            self.watches
                .register(conn, namespace, plugin_id, client_id, request)
        })
    }

    /// Cancel a watch. A plugin may only cancel its own.
    pub fn unregister_watch(&self, plugin_id: u32, watch_id: u64) -> Result<(), DataError> {
        self.watches.unregister(plugin_id, watch_id)
    }

    /// Drop every watch belonging to a plugin, for when it unloads.
    pub fn drop_watches_for_plugin(&self, plugin_id: u32) {
        self.watches.drop_for_plugin(plugin_id)
    }

    /// Re-run every watch a write to these entities could have affected, and report the ones whose
    /// results actually changed.
    ///
    /// Comparing a digest rather than reporting unconditionally is what makes an idempotent write
    /// silent — a plugin that re-puts the same row does not wake every watcher.
    pub fn refresh_watches(
        &self,
        namespace: &str,
        touched_entities: &BTreeSet<String>,
    ) -> Result<Vec<WatchChange>, DataError> {
        // Nothing to do before taking the connection lock at all in the common case of a write to an
        // entity nobody watches.
        if !self.watches.any_watching(namespace, touched_entities)? {
            return Ok(Vec::new());
        }
        self.with_conn(|conn| self.watches.refresh(conn, namespace, touched_entities))
    }

    /// How many watches are registered. For tests and diagnostics.
    pub fn watch_count(&self) -> usize {
        self.watches.count()
    }
}
