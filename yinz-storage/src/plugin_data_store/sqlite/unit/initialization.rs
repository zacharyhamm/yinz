use super::*;

#[test]
fn concurrent_first_connections_share_wal_and_one_migration() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join(DB_FILENAME);
    let ready = std::sync::Barrier::new(12);
    std::thread::scope(|scope| {
        for _ in 0..12 {
            scope.spawn(|| {
                ready.wait();
                // This checks concurrent initialization, not latency under competing disk I/O.
                let connection =
                    open_db_before(&database, Instant::now() + Duration::from_secs(30)).unwrap();
                let mode: String = connection
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(mode, "wal");
                assert!(connection
                    .query_row("SELECT count(*) FROM store_entities", [], |row| row
                        .get::<_, i64>(0))
                    .is_ok());
            });
        }
    });
}

#[test]
fn persistent_initialization_lock_has_one_bounded_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join(DB_FILENAME);
    let lock = Connection::open(&database).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE; CREATE TABLE held (value INTEGER)")
        .unwrap();
    let started = Instant::now();
    let error = open_db_before(&database, started + Duration::from_millis(150)).unwrap_err();
    assert!(
        format!("{error:?}").contains("failed to enable WAL"),
        "{error:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(2));
    lock.execute_batch("ROLLBACK").unwrap();
    open_db(&database).unwrap();
}

#[test]
fn initialization_does_not_retry_unrelated_errors() {
    let connection = Connection::open_in_memory().unwrap();
    let mut attempts = 0;
    let result = initialize_before(&connection, Instant::now() + Duration::from_secs(5), |_| {
        attempts += 1;
        Err::<(), _>(rusqlite::Error::InvalidQuery)
    });
    assert!(matches!(result, Err(rusqlite::Error::InvalidQuery)));
    assert_eq!(attempts, 1);
}
