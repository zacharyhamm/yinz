//! The engine behind the plugin data store, and the databases it runs on.
//!
//! Host-only: plugins use the shared types in [`crate::plugin_data`]; the host owns storage.
//!
//! One real table per `(namespace, entity)`, named `e_<n>` after its row in the
//! `store_entities` registry. Attribute names pass through
//! [`yinz_types::plugin_data::validate_name`] before SQL composition.
//!
//! [`engine`] composes operations using [`dialect`] and [`executor`]. The [`sqlite`]
//! backend opens the database and supplies both interfaces. Other backends can implement
//! [`PluginDataBackend`]; validation and [`watch`] remain independent of storage.
//!
//! SQLite uses external-content FTS5 tables maintained by triggers and ranked by `bm25`.
//! Invalid search syntax is retried as a literal phrase. Text ordering is byte-wise,
//! pattern matching folds ASCII case, and nulls sort first ascending and last descending.
//! Text values reject NUL bytes; binary values belong in blobs.

mod ddl;
mod dialect;
mod engine;
mod executor;
mod migrate;
mod predicate;
mod query;
mod registry;
mod schema;
mod sql;
mod sqlite;
mod store_migrations;
mod value;
mod watch;
mod write;

#[cfg(test)]
pub(crate) mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use yinz_types::internal_ids::DataWatchId;

use crate::data_access::PluginDataBackend;
use yinz_types::internal_ids::{ClientId, PluginId};
use yinz_types::plugin_data::{
    ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, DataWatchHandle, EntityDef,
    FetchRequest, Mutation, WatchMode,
};

use self::watch::{apply_watch_mode, WatchRegistry};

pub use self::sqlite::SqlitePluginDataBackend;
pub use self::watch::WatchChange;

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
/// Keep the complete URL: replacing punctuation with underscores aliases distinct plugins.
/// This is a database value, not a filename, and SQL parameters need no path sanitization.
pub fn private_namespace(plugin_url: &str) -> String {
    format!("plugin:{}", plugin_url)
}

/// Whether a resolved namespace is one shared between plugins.
///
/// The `shared:` prefix itself is enforced by [`yinz_types::plugin_data::validate_namespace`], and the
/// permission that goes with it is checked in `resolve_data_namespace`, so nothing in the server
/// asks this today.
#[cfg(test)]
pub(crate) fn is_shared_namespace(namespace: &str) -> bool {
    namespace.starts_with(yinz_types::plugin_data::SHARED_NAMESPACE_PREFIX)
}

/// Public plugin-data façade. Validation and watches do not depend on the storage backend.
pub struct PluginDataStore {
    backend: Arc<dyn PluginDataBackend>,
    /// Serializes registration with writes so a watch cannot miss the update between its initial
    /// fetch and insertion into the registry.
    operation: Mutex<()>,
    watches: WatchRegistry,
}

impl std::fmt::Debug for PluginDataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDataStore").finish_non_exhaustive()
    }
}

impl PluginDataStore {
    /// SQLite-compatible constructor retained for callers that only need plugin data.
    pub fn new(data_dir: &Path) -> Self {
        Self::from_backend(Arc::new(SqlitePluginDataBackend::new(data_dir)))
    }

    pub fn from_backend(backend: Arc<dyn PluginDataBackend>) -> Self {
        Self {
            backend,
            operation: Mutex::new(()),
            watches: WatchRegistry::new(),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, DataError> {
        self.operation.lock().map_err(|_| {
            DataError::Storage("the plugin data store operation lock is poisoned".to_owned())
        })
    }

    pub fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        def.validate()?;
        let _guard = self.lock()?;
        self.backend
            .define_entity(namespace, owner_plugin, def, allow_destructive)
    }

    pub fn catalog(&self, namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
        let _guard = self.lock()?;
        self.backend.catalog(namespace)
    }

    pub fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
        request.validate()?;
        let _guard = self.lock()?;
        self.backend.fetch(namespace, request)
    }

    pub fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
        request.validate()?;
        let _guard = self.lock()?;
        self.backend.count(namespace, request)
    }

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
        let _guard = self.lock()?;
        let report = self.backend.apply(namespace, mutations)?;
        let touched_entities = mutations
            .iter()
            .map(|mutation| mutation.entity().to_owned())
            .collect();
        Ok(ApplyOutcome {
            report,
            touched_entities,
        })
    }

    /// Register a standing fetch and return its current results.
    ///
    /// The initial rows come back with the handle so that a plugin cannot miss a change in the gap
    /// between fetching and watching.
    pub fn register_watch(
        &self,
        namespace: &str,
        plugin_id: PluginId,
        client_id: ClientId,
        request: FetchRequest,
        mode: WatchMode,
    ) -> Result<DataWatchHandle, DataError> {
        request.validate()?;
        let request = apply_watch_mode(request, mode);
        let _guard = self.lock()?;
        let rows = self.backend.fetch(namespace, &request)?;
        self.watches
            .register(namespace, plugin_id, client_id, request, rows)
    }

    /// Cancel a watch. A plugin may only cancel its own.
    pub fn unregister_watch(
        &self,
        plugin_id: PluginId,
        watch_id: DataWatchId,
    ) -> Result<(), DataError> {
        self.watches.unregister(plugin_id, watch_id)
    }

    /// Drop every watch belonging to a plugin, for when it unloads.
    pub fn drop_watches_for_plugin(&self, plugin_id: PluginId) {
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
        let _guard = self.lock()?;
        self.watches
            .refresh(namespace, touched_entities, |namespace, request| {
                self.backend.fetch(namespace, request)
            })
    }

    /// How many watches are registered.
    #[cfg(test)]
    pub(crate) fn watch_count(&self) -> usize {
        self.watches.count()
    }
}
