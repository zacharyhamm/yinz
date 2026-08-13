//! The store's tests, grouped the way the engine is.
//!
//! Every group shares the fixtures below, which each file picks up with `use super::*`.

mod model;
mod oracle;
mod pagination;
mod properties;
mod queries;
mod schema;
mod search;
mod watches;
mod writes;

use super::*;
// The flat module's own `use` block used to reach the tests through `use super::*`. Now that the
// engine is split up, the types plugins speak in are imported here directly.
use crate::plugin_data::*;
use tempfile::{tempdir, TempDir};

/// Assert that an error has a particular shape, and say which shape was wanted when it doesn't.
///
/// Takes a pattern rather than a variant so that a test which accepts either of two errors can say
/// so: `assert_err!(err, DataError::InvalidName(_) | DataError::InvalidQuery(_))`.
macro_rules! assert_err {
    ($err:expr, $($pattern:tt)+) => {
        assert!(
            matches!($err, $($pattern)+),
            "expected {}, got {:?}",
            stringify!($($pattern)+),
            $err
        )
    };
}
pub(crate) use assert_err;

const NS: &str = "plugin:test";
const OWNER: &str = "zellij:test";

/// The plugin and client a watch is registered for. Shared with the watch property test in
/// [`oracle`].
const PLUGIN: u32 = 7;
const CLIENT: ClientId = 1;

/// The touched-entity set `refresh_watches` filters on, for a write to one entity.
fn touched(entity: &str) -> BTreeSet<String> {
    [entity.to_owned()].into_iter().collect()
}

fn store() -> (PluginDataStore, TempDir) {
    let dir = tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    (store, dir)
}

/// The entity most tests use: two indexed columns, one searchable one, one plain.
fn agent_def() -> EntityDef {
    EntityDef::new("agent", 1)
        .attr(AttrDef::text("project").indexed())
        .attr(AttrDef::text("branch").indexed())
        .attr(AttrDef::text("summary").full_text())
        .attr(AttrDef::int("updated_at_ms").indexed())
}

fn defined() -> (PluginDataStore, TempDir) {
    let (store, dir) = store();
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    (store, dir)
}

fn put(store: &PluginDataStore, project: &str, summary: &str, updated: i64) {
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .set("project", project)
                .set("branch", "main")
                .set("summary", summary)
                .set("updated_at_ms", updated)
                .build()],
        )
        .unwrap();
}

/// The value of one column across every returned row.
fn column(rows: &DataRows, name: &str) -> Vec<DataValue> {
    let index = rows
        .column_index(name)
        .unwrap_or_else(|| panic!("no `{}` column in {:?}", name, rows.columns));
    rows.rows.iter().map(|r| r[index].clone()).collect()
}

fn texts(rows: &DataRows, name: &str) -> Vec<String> {
    column(rows, name)
        .into_iter()
        .map(|v| match v {
            DataValue::Text(s) => s,
            other => panic!("expected text, got {:?}", other),
        })
        .collect()
}
