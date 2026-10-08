//! Process-wide access to host-owned persistent data.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::plugin_data_store::{
    PluginDataStore, PostgresPluginDataBackend, SqlitePluginDataBackend,
};
use yinz_types::plugin_data::{
    ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
    Mutation,
};

/// Persistence operations used by the plugin-data façade.
pub trait PluginDataBackend: Send + Sync {
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

/// The Postgres schema the plugin data store uses when `plugin_data_schema` is unset.
pub const DEFAULT_PLUGIN_DATA_SCHEMA: &str = "yinz_plugin_data";

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

    /// Plugin data in Postgres. Lazy: nothing
    /// connects here, and a URL that does not parse is reported on first use.
    pub fn postgres(url: &str, schema: &str) -> Self {
        Self::from_backend(Arc::new(PostgresPluginDataBackend::new(url, schema)))
    }

    /// The provider `Options::plugin_data_url` asks for: SQLite in the data dir when it is unset,
    /// Postgres otherwise.
    ///
    /// Infallible on purpose. The server has no error path where it builds this, and both
    /// backends already report a database they cannot open on first use — a bad URL surfaces as
    /// a `DataError::Storage` exactly as an unwritable data dir does. What it never does is fall
    /// back to SQLite: a typo in the URL must not quietly split plugin data across two stores.
    pub fn configured(url: Option<&str>, schema: Option<&str>, data_dir: &Path) -> Self {
        match url {
            None => Self::sqlite(data_dir),
            Some(url) => {
                let schema = schema.unwrap_or(DEFAULT_PLUGIN_DATA_SCHEMA);
                log::info!(
                    "plugin data store: Postgres at {} schema `{}`",
                    crate::plugin_data_store::redact_url(url),
                    schema
                );
                Self::postgres(url, schema)
            },
        }
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
