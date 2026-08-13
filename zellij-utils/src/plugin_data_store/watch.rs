//! Standing fetches, and telling a real change from the same rows written again.

use crate::data::ClientId;
use crate::plugin_data::{DataRows, DataValue, FetchRequest, WatchMode};

/// A standing fetch, and what its results looked like the last time anyone checked.
#[derive(Debug, Clone)]
pub(super) struct Watch {
    pub(super) watch_id: u64,
    /// The server's `PluginId`, kept as its underlying type so that this module needs nothing from
    /// `zellij-server`.
    pub(super) plugin_id: u32,
    pub(super) client_id: ClientId,
    pub(super) namespace: String,
    pub(super) entity: String,
    pub(super) request: FetchRequest,
    pub(super) last_digest: u64,
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
