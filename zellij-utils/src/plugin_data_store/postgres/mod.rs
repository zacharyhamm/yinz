//! The Postgres backend: one connection behind one mutex, to a schema of its own.
//!
//! The same shape as [`super::sqlite`], with a socket where that has a file. The connection is
//! opened on first use and reopened after it drops; the schema and the registry table are created
//! on connect, under an advisory lock so that two zellij servers sharing a database cannot both
//! decide to create them.

pub(super) mod dialect;
mod value;

use std::str::FromStr;
use std::sync::Mutex;

use postgres::types::ToSql;
use postgres::{Client, Config, GenericClient, NoTls};

use crate::data_access::PluginDataBackend;
use crate::plugin_data::{
    validate_name, ApplyReport, AttrKind, DataEntityInfo, DataError, DataRows, DataSchemaInfo,
    DataValue, EntityDef, FetchRequest, Mutation, NameKind,
};

use super::dialect::Dialect;
use super::engine;
use super::executor::Executor;
use super::sql::{quote_ident, SqlContext};

use self::dialect::POSTGRES;
use self::value::read_value;

/// The key every zellij takes with `pg_advisory_xact_lock` before changing the store's schema.
/// Arbitrary, and shared with any other application that happens to pick the same number, which
/// costs a little waiting and nothing else.
const SCHEMA_LOCK_KEY: i64 = 0x7a65_6c6c_696a_5f70; // "zellij_p"

/// The store's own tables, in the order they came to exist. `bootstrap` applies the ones past the
/// recorded version and records the new one, so a database created by an older zellij is brought
/// forward on connect.
const MIGRATIONS: &[&str] = &[
    // 1: the entity registry.
    r#"CREATE TABLE store_entities (
           id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
           namespace TEXT COLLATE "C" NOT NULL,
           entity TEXT COLLATE "C" NOT NULL,
           version BIGINT NOT NULL,
           definition TEXT NOT NULL,
           owner_plugin TEXT NOT NULL,
           UNIQUE (namespace, entity)
       );"#,
];

/// The Postgres plugin-data backend. It connects lazily on first use.
pub struct PostgresPluginDataBackend {
    url: String,
    schema: String,
    client: Mutex<Option<Client>>,
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
    /// `url` is anything `postgres::Config` parses: a `postgres://user:pass@host/db` URL or a
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
    /// because the sync `postgres` client drives its own runtime with `block_on`, which panics
    /// inside another — so this checks, and answers with an error rather than a panic.
    fn with_client<T>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T, DataError>,
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
        if guard.as_ref().is_some_and(Client::is_closed) {
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

    fn connect(&self) -> Result<Client, DataError> {
        validate_name(&self.schema, NameKind::Namespace).map_err(|e| {
            DataError::Storage(format!(
                "`{}` is not a usable Postgres schema name for the plugin data store: {}",
                self.schema, e
            ))
        })?;
        let mut config = Config::from_str(&self.url).map_err(|e| {
            DataError::Storage(format!(
                "the plugin data store's Postgres URL {} does not parse: {}",
                redact_url(&self.url),
                e
            ))
        })?;
        if config.get_application_name().is_none() {
            config.application_name("zellij");
        }
        let mut client = config
            .connect(NoTls)
            .with_ctx(|| format!("failed to connect to {}", redact_url(&self.url)))?;
        bootstrap(&mut client, &self.schema)?;
        Ok(client)
    }

    /// Run `f` inside a transaction, committing on success.
    fn in_transaction<T>(
        &self,
        what: &str,
        f: impl FnOnce(&mut dyn Executor) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        self.with_client(|client| {
            let mut tx = client
                .transaction()
                .with_ctx(|| format!("failed to begin a {} transaction", what))?;
            let result = f(&mut PgExec(&mut tx))?;
            tx.commit()
                .with_ctx(|| format!("failed to commit the {}", what))?;
            Ok(result)
        })
    }
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

/// Make sure the schema and the store's own tables exist and are current, then point the session
/// at the schema.
///
/// Runs under the advisory lock from start to finish. `CREATE SCHEMA IF NOT EXISTS` races with
/// itself across two connections — both see nothing, one of them fails — and so does anything
/// that follows it.
fn bootstrap(client: &mut Client, schema: &str) -> Result<(), DataError> {
    let schema_q = quote_ident(schema);
    let mut tx = client
        .transaction()
        .ctx("failed to begin the bootstrap transaction")?;
    tx.batch_execute(&format!(
        "SELECT pg_advisory_xact_lock({SCHEMA_LOCK_KEY});
         CREATE SCHEMA IF NOT EXISTS {schema_q};
         SET LOCAL search_path TO {schema_q};
         CREATE TABLE IF NOT EXISTS store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);"
    ))
    .ctx("failed to prepare the plugin data store's schema")?;
    let version: i64 = tx
        .query_opt(
            "SELECT value FROM store_meta WHERE key = 'schema_version'",
            &[],
        )
        .ctx("failed to read the store's schema version")?
        .map(|row| row.get::<_, String>(0))
        .map(|value| value.parse::<i64>().unwrap_or(0))
        .unwrap_or(0);
    let current = MIGRATIONS.len() as i64;
    if version > current {
        return Err(DataError::Storage(format!(
            "the plugin data store in schema `{}` is at version {}, newer than this zellij's {}",
            schema, version, current
        )));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        tx.batch_execute(migration)
            .with_ctx(|| format!("failed to apply store migration {}", index + 1))?;
    }
    if version < current {
        tx.execute(
            "INSERT INTO store_meta (key, value) VALUES ('schema_version', $1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            &[&current.to_string()],
        )
        .ctx("failed to record the store's schema version")?;
    }
    tx.commit().ctx("failed to commit the bootstrap")?;
    client
        .batch_execute(&format!("SET search_path TO {schema_q};"))
        .ctx("failed to select the store's schema")?;
    Ok(())
}

/// The engine's view of a `postgres` client or transaction.
struct PgExec<'a, C: GenericClient>(&'a mut C);

impl<C: GenericClient> Executor for PgExec<'_, C> {
    fn dialect(&self) -> &'static dyn Dialect {
        &POSTGRES
    }

    fn lock_schema(&mut self) -> Result<(), DataError> {
        self.0
            .batch_execute(&format!("SELECT pg_advisory_xact_lock({SCHEMA_LOCK_KEY});"))
            .ctx("failed to lock the store's schema")
    }

    fn execute_batch(&mut self, sql: &str) -> Result<(), DataError> {
        self.0.batch_execute(sql).ctx("statement failed")
    }

    fn execute(&mut self, sql: &str, params: &[DataValue]) -> Result<u64, DataError> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as _).collect();
        self.0.execute(sql, &refs).ctx("statement failed")
    }

    fn query_i64(&mut self, sql: &str, params: &[DataValue]) -> Result<i64, DataError> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as _).collect();
        let row = self.0.query_one(sql, &refs).ctx("query failed")?;
        row.try_get::<_, i64>(0)
            .ctx("failed to read the query's integer")
    }

    fn query_rows(
        &mut self,
        sql: &str,
        params: &[DataValue],
        kinds: &[AttrKind],
        sink: &mut dyn FnMut(Vec<DataValue>) -> Result<bool, DataError>,
    ) -> Result<(), DataError> {
        use postgres::fallible_iterator::FallibleIterator;
        let mut rows = self
            .0
            .query_raw(sql, params.iter())
            .ctx("failed to run the query")?;
        while let Some(row) = rows.next().ctx("failed to read a row")? {
            let mut values = Vec::with_capacity(kinds.len());
            for index in 0..kinds.len() {
                values.push(read_value(&row, index)?);
            }
            if !sink(values)? {
                break;
            }
        }
        Ok(())
    }
}
