//! The SQLite backend: one file in the data dir, one connection behind one mutex.

pub(super) mod dialect;
mod value;

use crate::plugin_data_store::value::SqlValue;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::{Connection, TransactionBehavior};

use crate::data_access::PluginDataBackend;
use yinz_types::plugin_data::{
    ApplyReport, AttrKind, DataEntityInfo, DataError, DataRows, DataSchemaInfo, DataValue,
    EntityDef, FetchRequest, Mutation,
};

use super::dialect::Dialect;
use super::engine;
use super::executor::Executor;
use super::registry;
use super::sql::{storage_err, SqlContext};
use super::store_migrations;

use self::dialect::SQLITE;
use self::value::read_value;

/// Filename under the yinz data dir.
const DB_FILENAME: &str = "plugin-data.db";

/// The SQLite plugin-data backend. It opens its database lazily on first use.
pub struct SqlitePluginDataBackend {
    db_path: PathBuf,
    connection: Mutex<Option<Connection>>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl std::fmt::Debug for SqlitePluginDataBackend {
    /// Shows the path and whether the file has been opened. Deliberately does not touch the
    /// connection mutex: this is reached through `LoadingContext`'s derived `Debug`, and a
    /// formatter that can block on a lock held by a running query is a hazard in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePluginDataBackend")
            .field("db_path", &self.db_path)
            .field(
                "open",
                &self
                    .connection
                    .try_lock()
                    .map(|guard| guard.is_some())
                    .unwrap_or(true),
            )
            .finish()
    }
}

impl SqlitePluginDataBackend {
    /// Point the store at a data dir. Opens nothing.
    pub fn new(data_dir: &Path) -> Self {
        Self::with_clock(data_dir, super::value::now_ms)
    }

    /// Supply a wall clock for deterministic store and guest-runtime comparisons.
    pub fn with_clock(data_dir: &Path, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            db_path: data_dir.join(DB_FILENAME),
            connection: Mutex::new(None),
            clock: Box::new(clock),
        }
    }

    /// Run `f` against the connection, opening it on first use.
    ///
    /// One connection behind one mutex. Handlers run on the calling plugin's pinned executor
    /// thread where blocking is expected, and the alternative — a connection per thread — would
    /// multiply the writers contending for the same file without making any single call faster.
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| DataError::Storage("the plugin data store lock is poisoned".to_owned()))?;
        if guard.is_none() {
            *guard = Some(open_db(&self.db_path)?);
        }
        let conn = guard
            .as_mut()
            .ok_or_else(|| DataError::Storage("the plugin data store is not open".to_owned()))?;
        f(conn)
    }

    /// Run `f` inside a write transaction, committing on success.
    fn in_transaction<T>(
        &self,
        what: &str,
        f: impl FnOnce(&mut dyn Executor) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .with_ctx(|| format!("failed to begin a {} transaction", what))?;
            let result = f(&mut SqliteExec(&tx))?;
            tx.commit()
                .with_ctx(|| format!("failed to commit the {}", what))?;
            Ok(result)
        })
    }
}

impl PluginDataBackend for SqlitePluginDataBackend {
    fn external_version(&self) -> Result<Option<u64>, DataError> {
        self.with_conn(|conn| {
            conn.pragma_query_value(None, "data_version", |row| row.get(0))
                .map(Some)
                .ctx("failed to read the plugin data version")
        })
    }

    fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        // Repeated declarations must not make readers wait for another server's writer.
        if !allow_destructive
            && self.with_conn(|conn| {
                Ok(
                    registry::load_entity(&mut SqliteExec(conn), namespace, &def.name)?
                        .is_some_and(|existing| existing.def == *def),
                )
            })?
        {
            return Ok(registry::schema_info(namespace, def, false));
        }
        // Recheck under the write lock in case another server changed the schema meanwhile.
        self.in_transaction("schema change", |exec| {
            engine::define_entity(exec, namespace, owner_plugin, def, allow_destructive)
        })
    }

    fn catalog(&self, namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
        self.with_conn(|conn| engine::catalog(&mut SqliteExec(conn), namespace))
    }

    fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
        self.with_conn(|conn| engine::fetch(&mut SqliteExec(conn), namespace, request))
    }

    fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
        self.with_conn(|conn| engine::count(&mut SqliteExec(conn), namespace, request))
    }

    /// Apply a batch of mutations in one transaction, all or nothing.
    fn apply(&self, namespace: &str, mutations: &[Mutation]) -> Result<ApplyReport, DataError> {
        if mutations.is_empty() {
            return Ok(ApplyReport::default());
        }
        self.in_transaction("write", |exec| {
            engine::apply(exec, namespace, mutations, (self.clock)())
        })
    }
}

/// Open the store's file, set its pragmas, and make sure the registry table exists.
fn open_db(db_path: &Path) -> Result<Connection, DataError> {
    open_db_before(db_path, Instant::now() + Duration::from_secs(5))
}

fn open_db_before(db_path: &Path, deadline: Instant) -> Result<Connection, DataError> {
    let started = Instant::now();
    log::info!("startup: database open begin path={}", db_path.display());
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| storage_err(&format!("failed to create {:?}", parent), e))?;
    }
    let conn = Connection::open(db_path).with_ctx(|| format!("failed to open {:?}", db_path))?;
    conn.busy_timeout(deadline.saturating_duration_since(Instant::now()))
        .ctx("failed to set initialization busy timeout")?;
    log::info!(
        "startup: database connection opened path={} elapsed={:?}; enabling WAL",
        db_path.display(),
        started.elapsed()
    );
    // WAL and a busy timeout let concurrent yinz servers share this file in the data dir.
    initialize_before(&conn, deadline, |conn| {
        conn.pragma_update(None, "journal_mode", "WAL")
    })
    .ctx("failed to enable WAL")?;
    log::info!(
        "startup: database WAL enabled path={} elapsed={:?}",
        db_path.display(),
        started.elapsed()
    );
    initialize_before(&conn, deadline, |conn| {
        conn.pragma_update(None, "foreign_keys", "ON")
    })
    .ctx("failed to enable foreign keys")?;
    initialize_before(&conn, deadline, |conn| {
        conn.pragma_update(None, "synchronous", "NORMAL")
    })
    .ctx("failed to set synchronous")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(db_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(db_path, perms);
        }
    }
    // The store's own tables, brought up to date in one write transaction. `BEGIN IMMEDIATE`
    // takes the database's single write lock for its whole length, which is what stops two
    // servers opening this file at once from both deciding a migration has not run.
    log::info!(
        "startup: database migration transaction begin path={} elapsed={:?}",
        db_path.display(),
        started.elapsed()
    );
    let tx = initialize_before(&conn, deadline, |conn| {
        rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
    })
    .ctx("failed to begin the store migration transaction")?;
    log::info!(
        "startup: database migration lock acquired path={} elapsed={:?}",
        db_path.display(),
        started.elapsed()
    );
    store_migrations::migrate_store(&mut SqliteExec(&tx))?;
    log::info!(
        "startup: database migration commit begin path={} elapsed={:?}",
        db_path.display(),
        started.elapsed()
    );
    tx.commit().ctx("failed to commit the store migration")?;
    conn.busy_timeout(Duration::from_secs(5))
        .ctx("failed to set busy timeout")?;
    log::info!(
        "startup: database open complete path={} elapsed={:?}",
        db_path.display(),
        started.elapsed()
    );
    Ok(conn)
}

// Retry only initialization operations that have not begun mutating the store.
fn initialize_before<'a, T>(
    conn: &'a Connection,
    deadline: Instant,
    mut operation: impl FnMut(&'a Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    loop {
        conn.busy_timeout(deadline.saturating_duration_since(Instant::now()))?;
        match operation(conn) {
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(error);
                }
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            },
            result => return result,
        }
    }
}

#[cfg(test)]
#[path = "unit/initialization.rs"]
mod initialization_tests;

/// The engine's view of a `rusqlite` connection or transaction (which derefs to one).
struct SqliteExec<'a>(&'a Connection);

impl Executor for SqliteExec<'_> {
    fn dialect(&self) -> &'static dyn Dialect {
        &SQLITE
    }

    /// Nothing to do: the `BEGIN IMMEDIATE` that opened the transaction already holds the
    /// database's one write lock.
    fn lock_schema(&mut self) -> Result<(), DataError> {
        Ok(())
    }

    fn lock_writes(&mut self, _tables: &[String]) -> Result<(), DataError> {
        Ok(())
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DataError> {
        self.0.execute_batch(sql).ctx("statement failed")
    }

    fn execute(&mut self, sql: &str, params: &[DataValue]) -> Result<u64, DataError> {
        let changed = self
            .0
            .execute(sql, rusqlite::params_from_iter(params.iter().map(SqlValue)))
            .ctx("statement failed")?;
        Ok(changed as u64)
    }

    fn query_i64(&mut self, sql: &str, params: &[DataValue]) -> Result<i64, DataError> {
        self.0
            .query_row(
                sql,
                rusqlite::params_from_iter(params.iter().map(SqlValue)),
                |row| row.get(0),
            )
            .ctx("query failed")
    }

    fn query_rows(
        &mut self,
        sql: &str,
        params: &[DataValue],
        kinds: &[AttrKind],
        sink: &mut dyn FnMut(Vec<DataValue>) -> Result<bool, DataError>,
    ) -> Result<(), DataError> {
        let mut stmt = self.0.prepare(sql).ctx("failed to prepare the query")?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params.iter().map(SqlValue)))
            .ctx("failed to run the query")?;
        while let Some(row) = rows.next().ctx("failed to read a row")? {
            let mut values = Vec::with_capacity(kinds.len());
            for (index, kind) in kinds.iter().enumerate() {
                values.push(read_value(row, index, *kind)?);
            }
            if !sink(values)? {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit/snapshots.rs"]
mod snapshot_tests;

#[cfg(test)]
#[path = "unit/schema.rs"]
mod schema_tests;
