//! Independent real SQLite backends for runtime comparisons, with explicit watch delivery.
use super::runtime_parity::json_reply;
use super::runtime_parity::Responder;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use yinz_storage::plugin_data_store::{PluginDataStore, SqlitePluginDataBackend};
use yinz_types::data::*;
use yinz_types::internal_ids::DataWatchId;

pub(crate) struct DataHost {
    store: PluginDataStore,
    owner: String,
    watches: Mutex<BTreeMap<DataWatchId, (FetchRequest, DataRows)>>,
    _root: tempfile::TempDir,
}
impl DataHost {
    pub(crate) fn new(owner: &str) -> Arc<Self> {
        Self::with_clock(owner, Arc::new(AtomicU64::new(1_700_000_000_000)))
    }
    pub(crate) fn with_clock(owner: &str, clock: Arc<AtomicU64>) -> Arc<Self> {
        let root = tempfile::tempdir().unwrap();
        let backend = SqlitePluginDataBackend::with_clock(root.path(), move || {
            clock.load(Ordering::SeqCst) as i64
        });
        Arc::new(Self {
            store: PluginDataStore::from_backend(Arc::new(backend)),
            owner: owner.into(),
            watches: Mutex::default(),
            _root: root,
        })
    }
    fn namespace(&self, requested: Option<&str>) -> String {
        requested
            .map(str::to_owned)
            .unwrap_or_else(|| format!("plugin:{}", self.owner))
    }
    pub(crate) fn fetch(&self, request: &FetchRequest) -> DataRows {
        self.store
            .fetch(&self.namespace(request.namespace.as_deref()), request)
            .unwrap()
    }
    pub(crate) fn apply(&self, namespace: &str, mutations: &[Mutation]) -> ApplyReport {
        self.store.apply(namespace, mutations).unwrap().report
    }
    pub(crate) fn responder(self: &Arc<Self>, fallback: Responder) -> Responder {
        let data = self.clone();
        Arc::new(move |command| data.respond(command).or_else(|| fallback(command)))
    }
    /// The test chooses the event boundary, just as the plugin thread queues notifications.
    pub(crate) fn changed(&self) -> Vec<Event> {
        let mut watches = self.watches.lock().unwrap();
        watches
            .iter_mut()
            .filter_map(|(watch_id, (request, previous))| {
                let rows = self.fetch(request);
                if rows == *previous {
                    return None;
                }
                *previous = rows.clone();
                Some(Event::PluginDataChanged {
                    watch_id: *watch_id,
                    truncated: rows.truncated,
                    rows,
                })
            })
            .collect()
    }
    fn respond(&self, command: &PluginCommand) -> Option<super::runtime_parity::Reply> {
        Some(match command {
            PluginCommand::DataDefineEntity {
                namespace,
                def,
                allow_destructive,
            } => {
                let result = self.store.define_entity(
                    &self.namespace(namespace.as_deref()),
                    &self.owner,
                    def,
                    *allow_destructive,
                );
                match result {
                    Ok(info) => json_reply(info),
                    Err(error) => Err(error.to_string()),
                }
            },
            PluginCommand::DataFetch { request } => {
                let result = self
                    .store
                    .fetch(&self.namespace(request.namespace.as_deref()), request);
                match result {
                    Ok(rows) => crate::binary_data::rows(&rows)
                        .map(crate::response::DecodedResponse::Json)
                        .map_err(|e| e.to_string()),
                    Err(error) => Err(error.to_string()),
                }
            },
            PluginCommand::DataCount { request } => {
                let result = self
                    .store
                    .count(&self.namespace(request.namespace.as_deref()), request);
                match result {
                    Ok(count) => json_reply(count),
                    Err(error) => Err(error.to_string()),
                }
            },
            PluginCommand::DataApply {
                namespace,
                mutations,
            } => {
                let result = self
                    .store
                    .apply(&self.namespace(namespace.as_deref()), mutations);
                match result {
                    Ok(outcome) => json_reply(outcome.report),
                    Err(DataError::Conflict(error)) => Err(format!("data conflict: {error}")),
                    Err(error) => Err(error.to_string()),
                }
            },
            PluginCommand::DataWatch { request, .. } => {
                let mut watches = self.watches.lock().unwrap();
                let watch_id =
                    DataWatchId::new(watches.keys().last().map(|id| id.get()).unwrap_or(0) + 1);
                let result = self
                    .store
                    .fetch(&self.namespace(request.namespace.as_deref()), request);
                match result {
                    Ok(rows) => {
                        watches.insert(watch_id, (request.clone(), rows.clone()));
                        json_reply(
                            serde_json::json!({"watch_id":watch_id,"rows":crate::binary_data::rows(&rows).unwrap()}),
                        )
                    },
                    Err(error) => Err(error.to_string()),
                }
            },
            PluginCommand::DataUnwatch { watch_id } => {
                self.watches.lock().unwrap().remove(watch_id);
                json_reply(true)
            },
            _ => return None,
        })
    }
}
