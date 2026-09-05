//! The Postgres backend: one connection behind one mutex, to a schema of its own.
//!
//! The same shape as [`super::sqlite`], with a socket where that has a file. The connection is
//! opened on first use and reopened after it drops; the schema and the registry table are created
//! on connect, under an advisory lock so that two zellij servers sharing a database cannot both
//! decide to create them.

pub(super) mod dialect;
mod value;

use std::cell::Cell;
use std::future::Future;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Config, NoTls};
use tokio_stream::StreamExt;

use crate::data_access::PluginDataBackend;
use crate::plugin_data::{
    validate_name, ApplyReport, AttrKind, DataEntityInfo, DataError, DataRows, DataSchemaInfo,
    DataValue, EntityDef, FetchRequest, Mutation, NameKind,
};

use super::dialect::Dialect;
use super::engine;
use super::executor::Executor;
use super::sql::{quote_ident, SqlContext, StorageError};
use super::store_migrations;

use self::dialect::POSTGRES;
use self::value::read_value;

/// The key every zellij takes with `pg_advisory_xact_lock` before changing the store's schema.
/// Arbitrary, and shared with any other application that happens to pick the same number, which
/// costs a little waiting and nothing else.
const SCHEMA_LOCK_KEY: i64 = 0x7a65_6c6c_696a_5f70; // "zellij_p"

/// The Postgres plugin-data backend. It connects lazily on first use.
pub struct PostgresPluginDataBackend {
    url: String,
    schema: String,
    client: Mutex<Option<Connection>>,
}

const NETWORK_TIMEOUT: Duration = Duration::from_secs(5);

/// DNS may use a blocking task; shutdown must not wait for it after a timeout.
struct Driver(Option<tokio::runtime::Runtime>);

impl Driver {
    fn run<T>(
        &self,
        what: &str,
        future: impl Future<Output = Result<T, DataError>>,
    ) -> Result<T, DataError> {
        self.0
            .as_ref()
            .expect("runtime exists until drop")
            .block_on(async {
                tokio::time::timeout(NETWORK_TIMEOUT, future)
                    .await
                    .map_err(|_| {
                        DataError::Storage(format!("{what}: timed out after five seconds"))
                    })?
            })
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

struct Connection {
    client: Client,
    driver: Driver,
    task: tokio::task::JoinHandle<()>,
    unusable: Cell<bool>,
}

impl Connection {
    fn run<T>(
        &self,
        what: &str,
        future: impl Future<Output = Result<T, DataError>>,
    ) -> Result<T, DataError> {
        let result = self.driver.run(what, future);
        // Discard storage errors too: a failed transaction must not leak into the next call.
        // Never replay an operation, especially a write whose outcome may be unknown.
        if matches!(result, Err(DataError::Storage(_))) {
            self.unusable.set(true);
            self.task.abort();
        }
        result
    }

    fn is_closed(&self) -> bool {
        self.unusable.get() || self.client.is_closed()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for PostgresPluginDataBackend {
    /// Shows where the store points and whether it is connected. The URL is printed without its
    /// password, and the mutex is only tried, never waited on — this is reached through
    /// `LoadingContext`'s derived `Debug`, and a formatter that can block on a lock held by a
    /// running query is a hazard in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresPluginDataBackend")
            .field("url", &redact_url(&self.url))
            .field("schema", &self.schema)
            .field(
                "connected",
                &self
                    .client
                    .try_lock()
                    .map(|guard| guard.is_some())
                    .unwrap_or(true),
            )
            .finish()
    }
}

/// A connection URL with its password removed, for logs.
///
/// A URL that does not parse as one (the `key=value` form is also accepted by Postgres) is not
/// echoed at all, since the password could be anywhere in it.
pub fn redact_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.password().is_some() {
                let _ = parsed.set_password(None);
            }
            if let Some(query) = parsed.query() {
                let retained = query
                    .split('&')
                    .filter(|pair| {
                        !url::form_urlencoded::parse(pair.as_bytes())
                            .any(|(key, _)| key == "password")
                    })
                    .collect::<Vec<_>>()
                    .join("&");
                parsed.set_query(if retained.is_empty() {
                    None
                } else {
                    Some(&retained)
                });
            }
            parsed.to_string()
        },
        Err(_) => "<postgres connection string>".to_owned(),
    }
}

impl PostgresPluginDataBackend {
    /// Point the store at a database and a schema within it. Connects to nothing.
    ///
    /// `url` is anything `tokio_postgres::Config` parses: a `postgres://user:pass@host/db` URL or a
    /// `key=value` string. `schema` must satisfy the same rule as any store name
    /// (`^[a-z][a-z0-9_]{0,62}$`), which is also inside Postgres's identifier limit.
    pub fn new(url: &str, schema: &str) -> Self {
        Self {
            url: url.to_owned(),
            schema: schema.to_owned(),
            client: Mutex::new(None),
        }
    }

    /// Run `f` against the connection, opening it on first use and reopening it after it drops.
    ///
    /// One connection behind one mutex, as for SQLite: handlers run on the calling plugin's
    /// pinned executor thread where blocking is expected. That thread must not be a tokio worker,
    /// because the backend drives its own runtime with `block_on`, which panics
    /// inside another — so this checks, and answers with an error rather than a panic.
    fn with_client<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(DataError::Storage(
                "the Postgres plugin-data backend was called on an async runtime thread; it \
                 blocks on its own runtime and cannot do so inside another"
                    .to_owned(),
            ));
        }
        let mut guard = self
            .client
            .lock()
            .map_err(|_| DataError::Storage("the plugin data store lock is poisoned".to_owned()))?;
        if guard.as_ref().is_some_and(Connection::is_closed) {
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some(self.connect()?);
        }
        let client = guard
            .as_mut()
            .ok_or_else(|| DataError::Storage("the plugin data store is not open".to_owned()))?;
        let result = f(client);
        // A failed call on a dead socket is not retried, but the next call gets a fresh one.
        if result.is_err() && client.is_closed() {
            *guard = None;
        }
        result
    }

    fn connect(&self) -> Result<Connection, DataError> {
        validate_name(&self.schema, NameKind::Namespace).map_err(|e| {
            DataError::Storage(format!(
                "`{}` is not a usable Postgres schema name for the plugin data store: {}",
                self.schema, e
            ))
        })?;
        let mut config = Config::from_str(&self.url).map_err(|e| {
            e.into_data_error(&format!(
                "the plugin data store's Postgres URL {} does not parse",
                redact_url(&self.url),
            ))
        })?;
        if config.get_application_name().is_none() {
            config.application_name("zellij");
        }
        let driver = Driver(Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    DataError::Storage(format!("failed to create Postgres runtime: {e}"))
                })?,
        ));
        let what = format!("failed to connect to {}", redact_url(&self.url));
        let (client, connection) =
            driver.run(&what, async { config.connect(NoTls).await.ctx(&what) })?;
        let task = driver
            .0
            .as_ref()
            .expect("runtime exists")
            .spawn(async move {
                let _ = connection.await;
            });
        let mut connection = Connection {
            client,
            driver,
            task,
            unusable: Cell::new(false),
        };
        connection.driver.run(
            "failed to bootstrap the plugin data store",
            bootstrap(&mut connection.client, &self.schema),
        )?;
        // Not through `self.in_transaction`: this runs inside `with_client`, which is holding the
        // connection mutex, and taking it again would deadlock. The advisory lock is what makes
        // this safe against another server connecting at the same moment.
        in_transaction_on(&mut connection, "store migration", |exec| {
            exec.lock_schema()?;
            store_migrations::migrate_store(exec)
        })?;
        Ok(connection)
    }

    /// Run `f` inside a transaction, committing on success.
    fn in_transaction<T>(
        &self,
        what: &str,
        f: impl FnOnce(&mut dyn Executor) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        self.with_client(|client| in_transaction_on(client, what, f))
    }
}

/// [`PostgresPluginDataBackend::in_transaction`] against a connection already in hand.
///
/// Split out so that `connect` can open a transaction on the connection it is still building,
/// before that connection is reachable through the backend's mutex.
fn in_transaction_on<T>(
    client: &mut Connection,
    what: &str,
    f: impl FnOnce(&mut dyn Executor) -> Result<T, DataError>,
) -> Result<T, DataError> {
    client.run("failed to begin transaction", async {
        client
            .client
            .batch_execute("BEGIN")
            .await
            .with_ctx(|| format!("failed to begin a {what} transaction"))
    })?;
    let result = f(&mut PgExec(client));
    if result.is_ok() || !client.is_closed() {
        let command = if result.is_ok() { "COMMIT" } else { "ROLLBACK" };
        client.run(command, async {
            client
                .client
                .batch_execute(command)
                .await
                .with_ctx(|| format!("failed to {command} the {what}"))
        })?;
    }
    result
}

impl PluginDataBackend for PostgresPluginDataBackend {
    fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        // DDL is transactional on Postgres, so a refused migration leaves nothing half-applied.
        self.in_transaction("schema change", |exec| {
            engine::define_entity(exec, namespace, owner_plugin, def, allow_destructive)
        })
    }

    fn catalog(&self, namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
        self.with_client(|client| engine::catalog(&mut PgExec(client), namespace))
    }

    fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
        self.with_client(|client| engine::fetch(&mut PgExec(client), namespace, request))
    }

    fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
        self.with_client(|client| engine::count(&mut PgExec(client), namespace, request))
    }

    /// Apply a batch of mutations in one transaction, all or nothing.
    fn apply(&self, namespace: &str, mutations: &[Mutation]) -> Result<ApplyReport, DataError> {
        if mutations.is_empty() {
            return Ok(ApplyReport::default());
        }
        self.in_transaction("write", |exec| engine::apply(exec, namespace, mutations))
    }
}

/// Make sure the schema exists, then point the session at it.
///
/// Runs under the advisory lock: `CREATE SCHEMA IF NOT EXISTS` races with itself across two
/// connections — both see nothing, one of them fails.
///
/// Only the schema. The store's own tables are brought up to date afterwards by
/// [`store_migrations::migrate_store`], on the synchronous side, because those steps are shared
/// with SQLite and are written against [`Executor`] rather than against a Postgres client.
async fn bootstrap(client: &mut Client, schema: &str) -> Result<(), DataError> {
    let schema_q = quote_ident(schema);
    let tx = client
        .transaction()
        .await
        .ctx("failed to begin the bootstrap transaction")?;
    tx.batch_execute(&format!(
        "SELECT pg_advisory_xact_lock({SCHEMA_LOCK_KEY});
         CREATE SCHEMA IF NOT EXISTS {schema_q};"
    ))
    .await
    .ctx("failed to prepare the plugin data store's schema")?;
    tx.commit().await.ctx("failed to commit the bootstrap")?;
    client
        .batch_execute(&format!("SET search_path TO {schema_q};"))
        .await
        .ctx("failed to select the store's schema")?;
    Ok(())
}

/// The synchronous engine's view of the connection and its current transaction.
struct PgExec<'a>(&'a Connection);

impl Executor for PgExec<'_> {
    fn dialect(&self) -> &'static dyn Dialect {
        &POSTGRES
    }

    fn lock_schema(&mut self) -> Result<(), DataError> {
        self.execute_batch(&format!("SELECT pg_advisory_xact_lock({SCHEMA_LOCK_KEY});"))
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DataError> {
        self.0.run("statement failed", async {
            self.0
                .client
                .batch_execute(sql)
                .await
                .ctx("statement failed")
        })
    }

    fn execute(&mut self, sql: &str, params: &[DataValue]) -> Result<u64, DataError> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as _).collect();
        self.0.run("statement failed", async {
            self.0
                .client
                .execute(sql, &refs)
                .await
                .ctx("statement failed")
        })
    }

    fn query_i64(&mut self, sql: &str, params: &[DataValue]) -> Result<i64, DataError> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as _).collect();
        self.0.run("query failed", async {
            let row = self
                .0
                .client
                .query_one(sql, &refs)
                .await
                .ctx("query failed")?;
            row.try_get::<_, i64>(0)
                .ctx("failed to read the query's integer")
        })
    }

    fn query_rows(
        &mut self,
        sql: &str,
        params: &[DataValue],
        kinds: &[AttrKind],
        sink: &mut dyn FnMut(Vec<DataValue>) -> Result<bool, DataError>,
    ) -> Result<(), DataError> {
        self.0.run("query failed", async {
            let rows = self
                .0
                .client
                .query_raw(sql, params.iter())
                .await
                .ctx("failed to run the query")?;
            tokio::pin!(rows);
            while let Some(row) = rows.next().await {
                let row = row.ctx("failed to read a row")?;
                let mut values = Vec::with_capacity(kinds.len());
                for index in 0..kinds.len() {
                    values.push(read_value(&row, index)?);
                }
                if !sink(values)? {
                    break;
                }
            }
            Ok(())
        })
    }
}

#[cfg(all(test, feature = "postgres-tests"))]
#[path = "unit/deadlines.rs"]
mod deadline_tests;
