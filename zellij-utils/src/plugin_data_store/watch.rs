//! Standing fetches, and telling a real change from the same rows written again.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::data::ClientId;
use crate::plugin_data::{
    DataError, DataRows, DataValue, DataWatchHandle, FetchRequest, WatchMode,
};

use super::query::run_fetch_with_search_fallback;
use super::registry::require_entity;

/// A standing fetch, and what its results looked like the last time anyone checked.
#[derive(Debug, Clone)]
struct Watch {
    watch_id: u64,
    /// The server's `PluginId`, kept as its underlying type so that this module needs nothing from
    /// `zellij-server`.
    plugin_id: u32,
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
/// The lock inside is always taken *within* the store's connection lock, so the two are taken in
/// the same order everywhere and cannot deadlock against each other. That is why [`Self::refresh`]
/// and [`Self::register`] take a `&Connection` rather than reaching for one: the caller can only
/// have got one by holding the connection lock already.
#[derive(Debug, Default)]
pub(super) struct WatchRegistry {
    watches: Mutex<(Vec<Watch>, u64)>,
}

impl WatchRegistry {
    pub(super) fn new() -> Self {
        WatchRegistry {
            watches: Mutex::new((Vec::new(), 1)),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, (Vec<Watch>, u64)>, DataError> {
        self.watches.lock().map_err(|_| {
            DataError::Storage("the plugin data store watch lock is poisoned".to_owned())
        })
    }

    /// Register a standing fetch and return its current results.
    pub(super) fn register(
        &self,
        conn: &Connection,
        namespace: &str,
        plugin_id: u32,
        client_id: ClientId,
        request: FetchRequest,
    ) -> Result<DataWatchHandle, DataError> {
        let schema = require_entity(conn, namespace, &request.entity)?;
        let rows = run_fetch_with_search_fallback(conn, &schema, &request)?;
        let mut guard = self.lock()?;
        let (watches, next_id) = &mut *guard;
        let watch_id = *next_id;
        *next_id += 1;
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
    pub(super) fn unregister(&self, plugin_id: u32, watch_id: u64) -> Result<(), DataError> {
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
    pub(super) fn drop_for_plugin(&self, plugin_id: u32) {
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
        conn: &Connection,
        namespace: &str,
        touched_entities: &BTreeSet<String>,
    ) -> Result<Vec<WatchChange>, DataError> {
        let mut guard = self.lock()?;
        let (watches, _) = &mut *guard;
        let mut changes = Vec::new();
        for watch in watches.iter_mut() {
            if watch.namespace != namespace || !touched_entities.contains(&watch.entity) {
                continue;
            }
            // A watch whose entity has since been dropped, or whose fetch has become invalid, is
            // skipped rather than failing the write that triggered this.
            let Ok(schema) = require_entity(conn, &watch.namespace, &watch.entity) else {
                continue;
            };
            let Ok(rows) = run_fetch_with_search_fallback(conn, &schema, &watch.request) else {
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
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        self.watches.lock().map(|guard| guard.0.len()).unwrap_or(0)
    }
}

/// A watch whose results have changed, and the rows to send it.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchChange {
    pub watch_id: u64,
    pub plugin_id: u32,
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
/// Hand-rolled rather than `Hash` because `f64` is not `Hash`; floats are folded in by their bit
/// pattern, which is what an equality check on the stored value amounts to anyway.
pub(super) fn digest_rows(rows: &DataRows) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for column in &rows.columns {
        hasher.write(column.as_bytes());
        hasher.write_u8(0xff);
    }
    for row in &rows.rows {
        for value in row {
            match value {
                DataValue::Null => hasher.write_u8(0),
                DataValue::Int(i) => {
                    hasher.write_u8(1);
                    hasher.write_i64(*i);
                },
                DataValue::Real(f) => {
                    hasher.write_u8(2);
                    hasher.write_u64(f.to_bits());
                },
                DataValue::Text(s) => {
                    hasher.write_u8(3);
                    hasher.write(s.as_bytes());
                },
                DataValue::Bool(b) => {
                    hasher.write_u8(4);
                    hasher.write_u8(*b as u8);
                },
                DataValue::Blob(b) => {
                    hasher.write_u8(5);
                    hasher.write(b);
                },
            }
            hasher.write_u8(0xfe);
        }
        hasher.write_u8(0xfd);
    }
    // Truncation is part of the result set: a watch whose rows were cut off and then were not is a
    // change worth reporting.
    hasher.write_u8(rows.truncated as u8);
    hasher.finish()
}
