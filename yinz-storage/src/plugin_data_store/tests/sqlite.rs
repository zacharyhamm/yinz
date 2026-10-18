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

fn reopen(guard: &Guard) -> PluginDataStore {
    PluginDataStore::new(guard.path())
}

fn set_store_version(guard: &Guard, version: i64) {
    let conn = rusqlite::Connection::open(guard.path().join("plugin-data.db")).unwrap();
    conn.execute(
        "UPDATE store_meta SET value = ?1 WHERE key = 'schema_version'",
        [version.to_string()],
    )
    .unwrap();
}

fn store_version(guard: &Guard) -> i64 {
    rusqlite::Connection::open(guard.path().join("plugin-data.db"))
        .unwrap()
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
        .parse()
        .unwrap()
}

fn defined() -> (PluginDataStore, Guard) {
    let (store, dir) = store();
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    (store, dir)
}

#[path = "concurrency.rs"]
mod concurrency;
#[path = "../unit/cross_process.rs"]
mod cross_process;
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

/// A database created before the store tracked its own version, and before `rev` existed.
///
/// A store this old is at version 0 by having no `store_meta` at all, not by being empty — which
/// is why migration 1 creates the registry with `IF NOT EXISTS` rather than assuming it is gone.
#[test]
fn a_database_from_before_rev_is_migrated_in_place() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("plugin-data.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE store_entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                namespace TEXT NOT NULL,
                entity TEXT NOT NULL,
                version INTEGER NOT NULL,
                definition TEXT NOT NULL,
                owner_plugin TEXT NOT NULL,
                UNIQUE(namespace, entity)
            );
            INSERT INTO store_entities (id, namespace, entity, version, definition, owner_plugin)
            VALUES (1, 'plugin:test', 'agent', 1,
                    '{"name":"agent","version":1,"attrs":[{"name":"project","kind":"Text","indexed":true,"full_text":false,"unique":false,"required":false,"default":null}]}',
                    'yinz:test');
            CREATE TABLE "e_1" (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uid TEXT UNIQUE,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                project TEXT
            );
            INSERT INTO "e_1" (uid, created_at, updated_at, project)
            VALUES ('old', 1, 1, 'written-before-rev');
            "#,
        )
        .unwrap();
    }

    let store = PluginDataStore::new(dir.path());
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    // The row is still there, still readable, and now carries a revision.
    assert_eq!(texts(&rows, "project"), vec!["written-before-rev"]);
    assert_eq!(column(&rows, "rev"), vec![DataValue::Int(1)]);

    // And it moves from there like any other row, rather than being stuck at its backfilled value.
    store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("uid", "old"))
                .set("project", "written-after")
                .build()],
        )
        .unwrap();
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(column(&rows, "rev"), vec![DataValue::Int(2)]);
}

/// Opening the same file twice must not re-run a migration that has already happened — an
/// `ALTER TABLE ADD COLUMN` is not idempotent, so the version record is what protects it.
#[test]
fn reopening_a_migrated_database_is_a_no_op() {
    let dir = tempdir().unwrap();
    let first = PluginDataStore::new(dir.path());
    first.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    drop(first);

    let second = PluginDataStore::new(dir.path());
    second
        .apply(NS, &[Mutation::put("agent").set("project", "yinz").build()])
        .unwrap();
    let rows = second.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(column(&rows, "rev"), vec![DataValue::Int(1)]);
}
