//! Standing fetches, and telling a real change from the same rows written again.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};
use yinz_types::internal_ids::DataWatchId;

use yinz_types::internal_ids::{ClientId, PluginId};
use yinz_types::plugin_data::{
    DataError, DataRows, DataValue, DataWatchHandle, FetchRequest, WatchMode,
};

/// A standing fetch, and what its results looked like the last time anyone checked.
#[derive(Debug, Clone)]
struct Watch {
    watch_id: DataWatchId,
    /// The server's `PluginId`, kept as its underlying type so that this module needs nothing from
    /// `yinz-server`.
    plugin_id: PluginId,
    client_id: ClientId,
    namespace: String,
    entity: String,
    request: FetchRequest,
    last_digest: u64,
}

/// Every watch registered in this process, and the id to hand the next one.
///
/// Not persisted: a watch belongs to a running plugin instance, and a plugin that has been
/// reloaded has to register again anyway.
///
/// The façade takes its operation lock before this lock, so registration is atomic with its
/// initial fetch and writes cannot slip through that gap.
#[derive(Debug, Default)]
pub(super) struct WatchRegistry {
    watches: Mutex<(Vec<Watch>, DataWatchId)>,
}

impl WatchRegistry {
    pub(super) fn new() -> Self {
        WatchRegistry {
            watches: Mutex::new((Vec::new(), DataWatchId::new(1))),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, (Vec<Watch>, DataWatchId)>, DataError> {
        self.watches.lock().map_err(|_| {
            DataError::Storage("the plugin data store watch lock is poisoned".to_owned())
        })
    }

    /// Register a standing fetch and return its current results.
    pub(super) fn register(
        &self,
        namespace: &str,
        plugin_id: PluginId,
        client_id: ClientId,
        request: FetchRequest,
        rows: DataRows,
    ) -> Result<DataWatchHandle, DataError> {
        let mut guard = self.lock()?;
        let (watches, next_id) = &mut *guard;
        let watch_id = *next_id;
        *next_id = DataWatchId::new(next_id.get().checked_add(1).ok_or_else(|| {
            DataError::Storage("plugin data watch identities exhausted".to_owned())
        })?);
        watches.push(Watch {
            watch_id,
            plugin_id,
            client_id,
            namespace: namespace.to_owned(),
            entity: request.entity.clone(),
            request,
            last_digest: digest_rows(&rows),
        });
        Ok(DataWatchHandle { watch_id, rows })
    }

    /// Cancel a watch. A plugin may only cancel its own.
    pub(super) fn unregister(
        &self,
        plugin_id: PluginId,
        watch_id: DataWatchId,
    ) -> Result<(), DataError> {
        let mut guard = self.lock()?;
        let (watches, _) = &mut *guard;
        let before = watches.len();
        watches.retain(|w| !(w.watch_id == watch_id && w.plugin_id == plugin_id));
        if watches.len() == before {
            return Err(DataError::InvalidQuery(format!(
                "watch {} is not registered by this plugin",
                watch_id
            )));
        }
        Ok(())
    }

    /// Drop every watch belonging to a plugin, for when it unloads.
    pub(super) fn drop_for_plugin(&self, plugin_id: PluginId) {
        if let Ok(mut guard) = self.watches.lock() {
            guard.0.retain(|w| w.plugin_id != plugin_id);
        }
    }

    /// Whether any watch could care about a write to these entities.
    ///
    /// Lets the store skip taking the connection lock at all in the common case of a write nobody
    /// is watching.
    pub(super) fn any_watching(
        &self,
        namespace: &str,
        touched_entities: &BTreeSet<String>,
    ) -> Result<bool, DataError> {
        Ok(self
            .lock()?
            .0
            .iter()
            .any(|w| w.namespace == namespace && touched_entities.contains(&w.entity)))
    }

    /// Re-run every watch a write to these entities could have affected, and report the ones whose
    /// results actually changed.
    ///
    /// Comparing a digest rather than reporting unconditionally is what makes an idempotent write
    /// silent — a plugin that re-puts the same row does not wake every watcher.
    pub(super) fn refresh(
        &self,
        affected: Option<(&str, &BTreeSet<String>)>,
        mut fetch: impl FnMut(&str, &FetchRequest) -> Result<DataRows, DataError>,
    ) -> Result<Vec<WatchChange>, DataError> {
        let mut guard = self.lock()?;
        let (watches, _) = &mut *guard;
        let mut changes = Vec::new();
        for watch in watches.iter_mut() {
            if affected.is_some_and(|(namespace, entities)| {
                watch.namespace != namespace || !entities.contains(&watch.entity)
            }) {
                continue;
            }
            // A watch whose entity has since been dropped, or whose fetch has become invalid, is
            // skipped rather than failing the write that triggered this.
            let Ok(rows) = fetch(&watch.namespace, &watch.request) else {
                continue;
            };
            let digest = digest_rows(&rows);
            if digest == watch.last_digest {
                continue;
            }
            watch.last_digest = digest;
            changes.push(WatchChange {
                watch_id: watch.watch_id,
                plugin_id: watch.plugin_id,
                client_id: watch.client_id,
                rows,
            });
        }
        Ok(changes)
    }

    /// How many watches are registered.
    pub(super) fn count(&self) -> usize {
        self.watches.lock().map(|guard| guard.0.len()).unwrap_or(0)
    }
}

/// A watch whose results have changed, and the rows to send it.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchChange {
    pub watch_id: DataWatchId,
    pub plugin_id: PluginId,
    pub client_id: ClientId,
    pub rows: DataRows,
}

/// Narrow a watch's fetch to just the `id` column when it only wants ids.
pub(super) fn apply_watch_mode(mut request: FetchRequest, mode: WatchMode) -> FetchRequest {
    if mode == WatchMode::Ids {
        request.select = vec!["id".to_owned()];
    }
    request
}

/// A digest of a result set, for telling "changed" from "written again with the same values".
///
/// Use `Hash`'s length framing for variable-size values: a delimiter alone can also occur in a
/// blob and make different rows indistinguishable. Floats are hashed by their bit pattern.
pub(super) fn digest_rows(rows: &DataRows) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.columns.hash(&mut hasher);
    rows.rows.len().hash(&mut hasher);
    for row in &rows.rows {
        row.len().hash(&mut hasher);
        for value in row {
            std::mem::discriminant(value).hash(&mut hasher);
            match value {
                DataValue::Null => {},
                DataValue::Int(i) => i.hash(&mut hasher),
                DataValue::Real(f) => f.to_bits().hash(&mut hasher),
                DataValue::Text(s) => s.hash(&mut hasher),
                DataValue::Bool(b) => b.hash(&mut hasher),
                DataValue::Blob(b) => b.hash(&mut hasher),
            }
        }
    }
    // Truncation is part of the result set: a watch whose rows were cut off and then were not is a
    // change worth reporting.
    rows.truncated.hash(&mut hasher);
    rows.next_cursor.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_watch_ids_do_not_reuse_an_identity_or_add_a_watch() {
        let watches = WatchRegistry::new();
        watches.lock().unwrap().1 = DataWatchId::new(u64::MAX);
        let result = watches.register(
            "test",
            PluginId::new(1),
            ClientId::new(1),
            FetchRequest::entity("row"),
            DataRows::default(),
        );
        assert!(matches!(result, Err(DataError::Storage(_))));
        assert_eq!(watches.count(), 0);
        assert_eq!(watches.lock().unwrap().1, DataWatchId::new(u64::MAX));
    }
}
