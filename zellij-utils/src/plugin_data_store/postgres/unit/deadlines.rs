use super::*;
use crate::plugin_data_store::tests::postgres::Guard;
use std::time::Instant;

fn assert_timeout<T: std::fmt::Debug>(started: Instant, result: Result<T, DataError>) {
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("timed out after five seconds"),
        "{error}"
    );
    assert!(started.elapsed() >= NETWORK_TIMEOUT);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

fn assert_disconnected(backend: &PostgresPluginDataBackend) {
    assert!(backend.client.lock().unwrap().is_none());
}

#[test]
fn blocked_bootstrap_and_query_discard_connections_and_reconnect() {
    let guard = Guard::fresh();
    let backend = guard.backend();
    backend.catalog(None).unwrap();
    let mut blocker = postgres::Client::connect(&guard.backend_url(), postgres::NoTls).unwrap();

    for (table, bootstrap) in [("store_meta", true), ("store_entities", false)] {
        if bootstrap {
            backend.client.lock().unwrap().take();
        }
        let mut tx = blocker.transaction().unwrap();
        tx.batch_execute(&format!(
            "LOCK TABLE \"{}\".{table} IN ACCESS EXCLUSIVE MODE",
            guard.schema
        ))
        .unwrap();
        let started = Instant::now();
        assert_timeout(started, backend.catalog(None));
        assert_disconnected(&backend);
        tx.rollback().unwrap();
        assert!(backend.catalog(None).unwrap().is_empty());
    }
}

#[test]
fn a_stalled_row_stream_has_one_deadline_and_reconnects() {
    let guard = Guard::fresh();
    let backend = guard.backend();
    backend.catalog(None).unwrap();
    let mut blocker = postgres::Client::connect(&guard.backend_url(), postgres::NoTls).unwrap();
    let key = (uuid::Uuid::new_v4().as_u128() as i64) & (i64::MAX - 1);
    let mut tx = blocker.transaction().unwrap();
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&(key + 1)])
        .unwrap();
    let mut consumed = 0;
    let started = Instant::now();
    let result = backend.with_client(|connection| {
        PgExec(connection).query_rows(
            // Large rows force a network flush before row 100 blocks on the held lock.
            "SELECT i::bigint, repeat('x', 8192), pg_advisory_xact_lock($1::bigint + (i = 100)::int) FROM generate_series(1, 100) i",
            &[DataValue::Int(key)],
            &[AttrKind::Int, AttrKind::Text],
            &mut |_| { consumed += 1; Ok(true) },
        )
    });
    assert_timeout(started, result);
    assert!(consumed > 0 && consumed < 100, "consumed {consumed} rows");
    assert_disconnected(&backend);
    tx.rollback().unwrap();
    backend.catalog(None).unwrap();
}

#[test]
fn a_timed_out_write_is_rolled_back_and_never_replayed() {
    let guard = Guard::fresh();
    let backend = guard.backend();
    backend.catalog(None).unwrap();
    let mut blocker = postgres::Client::connect(&guard.backend_url(), postgres::NoTls).unwrap();
    blocker
        .batch_execute(&format!(
            "CREATE TABLE \"{}\".deadline_write (value BIGINT)",
            guard.schema
        ))
        .unwrap();
    let mut tx = blocker.transaction().unwrap();
    tx.batch_execute(&format!(
        "LOCK TABLE \"{}\".store_entities IN ACCESS EXCLUSIVE MODE",
        guard.schema
    ))
    .unwrap();
    let started = Instant::now();
    assert_timeout(
        started,
        backend.in_transaction("test write", |exec| {
            exec.execute("INSERT INTO deadline_write VALUES (1)", &[])?;
            exec.query_i64("SELECT count(*) FROM store_entities", &[])
        }),
    );
    assert_disconnected(&backend);
    tx.rollback().unwrap();
    assert_eq!(
        backend
            .with_client(|connection| PgExec(connection)
                .query_i64("SELECT count(*) FROM deadline_write", &[]))
            .unwrap(),
        0
    );
}
