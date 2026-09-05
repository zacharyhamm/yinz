//! The shared suite against SQLite: a fresh file in a temp dir per test.

use super::*;
use tempfile::{tempdir, TempDir};

/// Keeps the temp dir, and so the database file, alive for the test.
pub(super) type Guard = TempDir;

fn store() -> (PluginDataStore, Guard) {
    let dir = tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    (store, dir)
}

fn defined() -> (PluginDataStore, Guard) {
    let (store, dir) = store();
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    (store, dir)
}

#[path = "model.rs"]
mod model;
#[path = "oracle.rs"]
mod oracle;
#[path = "pagination.rs"]
mod pagination;
#[path = "properties.rs"]
mod properties;
#[path = "queries.rs"]
mod queries;
#[path = "schema.rs"]
mod schema;
#[path = "search.rs"]
mod search;
#[path = "watches.rs"]
mod watches;
#[path = "writes.rs"]
mod writes;

/// SQLite can hold a NUL in text; the store still refuses it, so that a plugin's data means the
/// same thing on every engine. Pinned here because it is the one place the engine could say yes.
#[test]
fn a_nul_byte_in_text_is_rejected_before_it_reaches_sqlite() {
    let (store, _dir) = defined();
    let err = store
        .apply(NS, &[Mutation::put("agent").set("project", "a\0b").build()])
        .unwrap_err();
    assert_err!(err, DataError::InvalidQuery(_));
}
