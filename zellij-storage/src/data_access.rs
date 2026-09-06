//! Process-wide access to host-owned persistent data.
//!
//! Callers depend on these synchronous interfaces. SQLite is the default concrete backend for
//! both plugin data and recordings; plugin data can be pointed at Postgres instead with
//! `plugin_data_url`, in which case recordings stay in SQLite — which is [`DataAccess::postgres`],
//! and the reason [`DataAccess::from_backends`] takes two backends rather than one.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use zellij_types::internal_ids::{RecordingId, TerminalId};
use zellij_types::units::{ElapsedMicros, UnixMicros};

use anyhow::Result;

use crate::plugin_data_store::{
    PluginDataStore, PostgresPluginDataBackend, SqlitePluginDataBackend,
};
use crate::session_recording::{
    PaneMetadata, RecordedEvent, RecordingSummary, SearchHit, SqliteRecordingBackend,
};
use zellij_types::plugin_data::{
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

/// One active recording session.
pub trait RecordingSession: Send {
    /// Commit the entire batch or return an error without retaining any of its events.
    fn record_events(&mut self, events: &[RecordedEvent]) -> Result<()>;
    fn update_panes(&mut self, panes: &[PaneMetadata], elapsed_us: ElapsedMicros) -> Result<()>;
    fn finish_pane(&mut self, terminal_id: TerminalId, elapsed_us: ElapsedMicros) -> Result<()>;
    fn finish_session(
        &mut self,
        ended_at_us: UnixMicros,
        ended_elapsed_us: ElapsedMicros,
    ) -> Result<()>;
    fn mark_incomplete(&mut self, message: &str) -> Result<()>;
}

/// Persistence operations for recordings outside an active session.
pub trait RecordingBackend: Send + Sync {
    fn start_recording(
        &self,
        session_name: &str,
        capture_input: bool,
        started_at_us: UnixMicros,
    ) -> Result<Box<dyn RecordingSession>>;
    fn list(&self) -> Result<Vec<RecordingSummary>>;
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;
    fn export(&self, recording_id: RecordingId, output: &mut dyn Write) -> Result<()>;
}

/// The Postgres schema the plugin data store uses when `plugin_data_schema` is unset.
pub const DEFAULT_PLUGIN_DATA_SCHEMA: &str = "zellij_plugin_data";

/// The data backends shared by one server process.
#[derive(Clone)]
pub struct DataAccess {
    plugin_data: Arc<PluginDataStore>,
    recordings: Arc<dyn RecordingBackend>,
}

impl std::fmt::Debug for DataAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataAccess").finish_non_exhaustive()
    }
}

impl DataAccess {
    /// The default provider. Construction is lazy and does not create either database.
    pub fn sqlite(data_dir: &Path) -> Self {
        Self::from_backends(
            Arc::new(SqlitePluginDataBackend::new(data_dir)),
            Arc::new(SqliteRecordingBackend::new(data_dir)),
        )
    }

    /// Plugin data in Postgres, recordings in the SQLite file under `data_dir`. Lazy: nothing
    /// connects here, and a URL that does not parse is reported on first use.
    pub fn postgres(url: &str, schema: &str, data_dir: &Path) -> Self {
        Self::from_backends(
            Arc::new(PostgresPluginDataBackend::new(url, schema)),
            Arc::new(SqliteRecordingBackend::new(data_dir)),
        )
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
                    "plugin data store: Postgres at {} schema `{}`; recordings stay in {:?}",
                    crate::plugin_data_store::redact_url(url),
                    schema,
                    data_dir
                );
                Self::postgres(url, schema, data_dir)
            },
        }
    }

    /// Inject backends, primarily for hosts and tests that do not want SQLite.
    pub fn from_backends(
        plugin_data: Arc<dyn PluginDataBackend>,
        recordings: Arc<dyn RecordingBackend>,
    ) -> Self {
        Self {
            plugin_data: Arc::new(PluginDataStore::from_backend(plugin_data)),
            recordings,
        }
    }

    pub fn plugin_data(&self) -> &Arc<PluginDataStore> {
        &self.plugin_data
    }

    pub fn recordings(&self) -> &Arc<dyn RecordingBackend> {
        &self.recordings
    }
}
