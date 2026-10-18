//! Process-wide access to host-owned persistent data.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::plugin_data_store::{PluginDataStore, SqlitePluginDataBackend};
use yinz_types::plugin_data::{
    ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
    Mutation,
};

/// Persistence operations used by the plugin-data façade.
pub trait PluginDataBackend: Send + Sync {
    /// A connection-local token that changes after another connection commits.
    /// Backends without external change detection retain local-only watches.
    fn external_version(&self) -> Result<Option<u64>, DataError> {
        Ok(None)
    }

    fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError>;
    fn catalog(&self, namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError>;
    fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError>;
    fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError>;
    fn apply(&self, namespace: &str, mutations: &[Mutation]) -> Result<ApplyReport, DataError>;
}

/// The data backend shared by one server process.
#[derive(Clone)]
pub struct DataAccess {
    plugin_data: Arc<PluginDataStore>,
}

impl std::fmt::Debug for DataAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataAccess").finish_non_exhaustive()
    }
}

impl DataAccess {
    /// The default provider. Construction is lazy and does not create the database.
    pub fn sqlite(data_dir: &Path) -> Self {
        Self::from_backend(Arc::new(SqlitePluginDataBackend::new(data_dir)))
    }

    /// Inject a backend, primarily for hosts and tests that do not want SQLite.
    pub fn from_backend(plugin_data: Arc<dyn PluginDataBackend>) -> Self {
        Self {
            plugin_data: Arc::new(PluginDataStore::from_backend(plugin_data)),
        }
    }

    pub fn plugin_data(&self) -> &Arc<PluginDataStore> {
        &self.plugin_data
    }
}
