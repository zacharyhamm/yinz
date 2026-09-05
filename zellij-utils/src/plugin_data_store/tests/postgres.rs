//! The shared suite against Postgres: a throwaway schema per test in one shared database.
//!
//! Needs a server, so this module exists only with the `postgres-tests` feature, which
//! `cargo x postgres-test` turns on after bringing one up in docker. The URL comes from
//! `ZELLIJ_PLUGIN_DATA_TEST_URL`. Every test gets a schema of its own, created by the backend on
//! first use and dropped by the guard afterwards, so nextest's one-process-per-test can run the
//! suite as wide as the server's `max_connections` allows.

use std::str::FromStr;
use std::sync::Arc;

// The crate, not the backend module a glob import of `super::*` would otherwise find.
use ::postgres::{Config as PgConfig, NoTls};

use super::*;
use crate::data_access::DataAccess;
use crate::plugin_data_store::PostgresPluginDataBackend;

pub(crate) fn test_url() -> String {
    std::env::var("ZELLIJ_PLUGIN_DATA_TEST_URL").expect(
        "ZELLIJ_PLUGIN_DATA_TEST_URL is not set; run `cargo x postgres-test --build` and then \
         `cargo x postgres-test --test`, which sets it",
    )
}

/// Drops the test's schema when the test is done. Declared after the store in every fixture, so
/// that the store's connection is closed before the schema goes.
pub(crate) struct Guard {
    url: String,
    pub(crate) schema: String,
}

impl Guard {
    /// A schema nobody else is using, not yet created: the backend creates it on first use.
    pub(crate) fn fresh() -> Self {
        Self {
            url: test_url(),
            schema: format!("zellij_test_{}", uuid::Uuid::new_v4().simple()),
        }
    }

    /// The URL the backend under test connects with. It names the test's schema as the
    /// session's `application_name`, so that a test which needs to find its own connection in
    /// `pg_stat_activity` can, without touching the sessions of the tests running beside it.
    pub(crate) fn backend_url(&self) -> String {
        let separator = if self.url.contains('?') { '&' } else { '?' };
        format!("{}{}application_name={}", self.url, separator, self.schema)
    }

    pub(crate) fn backend(&self) -> PostgresPluginDataBackend {
        PostgresPluginDataBackend::new(&self.backend_url(), &self.schema)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let dropped = PgConfig::from_str(&self.url)
            .and_then(|config| config.connect(NoTls))
            .and_then(|mut client| {
                client.batch_execute(&format!(
                    "DROP SCHEMA IF EXISTS \"{}\" CASCADE",
                    self.schema
                ))
            });
        if let Err(e) = dropped {
            eprintln!("failed to drop test schema {}: {}", self.schema, e);
        }
    }
}

fn store() -> (PluginDataStore, Guard) {
    let guard = Guard::fresh();
    let store = PluginDataStore::from_backend(Arc::new(guard.backend()));
    (store, guard)
}

fn defined() -> (PluginDataStore, Guard) {
    let (store, guard) = store();
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    (store, guard)
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

/// A connection the server drops is reopened on the next call, and the call after the drop
/// reports the drop rather than hanging or panicking.
#[test]
fn a_lost_connection_is_reconnected_on_the_next_call() {
    let (store, guard) = defined();
    put(&store, "zellij", "before", 1);

    // Kill the store's session from another connection. Only this test's session: the
    // application name is the schema, which nothing else on the server is using.
    let mut killer = PgConfig::from_str(&guard.url)
        .unwrap()
        .connect(NoTls)
        .unwrap();
    let killed = killer
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE pid <> pg_backend_pid() AND application_name = $1",
            &[&guard.schema],
        )
        .unwrap();
    assert_eq!(killed, 1, "expected exactly the store's own session");

    // The first call after the kill may fail (the socket is dead) or succeed (the driver noticed
    // first); either way the one after it works.
    let _ = store.fetch(NS, &FetchRequest::entity("agent"));
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "summary"), vec!["before"]);
}

/// The store's own schema is versioned: a second backend pointed at the same schema finds the
/// tables in place and adopts them rather than recreating them.
#[test]
fn two_backends_share_one_schema() {
    let (store, guard) = defined();
    put(&store, "zellij", "shared", 1);
    let other = PluginDataStore::from_backend(Arc::new(guard.backend()));
    let rows = other.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "summary"), vec!["shared"]);
    // and redeclaring through the second one is idempotent
    let info = other.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    assert!(!info.created);
}

/// The reason this backend exists: `DataAccess::from_backends` with two real, different engines.
/// Plugin data goes to Postgres, the recording to the SQLite file in the data dir, and neither
/// store touches the other's home.
#[test]
fn from_backends_pairs_postgres_plugin_data_with_sqlite_recordings() {
    let guard = Guard::fresh();
    let dir = tempfile::tempdir().unwrap();
    let access = DataAccess::postgres(&guard.backend_url(), &guard.schema, dir.path());

    access
        .plugin_data()
        .define_entity(NS, OWNER, &agent_def(), false)
        .unwrap();
    access
        .plugin_data()
        .apply(
            NS,
            &[Mutation::put("agent")
                .set("project", "zellij")
                .set("summary", "in postgres")
                .build()],
        )
        .unwrap();
    let rows = access
        .plugin_data()
        .fetch(NS, &FetchRequest::entity("agent"))
        .unwrap();
    assert_eq!(texts(&rows, "summary"), vec!["in postgres"]);

    let mut session = access
        .recordings()
        .start_recording("proof", false, 1)
        .unwrap();
    // A recording is listed by its panes, so give it one and a byte of output.
    session
        .update_panes(
            &[crate::session_recording::PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "tab".to_owned(),
                title: "pane".to_owned(),
                cols: 80,
                rows: 24,
            }],
            1,
        )
        .unwrap();
    session
        .record_events(&[crate::session_recording::RecordedEvent::output(
            1,
            2,
            b"x".to_vec(),
        )])
        .unwrap();
    session.finish_pane(1, 3).unwrap();
    session.finish_session(4, 3).unwrap();
    drop(session);
    let recordings = access.recordings().list().unwrap();
    assert_eq!(recordings.len(), 1, "{:?}", recordings);

    assert!(dir.path().join("session-recordings.db").exists());
    assert!(
        !dir.path().join("plugin-data.db").exists(),
        "plugin data must not have touched the SQLite file"
    );
    drop(access);
}

#[test]
fn a_constraint_violation_includes_the_operation_and_server_explanation() {
    let (store, _guard) = store();
    let def = EntityDef::new("agent", 1).attr(AttrDef::text("project").unique());
    store.define_entity(NS, OWNER, &def, false).unwrap();
    let mutation = Mutation::put("agent").set("project", "duplicate").build();
    store.apply(NS, std::slice::from_ref(&mutation)).unwrap();
    let err = store.apply(NS, &[mutation]).unwrap_err();
    assert_err!(&err, DataError::Storage(_));
    let message = err.to_string();
    assert!(
        message.contains("failed to write a row of `agent`"),
        "{message}"
    );
    assert!(
        message.contains("duplicate key value violates unique constraint"),
        "{message}"
    );
    assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 1);
}
