//! Process-wide access to host-owned persistent data.
//!
//! Callers depend on these synchronous interfaces; SQLite is the default concrete backend.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::plugin_data::{
    ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
    Mutation,
};
use crate::plugin_data_store::{PluginDataStore, SqlitePluginDataBackend};
use crate::session_recording::{
    PaneMetadata, RecordedEvent, RecordingSummary, SearchHit, SqliteRecordingBackend,
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
    fn update_panes(&mut self, panes: &[PaneMetadata], elapsed_us: i64) -> Result<()>;
    fn finish_pane(&mut self, terminal_id: u32, elapsed_us: i64) -> Result<()>;
    fn finish_session(&mut self, ended_at_us: i64, ended_elapsed_us: i64) -> Result<()>;
    fn mark_incomplete(&mut self, message: &str) -> Result<()>;
}

/// Persistence operations for recordings outside an active session.
pub trait RecordingBackend: Send + Sync {
    fn start_recording(
        &self,
        session_name: &str,
        capture_input: bool,
        started_at_us: i64,
    ) -> Result<Box<dyn RecordingSession>>;
    fn list(&self) -> Result<Vec<RecordingSummary>>;
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;
    fn export(&self, recording_id: i64, output: &mut dyn Write) -> Result<()>;
}

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
