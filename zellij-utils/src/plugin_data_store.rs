//! The SQLite engine behind the plugin data store.
//!
//! Host-only: `rusqlite` cannot build for `wasm32-wasip1`, and even if it could, plugins run under
//! `wasmi` — a pure interpreter with a 16 MiB linear memory configured to trap rather than fail
//! when it can't grow. SQLite's page cache does not belong there. The types plugins speak in live
//! in [`crate::plugin_data`], which is compiled for both sides; this module turns them into SQL.
//!
//! # Layout
//!
//! One real table per `(namespace, entity)`, named `e_<n>` after its row in the `store_entities`
//! registry. No plugin-supplied string is ever interpolated into a table name. Attribute names
//! *are* interpolated as column names, which is why [`crate::plugin_data::validate_name`] is the
//! boundary that keeps a plugin from writing SQL, and why every entry point runs it before
//! composing anything.
//!
//! Full-text search uses an external-content FTS5 table kept in sync by triggers. That differs
//! deliberately from [`crate::session_recording`], whose watermark table exists only because
//! indexing terminal output needs a `vte` replay that cannot run inside a trigger. Here the
//! indexed text is already a column, so triggers keep the index exact for free.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, ToSql, TransactionBehavior};

use crate::data::ClientId;
use crate::plugin_data::{
    ApplyReport, AttrDef, AttrKind, DataError, DataRows, DataSchemaInfo, DataValue,
    DataWatchHandle, EntityDef, FetchRequest, Mutation, Pred, WatchMode, MAX_FETCH_BYTES,
    SHARED_NAMESPACE_PREFIX,
};
use crate::session_recording::fts_phrase;

/// The store's own columns, present on every entity table.
const STORE_COLUMNS: &[&str] = &["id", "uid", "created_at", "updated_at"];

/// Filename under the zellij data dir.
const DB_FILENAME: &str = "plugin-data.db";

/// What a batch of mutations did, plus which entities it touched.
///
/// The touched set is what watch re-evaluation filters on, so that a write to one entity doesn't
/// re-run every watch in the process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    pub report: ApplyReport,
    pub touched_entities: BTreeSet<String>,
}

/// The namespace a plugin gets when it doesn't name one.
///
/// Derived from the plugin's URL the same way `plugin_own_cache_dir` is, so it is stable across
/// instances, across clients and across sessions — and so a debug and a release build of the same
/// plugin get different namespaces, exactly as they get different permission entries.
pub fn private_namespace(plugin_url: &str) -> String {
    let sanitized: String = plugin_url
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("plugin:{}", sanitized)
}

/// Whether a resolved namespace is one shared between plugins.
pub fn is_shared_namespace(namespace: &str) -> bool {
    namespace.starts_with(SHARED_NAMESPACE_PREFIX)
}

impl ToSql for DataValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            DataValue::Null => ToSqlOutput::Owned(Value::Null),
            DataValue::Int(i) => ToSqlOutput::Owned(Value::Integer(*i)),
            DataValue::Real(f) => ToSqlOutput::Owned(Value::Real(*f)),
            // SQLite has no boolean storage class.
            DataValue::Bool(b) => ToSqlOutput::Owned(Value::Integer(*b as i64)),
            DataValue::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            DataValue::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

fn storage_err(context: &str, e: impl std::fmt::Display) -> DataError {
    DataError::Storage(format!("{}: {}", context, e))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A stored entity, as read back from the registry.
#[derive(Debug, Clone)]
struct EntitySchema {
    row_id: i64,
    def: EntityDef,
    owner_plugin: String,
}

impl EntitySchema {
    fn table(&self) -> String {
        format!("e_{}", self.row_id)
    }

    fn fts_table(&self) -> String {
        format!("e_{}_fts", self.row_id)
    }

    fn attr_index(&self, attr: &str) -> String {
        format!("e_{}_a_{}", self.row_id, attr)
    }

    fn unique_index(&self, attr: &str) -> String {
        format!("e_{}_u_{}", self.row_id, attr)
    }

    fn attr(&self, name: &str) -> Option<&AttrDef> {
        self.def.attr_named(name)
    }

    /// The kind of a column, or `None` for one of the store's own columns.
    fn column_kind(&self, name: &str) -> Option<AttrKind> {
        self.attr(name).map(|a| a.kind)
    }

    /// Reject a column name that is neither a declared attribute nor one of the store's own.
    fn require_column(&self, name: &str) -> Result<(), DataError> {
        if STORE_COLUMNS.contains(&name) || self.attr(name).is_some() {
            return Ok(());
        }
        Err(DataError::InvalidQuery(format!(
            "`{}` is not a declared attribute of `{}`; it has {}",
            name,
            self.def.name,
            if self.def.attrs.is_empty() {
                "none".to_owned()
            } else {
                self.def
                    .attrs
                    .iter()
                    .map(|a| format!("`{}`", a.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )))
    }
}

/// The plugin data store: one SQLite file shared by every plugin, partitioned by namespace.
///
/// Cheap to construct and does no IO until first use, so a session whose plugins never touch the
/// store never creates the file.
/// A standing fetch, and what its results looked like the last time anyone checked.
#[derive(Debug, Clone)]
struct Watch {
    watch_id: u64,
    /// The server's `PluginId`, kept as its underlying type so that this module needs nothing from
    /// `zellij-server`.
    plugin_id: u32,
    client_id: ClientId,
    namespace: String,
    entity: String,
    request: FetchRequest,
    last_digest: u64,
}

/// A watch whose results have changed, and the rows to send it.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchChange {
    pub watch_id: u64,
    pub plugin_id: u32,
    pub client_id: ClientId,
    pub rows: DataRows,
}

pub struct PluginDataStore {
    db_path: PathBuf,
    connection: Mutex<Option<Connection>>,
    /// Registered watches, and the id to hand the next one.
    ///
    /// Not persisted: a watch belongs to a running plugin instance, and a plugin that has been
    /// reloaded has to register again anyway.
    ///
    /// Locked *inside* the connection lock wherever both are needed, so that the two are always
    /// taken in the same order and cannot deadlock against each other.
    watches: Mutex<(Vec<Watch>, u64)>,
}

impl std::fmt::Debug for PluginDataStore {
    /// Shows the path and whether the file has been opened. Deliberately does not touch the
    /// connection mutex: this is reached through `LoadingContext`'s derived `Debug`, and a
    /// formatter that can block on a lock held by a running query is a hazard in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDataStore")
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

impl PluginDataStore {
    /// Point the store at a data dir. Opens nothing.
    pub fn new(data_dir: &Path) -> Self {
        PluginDataStore {
            db_path: data_dir.join(DB_FILENAME),
            connection: Mutex::new(None),
            watches: Mutex::new((Vec::new(), 1)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
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
            *guard = Some(self.open()?);
        }
        let conn = guard
            .as_mut()
            .ok_or_else(|| DataError::Storage("the plugin data store is not open".to_owned()))?;
        f(conn)
    }

    fn open(&self) -> Result<Connection, DataError> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| storage_err(&format!("failed to create {:?}", parent), e))?;
        }
        let conn = Connection::open(&self.db_path)
            .map_err(|e| storage_err(&format!("failed to open {:?}", self.db_path), e))?;
        // The same pragma set as the recording store. WAL and a busy timeout matter more here than
        // there: this file lives in the data dir rather than a session cache dir, so two zellij
        // servers running at once share it.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| storage_err("failed to enable WAL", e))?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| storage_err("failed to set busy_timeout", e))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| storage_err("failed to enable foreign keys", e))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| storage_err("failed to set synchronous", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&self.db_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&self.db_path, perms);
            }
        }
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS store_entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                namespace TEXT NOT NULL,
                entity TEXT NOT NULL,
                version INTEGER NOT NULL,
                definition TEXT NOT NULL,
                owner_plugin TEXT NOT NULL,
                UNIQUE(namespace, entity)
            );
            "#,
        )
        .map_err(|e| storage_err("failed to create the entity registry", e))?;
        Ok(conn)
    }

    // ---------------------------------------------------------------- schema

    /// Declare an entity, creating or migrating its table to match.
    ///
    /// Idempotent: re-declaring an identical definition touches nothing. The host diffs the stored
    /// definition against this one rather than comparing versions, so a plugin that forgets to bump
    /// `version` still gets its new attributes.
    pub fn define_entity(
        &self,
        namespace: &str,
        owner_plugin: &str,
        def: &EntityDef,
        allow_destructive: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        def.validate()?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| storage_err("failed to begin a schema transaction", e))?;
            let info = match load_entity(&tx, namespace, &def.name)? {
                Some(existing) => migrate_entity(
                    &tx,
                    namespace,
                    owner_plugin,
                    &existing,
                    def,
                    allow_destructive,
                )?,
                None => create_entity(&tx, namespace, owner_plugin, def)?,
            };
            tx.commit()
                .map_err(|e| storage_err("failed to commit the schema change", e))?;
            Ok(info)
        })
    }

    /// The definition currently stored for an entity, if it has been declared.
    pub fn entity_definition(
        &self,
        namespace: &str,
        entity: &str,
    ) -> Result<Option<EntityDef>, DataError> {
        self.with_conn(|conn| Ok(load_entity(conn, namespace, entity)?.map(|schema| schema.def)))
    }

    // ----------------------------------------------------------------- reads

    /// Run a fetch.
    pub fn fetch(&self, namespace: &str, request: &FetchRequest) -> Result<DataRows, DataError> {
        request.validate()?;
        self.with_conn(|conn| {
            let schema = require_entity(conn, namespace, &request.entity)?;
            run_fetch_with_search_fallback(conn, &schema, request)
        })
    }

    /// Count the rows a fetch would match, ignoring its limit, offset and cursor.
    pub fn count(&self, namespace: &str, request: &FetchRequest) -> Result<u64, DataError> {
        request.validate()?;
        self.with_conn(|conn| {
            let schema = require_entity(conn, namespace, &request.entity)?;
            let mut attempts = vec![request.search.clone()];
            if let Some(search) = &request.search {
                attempts.push(Some(fts_phrase(search)));
            }
            let mut first_error = None;
            for search in attempts {
                let (sql, params) = build_count_sql(&schema, request, search.as_deref())?;
                let result = conn
                    .query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|e| storage_err("failed to count rows", e));
                match result {
                    Ok(count) => return Ok(count.max(0) as u64),
                    Err(e) => first_error = Some(first_error.unwrap_or(e)),
                }
            }
            Err(first_error
                .unwrap_or_else(|| DataError::Storage("failed to count rows".to_owned())))
        })
    }

    // ---------------------------------------------------------------- writes

    /// Apply a batch of mutations in one transaction, all or nothing.
    pub fn apply(
        &self,
        namespace: &str,
        mutations: &[Mutation],
    ) -> Result<ApplyOutcome, DataError> {
        for mutation in mutations {
            mutation.validate()?;
        }
        if mutations.is_empty() {
            return Ok(ApplyOutcome::default());
        }
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| storage_err("failed to begin a write transaction", e))?;
            let now = now_ms();
            let mut outcome = ApplyOutcome::default();
            // Schemas are looked up once per entity per batch. Not cached across calls: another
            // zellij server sharing this file could have migrated an entity since, and one
            // indexed lookup is nothing beside the wasm and protobuf hops this call already cost.
            let mut schemas: BTreeMap<String, EntitySchema> = BTreeMap::new();
            for mutation in mutations {
                let entity = mutation.entity();
                if !schemas.contains_key(entity) {
                    schemas.insert(entity.to_owned(), require_entity(&tx, namespace, entity)?);
                }
                let schema = &schemas[entity];
                apply_one(&tx, schema, mutation, now, &mut outcome.report)?;
                outcome.touched_entities.insert(entity.to_owned());
            }
            tx.commit()
                .map_err(|e| storage_err("failed to commit the write", e))?;
            Ok(outcome)
        })
    }

    // --------------------------------------------------------------- watches

    /// Register a standing fetch and return its current results.
    ///
    /// The initial rows come back with the handle so that a plugin cannot miss a change in the gap
    /// between fetching and watching.
    pub fn register_watch(
        &self,
        namespace: &str,
        plugin_id: u32,
        client_id: ClientId,
        request: FetchRequest,
        mode: WatchMode,
    ) -> Result<DataWatchHandle, DataError> {
        request.validate()?;
        let request = apply_watch_mode(request, mode);
        self.with_conn(|conn| {
            let schema = require_entity(conn, namespace, &request.entity)?;
            let rows = run_fetch_with_search_fallback(conn, &schema, &request)?;
            let mut guard = self.lock_watches()?;
            let (watches, next_id) = &mut *guard;
            let watch_id = *next_id;
            *next_id += 1;
            watches.push(Watch {
                watch_id,
                plugin_id,
                client_id,
                namespace: namespace.to_owned(),
                entity: request.entity.clone(),
                request,
                last_digest: digest_rows(&rows),
            });
            Ok(DataWatchHandle { watch_id, rows })
        })
    }

    /// Cancel a watch. A plugin may only cancel its own.
    pub fn unregister_watch(&self, plugin_id: u32, watch_id: u64) -> Result<(), DataError> {
        let mut guard = self.lock_watches()?;
        let (watches, _) = &mut *guard;
        let before = watches.len();
        watches.retain(|w| !(w.watch_id == watch_id && w.plugin_id == plugin_id));
        if watches.len() == before {
            return Err(DataError::InvalidQuery(format!(
                "watch {} is not registered by this plugin",
                watch_id
            )));
        }
        Ok(())
    }

    /// Drop every watch belonging to a plugin, for when it unloads.
    pub fn drop_watches_for_plugin(&self, plugin_id: u32) {
        if let Ok(mut guard) = self.watches.lock() {
            guard.0.retain(|w| w.plugin_id != plugin_id);
        }
    }

    /// Re-run every watch a write to these entities could have affected, and report the ones whose
    /// results actually changed.
    ///
    /// Comparing a digest rather than reporting unconditionally is what makes an idempotent write
    /// silent — a plugin that re-puts the same row does not wake every watcher.
    pub fn refresh_watches(
        &self,
        namespace: &str,
        touched_entities: &BTreeSet<String>,
    ) -> Result<Vec<WatchChange>, DataError> {
        // Nothing to do before taking the connection lock at all in the common case of a write to an
        // entity nobody watches.
        {
            let guard = self.lock_watches()?;
            let any = guard
                .0
                .iter()
                .any(|w| w.namespace == namespace && touched_entities.contains(&w.entity));
            if !any {
                return Ok(Vec::new());
            }
        }
        self.with_conn(|conn| {
            let mut guard = self.lock_watches()?;
            let (watches, _) = &mut *guard;
            let mut changes = Vec::new();
            for watch in watches.iter_mut() {
                if watch.namespace != namespace || !touched_entities.contains(&watch.entity) {
                    continue;
                }
                // A watch whose entity has since been dropped, or whose fetch has become invalid,
                // is skipped rather than failing the write that triggered this.
                let Ok(schema) = require_entity(conn, &watch.namespace, &watch.entity) else {
                    continue;
                };
                let Ok(rows) = run_fetch_with_search_fallback(conn, &schema, &watch.request) else {
                    continue;
                };
                let digest = digest_rows(&rows);
                if digest == watch.last_digest {
                    continue;
                }
                watch.last_digest = digest;
                changes.push(WatchChange {
                    watch_id: watch.watch_id,
                    plugin_id: watch.plugin_id,
                    client_id: watch.client_id,
                    rows,
                });
            }
            Ok(changes)
        })
    }

    /// How many watches are registered. For tests and diagnostics.
    pub fn watch_count(&self) -> usize {
        self.watches.lock().map(|guard| guard.0.len()).unwrap_or(0)
    }

    fn lock_watches(&self) -> Result<std::sync::MutexGuard<'_, (Vec<Watch>, u64)>, DataError> {
        self.watches.lock().map_err(|_| {
            DataError::Storage("the plugin data store watch lock is poisoned".to_owned())
        })
    }
}

/// Narrow a watch's fetch to just the `id` column when it only wants ids.
fn apply_watch_mode(mut request: FetchRequest, mode: WatchMode) -> FetchRequest {
    if mode == WatchMode::Ids {
        request.select = vec!["id".to_owned()];
    }
    request
}

/// A digest of a result set, for telling "changed" from "written again with the same values".
///
/// Hand-rolled rather than `Hash` because `f64` is not `Hash`; floats are folded in by their bit
/// pattern, which is what an equality check on the stored value amounts to anyway.
fn digest_rows(rows: &DataRows) -> u64 {
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

// ------------------------------------------------------------------ registry

fn load_entity(
    conn: &Connection,
    namespace: &str,
    entity: &str,
) -> Result<Option<EntitySchema>, DataError> {
    let row = conn
        .query_row(
            "SELECT id, definition, owner_plugin FROM store_entities
             WHERE namespace = ?1 AND entity = ?2",
            rusqlite::params![namespace, entity],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| storage_err("failed to read the entity registry", e))?;
    let Some((row_id, definition, owner_plugin)) = row else {
        return Ok(None);
    };
    let def: EntityDef = serde_json::from_str(&definition).map_err(|e| {
        storage_err(
            &format!("the stored definition of `{}` is unreadable", entity),
            e,
        )
    })?;
    Ok(Some(EntitySchema {
        row_id,
        def,
        owner_plugin,
    }))
}

fn require_entity(
    conn: &Connection,
    namespace: &str,
    entity: &str,
) -> Result<EntitySchema, DataError> {
    load_entity(conn, namespace, entity)?.ok_or_else(|| {
        DataError::UnknownEntity(format!(
            "`{}` has not been declared in namespace `{}`; call define_entity first",
            entity, namespace
        ))
    })
}

fn create_entity(
    conn: &Connection,
    namespace: &str,
    owner_plugin: &str,
    def: &EntityDef,
) -> Result<DataSchemaInfo, DataError> {
    let definition = serde_json::to_string(def)
        .map_err(|e| storage_err("failed to serialize the entity definition", e))?;
    conn.execute(
        "INSERT INTO store_entities (namespace, entity, version, definition, owner_plugin)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![namespace, def.name, def.version, definition, owner_plugin],
    )
    .map_err(|e| storage_err("failed to register the entity", e))?;
    // The table is named after the registry row, so it can only be created once the row exists.
    let schema = EntitySchema {
        row_id: conn.last_insert_rowid(),
        def: def.clone(),
        owner_plugin: owner_plugin.to_owned(),
    };
    let table = schema.table();
    let mut columns = vec![
        "id INTEGER PRIMARY KEY AUTOINCREMENT".to_owned(),
        "uid TEXT UNIQUE".to_owned(),
        "created_at INTEGER NOT NULL".to_owned(),
        "updated_at INTEGER NOT NULL".to_owned(),
    ];
    for attr in &def.attrs {
        columns.push(column_definition(attr)?);
    }
    conn.execute_batch(&format!(
        "CREATE TABLE \"{}\" ({});",
        table,
        columns.join(", ")
    ))
    .map_err(|e| storage_err(&format!("failed to create the table for `{}`", def.name), e))?;
    // Always indexed, because every list, sync and paginate query orders or filters on them.
    conn.execute_batch(&format!(
        "CREATE INDEX \"{table}_created\" ON \"{table}\"(created_at);
         CREATE INDEX \"{table}_updated\" ON \"{table}\"(updated_at);"
    ))
    .map_err(|e| storage_err("failed to create the timestamp indexes", e))?;

    let mut indexes_created = Vec::new();
    for attr in &def.attrs {
        if create_attr_indexes(conn, &schema, attr)? {
            indexes_created.push(attr.name.clone());
        }
    }
    let full_text_rebuilt = rebuild_full_text(conn, &schema)?;
    Ok(DataSchemaInfo {
        namespace: namespace.to_owned(),
        entity: def.name.clone(),
        version: def.version,
        created: true,
        attrs_added: Vec::new(),
        indexes_created,
        full_text_rebuilt,
    })
}

fn column_definition(attr: &AttrDef) -> Result<String, DataError> {
    let mut sql = format!("\"{}\" {}", attr.name, attr.kind.sql_type());
    if attr.required {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = &attr.default {
        sql.push_str(&format!(" DEFAULT {}", sql_literal(default)?));
    }
    Ok(sql)
}

/// Render a default as a SQL literal.
///
/// Defaults are the one place a value cannot be a bound parameter — SQLite requires a literal in a
/// column definition — so text is escaped by doubling quotes and blobs are rendered as hex.
fn sql_literal(value: &DataValue) -> Result<String, DataError> {
    Ok(match value {
        DataValue::Null => "NULL".to_owned(),
        DataValue::Int(i) => i.to_string(),
        DataValue::Bool(b) => (*b as i64).to_string(),
        DataValue::Real(f) => {
            if f.is_finite() {
                format!("{:?}", f)
            } else {
                return Err(DataError::InvalidSchema(format!(
                    "{} is not a usable column default",
                    f
                )));
            }
        },
        DataValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
        DataValue::Blob(b) => {
            let mut hex = String::with_capacity(b.len() * 2 + 3);
            hex.push_str("X'");
            for byte in b {
                hex.push_str(&format!("{:02x}", byte));
            }
            hex.push('\'');
            hex
        },
    })
}

/// Create whatever indexes `attr` declares. Returns whether anything was created.
fn create_attr_indexes(
    conn: &Connection,
    schema: &EntitySchema,
    attr: &AttrDef,
) -> Result<bool, DataError> {
    let table = schema.table();
    let mut created = false;
    if attr.indexed {
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS \"{}\" ON \"{}\"(\"{}\");",
            schema.attr_index(&attr.name),
            table,
            attr.name
        ))
        .map_err(|e| storage_err(&format!("failed to index `{}`", attr.name), e))?;
        created = true;
    }
    if attr.unique {
        conn.execute_batch(&format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS \"{}\" ON \"{}\"(\"{}\");",
            schema.unique_index(&attr.name),
            table,
            attr.name
        ))
        .map_err(|e| {
            storage_err(
                &format!(
                    "failed to make `{}` unique (existing rows may already hold duplicates)",
                    attr.name
                ),
                e,
            )
        })?;
        created = true;
    }
    Ok(created)
}

fn drop_attr_indexes(
    conn: &Connection,
    schema: &EntitySchema,
    attr: &str,
) -> Result<(), DataError> {
    conn.execute_batch(&format!(
        "DROP INDEX IF EXISTS \"{}\"; DROP INDEX IF EXISTS \"{}\";",
        schema.attr_index(attr),
        schema.unique_index(attr)
    ))
    .map_err(|e| storage_err(&format!("failed to drop the indexes on `{}`", attr), e))
}

/// Remove the entity's full-text table and the triggers that maintain it.
///
/// Has to run *before* any `ALTER TABLE ... DROP COLUMN`: SQLite revalidates a table's triggers
/// when a column goes away, and a trigger still naming the dropped column fails the whole
/// statement.
fn drop_full_text(conn: &Connection, schema: &EntitySchema) -> Result<(), DataError> {
    let table = schema.table();
    let fts = schema.fts_table();
    conn.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS \"{table}_fts_ai\";
         DROP TRIGGER IF EXISTS \"{table}_fts_ad\";
         DROP TRIGGER IF EXISTS \"{table}_fts_au\";
         DROP TABLE IF EXISTS \"{fts}\";"
    ))
    .map_err(|e| storage_err("failed to clear the full-text index", e))
}

/// Drop and recreate the entity's full-text index and its triggers, then backfill it.
///
/// Returns whether the entity has a full-text index at all.
fn rebuild_full_text(conn: &Connection, schema: &EntitySchema) -> Result<bool, DataError> {
    let table = schema.table();
    let fts = schema.fts_table();
    drop_full_text(conn, schema)?;

    let ft_attrs: Vec<&str> = schema
        .def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    if ft_attrs.is_empty() {
        return Ok(false);
    }
    let quoted = ft_attrs
        .iter()
        .map(|a| format!("\"{}\"", a))
        .collect::<Vec<_>>()
        .join(", ");
    let new_values = ft_attrs
        .iter()
        .map(|a| format!("new.\"{}\"", a))
        .collect::<Vec<_>>()
        .join(", ");
    let old_values = ft_attrs
        .iter()
        .map(|a| format!("old.\"{}\"", a))
        .collect::<Vec<_>>()
        .join(", ");
    // External content: the FTS table holds only the index and reads the columns back from the
    // entity table, so the text is not stored twice. The triggers are what keep it exact.
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE \"{fts}\" USING fts5(
             {quoted}, content='{table}', content_rowid='id', tokenize='unicode61'
         );
         CREATE TRIGGER \"{table}_fts_ai\" AFTER INSERT ON \"{table}\" BEGIN
             INSERT INTO \"{fts}\"(rowid, {quoted}) VALUES (new.id, {new_values});
         END;
         CREATE TRIGGER \"{table}_fts_ad\" AFTER DELETE ON \"{table}\" BEGIN
             INSERT INTO \"{fts}\"(\"{fts}\", rowid, {quoted})
                 VALUES('delete', old.id, {old_values});
         END;
         CREATE TRIGGER \"{table}_fts_au\" AFTER UPDATE ON \"{table}\" BEGIN
             INSERT INTO \"{fts}\"(\"{fts}\", rowid, {quoted})
                 VALUES('delete', old.id, {old_values});
             INSERT INTO \"{fts}\"(rowid, {quoted}) VALUES (new.id, {new_values});
         END;
         INSERT INTO \"{fts}\"(\"{fts}\") VALUES('rebuild');"
    ))
    .map_err(|e| storage_err("failed to build the full-text index", e))?;
    Ok(true)
}

fn migrate_entity(
    conn: &Connection,
    namespace: &str,
    owner_plugin: &str,
    existing: &EntitySchema,
    def: &EntityDef,
    allow_destructive: bool,
) -> Result<DataSchemaInfo, DataError> {
    // In a shared namespace a second plugin may extend an entity but never shrink it: the plugin
    // that declared it is still reading the columns this one would drop. Conflicts are an error
    // rather than a silent migration in either direction.
    let foreign_owner = existing.owner_plugin != owner_plugin;
    if foreign_owner && allow_destructive {
        return Err(DataError::RefusedMigration(format!(
            "`{}` in namespace `{}` was declared by `{}`; only its owner may make destructive \
             changes",
            def.name, namespace, existing.owner_plugin
        )));
    }

    let mut info = DataSchemaInfo {
        namespace: namespace.to_owned(),
        entity: def.name.clone(),
        version: def.version,
        created: false,
        attrs_added: Vec::new(),
        indexes_created: Vec::new(),
        full_text_rebuilt: false,
    };

    // Work out everything that is about to change, and refuse the whole migration, before
    // performing any of it — a half-applied schema change is worse than a rejected one.
    let removed: Vec<&AttrDef> = existing
        .def
        .attrs
        .iter()
        .filter(|stored| def.attr_named(&stored.name).is_none())
        .collect();
    if !removed.is_empty() {
        let names = removed
            .iter()
            .map(|a| format!("`{}`", a.name))
            .collect::<Vec<_>>()
            .join(", ");
        if !allow_destructive {
            return Err(DataError::RefusedMigration(format!(
                "the new definition of `{}` drops {}, which would discard the stored values; pass \
                 allow_destructive to confirm, or leave the attributes declared and stop reading \
                 them",
                def.name, names
            )));
        }
        if def.version <= existing.def.version {
            return Err(DataError::RefusedMigration(format!(
                "dropping {} from `{}` needs a version above the stored {}, so that the change is \
                 recorded rather than silent",
                names, def.name, existing.def.version
            )));
        }
    }

    let stored_ft: Vec<&str> = existing
        .def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    let new_ft: Vec<&str> = def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    // The FTS table names the columns it indexes, so it has to be rebuilt whenever that set
    // changes. It also has to come down *before* any column is dropped, because its triggers name
    // those columns and SQLite revalidates every trigger on the table when one goes away.
    let rebuild_needed = stored_ft != new_ft || !removed.is_empty();
    if rebuild_needed && !stored_ft.is_empty() {
        drop_full_text(conn, existing)?;
    }

    for attr in &def.attrs {
        match existing.attr(&attr.name) {
            Some(stored) if stored.kind != attr.kind => {
                return Err(DataError::RefusedMigration(format!(
                    "attribute `{}` of `{}` is stored as {:?} and the new definition makes it \
                     {:?}; changing the type of a column would have to reinterpret existing rows, \
                     so declare a new attribute instead",
                    attr.name, def.name, stored.kind, attr.kind
                )));
            },
            Some(stored) => {
                // Indexes are cheap to add and drop, and doing so loses nothing, so they follow
                // the declaration without needing permission.
                if attr.indexed != stored.indexed || attr.unique != stored.unique {
                    drop_attr_indexes(conn, existing, &attr.name)?;
                    if create_attr_indexes(conn, existing, attr)? {
                        info.indexes_created.push(attr.name.clone());
                    }
                }
            },
            None => {
                if attr.required && attr.default.is_none() {
                    return Err(DataError::RefusedMigration(format!(
                        "attribute `{}` is being added to the existing entity `{}` as required, \
                         but has no default; SQLite cannot add a NOT NULL column without one, and \
                         the rows already stored would have nothing to put in it",
                        attr.name, def.name
                    )));
                }
                conn.execute_batch(&format!(
                    "ALTER TABLE \"{}\" ADD COLUMN {};",
                    existing.table(),
                    column_definition(attr)?
                ))
                .map_err(|e| storage_err(&format!("failed to add `{}`", attr.name), e))?;
                info.attrs_added.push(attr.name.clone());
                if create_attr_indexes(conn, existing, attr)? {
                    info.indexes_created.push(attr.name.clone());
                }
            },
        }
    }

    for attr in removed {
        drop_attr_indexes(conn, existing, &attr.name)?;
        conn.execute_batch(&format!(
            "ALTER TABLE \"{}\" DROP COLUMN \"{}\";",
            existing.table(),
            attr.name
        ))
        .map_err(|e| storage_err(&format!("failed to drop `{}`", attr.name), e))?;
    }

    let updated = EntitySchema {
        row_id: existing.row_id,
        def: def.clone(),
        owner_plugin: existing.owner_plugin.clone(),
    };
    if rebuild_needed {
        info.full_text_rebuilt = rebuild_full_text(conn, &updated)?;
    }

    let definition = serde_json::to_string(def)
        .map_err(|e| storage_err("failed to serialize the entity definition", e))?;
    conn.execute(
        "UPDATE store_entities SET version = ?1, definition = ?2 WHERE id = ?3",
        rusqlite::params![def.version, definition, existing.row_id],
    )
    .map_err(|e| storage_err("failed to record the new definition", e))?;
    Ok(info)
}

// -------------------------------------------------------------- compilation

/// Compile a predicate into a SQL fragment plus its bound parameters.
fn compile_pred(
    pred: &Pred,
    schema: &EntitySchema,
    table: &str,
    sql: &mut String,
    params: &mut Vec<DataValue>,
) -> Result<(), DataError> {
    /// `attr op ?`, with the value's kind checked against the column's.
    fn binary(
        schema: &EntitySchema,
        table: &str,
        attr: &str,
        op: &str,
        value: &DataValue,
        sql: &mut String,
        params: &mut Vec<DataValue>,
    ) -> Result<(), DataError> {
        schema.require_column(attr)?;
        check_value_kind(schema, attr, value)?;
        sql.push_str(&format!("\"{}\".\"{}\" {} ?", table, attr, op));
        params.push(value.clone());
        Ok(())
    }

    /// `LIKE` with the pattern's wildcards escaped, so a literal `%` in the needle stays literal.
    fn like(
        schema: &EntitySchema,
        table: &str,
        attr: &str,
        pattern: String,
        sql: &mut String,
        params: &mut Vec<DataValue>,
    ) -> Result<(), DataError> {
        schema.require_column(attr)?;
        require_text_column(schema, attr)?;
        sql.push_str(&format!("\"{}\".\"{}\" LIKE ? ESCAPE '\\'", table, attr));
        params.push(DataValue::Text(pattern));
        Ok(())
    }

    match pred {
        Pred::Eq(a, v) => binary(schema, table, a, "=", v, sql, params)?,
        Pred::Ne(a, v) => binary(schema, table, a, "!=", v, sql, params)?,
        Pred::Gt(a, v) => binary(schema, table, a, ">", v, sql, params)?,
        Pred::Gte(a, v) => binary(schema, table, a, ">=", v, sql, params)?,
        Pred::Lt(a, v) => binary(schema, table, a, "<", v, sql, params)?,
        Pred::Lte(a, v) => binary(schema, table, a, "<=", v, sql, params)?,
        Pred::In(a, values) => {
            schema.require_column(a)?;
            for value in values {
                check_value_kind(schema, a, value)?;
            }
            let placeholders = vec!["?"; values.len()].join(", ");
            sql.push_str(&format!("\"{}\".\"{}\" IN ({})", table, a, placeholders));
            params.extend(values.iter().cloned());
        },
        Pred::IsNull(a) => {
            schema.require_column(a)?;
            sql.push_str(&format!("\"{}\".\"{}\" IS NULL", table, a));
        },
        Pred::NotNull(a) => {
            schema.require_column(a)?;
            sql.push_str(&format!("\"{}\".\"{}\" IS NOT NULL", table, a));
        },
        Pred::Prefix(a, v) => like(
            schema,
            table,
            a,
            format!("{}%", escape_like(v)),
            sql,
            params,
        )?,
        Pred::Contains(a, v) => like(
            schema,
            table,
            a,
            format!("%{}%", escape_like(v)),
            sql,
            params,
        )?,
        Pred::And(preds) | Pred::Or(preds) => {
            let joiner = if matches!(pred, Pred::And(_)) {
                " AND "
            } else {
                " OR "
            };
            sql.push('(');
            for (i, inner) in preds.iter().enumerate() {
                if i > 0 {
                    sql.push_str(joiner);
                }
                compile_pred(inner, schema, table, sql, params)?;
            }
            sql.push(')');
        },
        Pred::Not(inner) => {
            sql.push_str("NOT (");
            compile_pred(inner, schema, table, sql, params)?;
            sql.push(')');
        },
    }
    Ok(())
}

/// Escape the wildcards `LIKE` would otherwise interpret.
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Reject a `Prefix` or `Contains` against a column that holds no text.
///
/// SQLite would coerce the value to its text form and match against that, so `Prefix("count", "1")`
/// would quietly match 1, 10 and 100. Every other predicate checks the kind it is comparing
/// against; leaving these two out made them the one place a type mistake produced results instead
/// of an error.
fn require_text_column(schema: &EntitySchema, attr: &str) -> Result<(), DataError> {
    let kind = match schema.column_kind(attr) {
        Some(kind) => kind,
        // A store column: `uid` is the only text one.
        None if attr == "uid" => AttrKind::Text,
        None => AttrKind::Int,
    };
    if matches!(kind, AttrKind::Text | AttrKind::Json) {
        return Ok(());
    }
    Err(DataError::InvalidQuery(format!(
        "`{}` of `{}` is {:?}, which holds no text to match a prefix or substring against",
        attr, schema.def.name, kind
    )))
}

/// Reject comparing a column against a value of the wrong kind.
///
/// SQLite would happily compare a string to an integer column and simply never match, which reads
/// as "no results" rather than as the mistake it is.
fn check_value_kind(schema: &EntitySchema, attr: &str, value: &DataValue) -> Result<(), DataError> {
    let Some(kind) = schema.column_kind(attr) else {
        // One of the store's own columns: `id`, `created_at` and `updated_at` are integers, `uid`
        // is text.
        let expected = if attr == "uid" {
            AttrKind::Text
        } else {
            AttrKind::Int
        };
        return if value.fits(expected) {
            Ok(())
        } else {
            Err(DataError::InvalidQuery(format!(
                "`{}` is {:?} but it is compared against {}",
                attr,
                expected,
                value.kind_name()
            )))
        };
    };
    if value.fits(kind) {
        Ok(())
    } else {
        Err(DataError::InvalidQuery(format!(
            "attribute `{}` is {:?} but it is compared against {}",
            attr,
            kind,
            value.kind_name()
        )))
    }
}

/// The columns a fetch returns, in order.
fn output_columns(schema: &EntitySchema, request: &FetchRequest) -> Result<Vec<String>, DataError> {
    if request.select.is_empty() {
        let mut columns: Vec<String> = STORE_COLUMNS.iter().map(|c| (*c).to_owned()).collect();
        columns.extend(schema.def.attrs.iter().map(|a| a.name.clone()));
        return Ok(columns);
    }
    for column in &request.select {
        schema.require_column(column)?;
    }
    Ok(request.select.clone())
}

/// Whether the fetch is ordered by `id` ascending — the only ordering `after_id` can page through.
fn is_default_ordering(request: &FetchRequest) -> bool {
    request.sort.is_empty() && request.search.is_none()
}

struct FetchSql {
    sql: String,
    params: Vec<DataValue>,
    output_columns: Vec<String>,
}

fn build_fetch_sql(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<FetchSql, DataError> {
    let table = schema.table();
    let columns = output_columns(schema, request)?;
    // `id` is always selected first, whether or not it was asked for, so that a truncated fetch
    // can hand back a cursor even when the plugin only wanted some other column.
    let mut selected = vec![format!("\"{}\".\"id\"", table)];
    for column in &columns {
        selected.push(format!("\"{}\".\"{}\"", table, column));
    }
    let (where_clause, params, join) = build_where(schema, request, search)?;
    let order = build_order(schema, request, search)?;
    let sql = format!(
        "SELECT {} FROM \"{}\"{} {} ORDER BY {} LIMIT ? OFFSET ?",
        selected.join(", "),
        table,
        join,
        where_clause,
        order
    );
    Ok(FetchSql {
        sql,
        params,
        output_columns: columns,
    })
}

fn build_count_sql(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<(String, Vec<DataValue>), DataError> {
    let table = schema.table();
    let (where_clause, params, join) = build_where(schema, request, search)?;
    Ok((
        format!(
            "SELECT COUNT(*) FROM \"{}\"{} {}",
            table, join, where_clause
        ),
        params,
    ))
}

/// Build the `WHERE` clause, its parameters, and the FTS join it may need.
fn build_where(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<(String, Vec<DataValue>, String), DataError> {
    let table = schema.table();
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<DataValue> = Vec::new();
    let mut join = String::new();

    if let Some(search) = search {
        if schema.def.full_text_attrs().is_empty() {
            return Err(DataError::InvalidQuery(format!(
                "`{}` has no full-text attributes to search; declare one with .full_text()",
                schema.def.name
            )));
        }
        let fts = schema.fts_table();
        join = format!(
            " JOIN \"{fts}\" ON \"{fts}\".rowid = \"{table}\".\"id\"",
            fts = fts,
            table = table
        );
        conditions.push(format!("\"{}\" MATCH ?", fts));
        params.push(DataValue::Text(search.to_owned()));
    }

    if let Some(filter) = &request.filter {
        let mut sql = String::new();
        compile_pred(filter, schema, &table, &mut sql, &mut params)?;
        conditions.push(sql);
    }

    if let Some(after_id) = request.after_id {
        if !is_default_ordering(request) {
            return Err(DataError::InvalidQuery(
                "after_id pages through the default id ordering, so it cannot be combined with a \
                 sort or a search; use offset for those"
                    .to_owned(),
            ));
        }
        conditions.push(format!("\"{}\".\"id\" > ?", table));
        params.push(DataValue::Int(after_id));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    Ok((where_clause, params, join))
}

fn build_order(
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<String, DataError> {
    let table = schema.table();
    let mut terms: Vec<String> = Vec::new();
    if request.sort.is_empty() {
        if search.is_some() {
            // bm25 returns a more negative score the better the match, so ascending is best-first.
            terms.push(format!("bm25(\"{}\") ASC", schema.fts_table()));
        }
    } else {
        for (attr, dir) in &request.sort {
            schema.require_column(attr)?;
            terms.push(format!("\"{}\".\"{}\" {}", table, attr, dir.sql()));
        }
    }
    // Always break ties on `id`, so that paging through a set sorted on a column with duplicates
    // returns each row once instead of reshuffling between pages.
    terms.push(format!("\"{}\".\"id\" ASC", table));
    Ok(terms.join(", "))
}

/// Run a fetch, retrying a search that isn't valid FTS5 as a literal phrase.
///
/// Punctuation and unbalanced quotes are FTS5 syntax errors, and are also exactly what gets pasted
/// in from a log line — the same reason [`crate::session_recording`] does this. On a second failure
/// the *first* error is returned, since that is the one describing what the plugin actually asked.
fn run_fetch_with_search_fallback(
    conn: &Connection,
    schema: &EntitySchema,
    request: &FetchRequest,
) -> Result<DataRows, DataError> {
    let first = run_fetch(conn, schema, request, request.search.as_deref());
    let (Err(first_error), Some(search)) = (&first, &request.search) else {
        return first;
    };
    match run_fetch(conn, schema, request, Some(&fts_phrase(search))) {
        Ok(rows) => Ok(rows),
        Err(_) => Err(first_error.clone()),
    }
}

fn run_fetch(
    conn: &Connection,
    schema: &EntitySchema,
    request: &FetchRequest,
    search: Option<&str>,
) -> Result<DataRows, DataError> {
    let FetchSql {
        sql,
        mut params,
        output_columns,
    } = build_fetch_sql(schema, request, search)?;
    let limit = request.effective_limit();
    // One row past the limit, so that hitting it is distinguishable from reaching the end.
    params.push(DataValue::Int(limit as i64 + 1));
    params.push(DataValue::Int(request.offset as i64));

    let kinds: Vec<Option<AttrKind>> = output_columns
        .iter()
        .map(|c| schema.column_kind(c))
        .collect();

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| storage_err("failed to prepare the fetch", e))?;
    let mut sqlite_rows = stmt
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| storage_err("failed to run the fetch", e))?;

    let mut rows: Vec<Vec<DataValue>> = Vec::new();
    let mut truncated = false;
    let mut last_id: Option<i64> = None;
    let mut bytes = 0usize;
    while let Some(row) = sqlite_rows
        .next()
        .map_err(|e| storage_err("failed to read a row", e))?
    {
        if rows.len() as u32 >= limit {
            truncated = true;
            break;
        }
        let id: i64 = row
            .get(0)
            .map_err(|e| storage_err("failed to read a row id", e))?;
        let mut values = Vec::with_capacity(output_columns.len());
        let mut row_bytes = 0usize;
        for (offset, kind) in kinds.iter().enumerate() {
            let value = read_value(row, offset + 1, *kind)?;
            row_bytes += value.encoded_size();
            values.push(value);
        }
        // Stop before exceeding the budget rather than after, but never return zero rows for it:
        // a single row over the budget still has to come back, or the plugin cannot make progress.
        if bytes + row_bytes > MAX_FETCH_BYTES && !rows.is_empty() {
            truncated = true;
            break;
        }
        bytes += row_bytes;
        last_id = Some(id);
        rows.push(values);
    }

    let next_cursor = if truncated && is_default_ordering(request) {
        last_id
    } else {
        None
    };
    Ok(DataRows {
        columns: output_columns,
        rows,
        truncated,
        next_cursor,
    })
}

fn read_value(
    row: &rusqlite::Row<'_>,
    index: usize,
    kind: Option<AttrKind>,
) -> Result<DataValue, DataError> {
    let value = row
        .get_ref(index)
        .map_err(|e| storage_err("failed to read a value", e))?;
    Ok(match (value, kind) {
        (ValueRef::Null, _) => DataValue::Null,
        // Only a column declared Bool comes back as one; SQLite stores both in an INTEGER.
        (ValueRef::Integer(i), Some(AttrKind::Bool)) => DataValue::Bool(i != 0),
        (ValueRef::Integer(i), _) => DataValue::Int(i),
        (ValueRef::Real(f), _) => DataValue::Real(f),
        (ValueRef::Text(t), _) => DataValue::Text(String::from_utf8_lossy(t).into_owned()),
        (ValueRef::Blob(b), _) => DataValue::Blob(b.to_vec()),
    })
}

// ------------------------------------------------------------------- writes

/// Check the values of a mutation against the schema, rejecting undeclared attributes, wrong kinds
/// and malformed JSON.
fn check_values(schema: &EntitySchema, values: &[(String, DataValue)]) -> Result<(), DataError> {
    for (attr, value) in values {
        let Some(declared) = schema.attr(attr) else {
            return Err(DataError::InvalidQuery(format!(
                "`{}` is not a declared attribute of `{}`{}",
                attr,
                schema.def.name,
                if STORE_COLUMNS.contains(&attr.as_str()) {
                    "; it is maintained by the store and cannot be written directly"
                } else {
                    ""
                }
            )));
        };
        if !value.fits(declared.kind) {
            return Err(DataError::InvalidQuery(format!(
                "attribute `{}` of `{}` is {:?} but is being set to {}",
                attr,
                schema.def.name,
                declared.kind,
                value.kind_name()
            )));
        }
        // A Json attribute is worth having as a distinct kind only if it is actually JSON when it
        // comes back out, so it is validated on the way in.
        if declared.kind == AttrKind::Json {
            if let DataValue::Text(text) = value {
                serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                    DataError::InvalidQuery(format!(
                        "attribute `{}` of `{}` is Json but was set to something that isn't: {}",
                        attr, schema.def.name, e
                    ))
                })?;
            }
        }
    }
    Ok(())
}

fn apply_one(
    conn: &Connection,
    schema: &EntitySchema,
    mutation: &Mutation,
    now: i64,
    report: &mut ApplyReport,
) -> Result<(), DataError> {
    let table = schema.table();
    match mutation {
        Mutation::Put { uid, values, .. } => {
            check_values(schema, values)?;
            let mut columns = vec![
                "uid".to_owned(),
                "created_at".to_owned(),
                "updated_at".to_owned(),
            ];
            let mut params: Vec<DataValue> = vec![
                uid.clone().map(DataValue::Text).unwrap_or(DataValue::Null),
                DataValue::Int(now),
                DataValue::Int(now),
            ];
            for (attr, value) in values {
                columns.push(attr.clone());
                params.push(value.clone());
            }
            let quoted = columns
                .iter()
                .map(|c| format!("\"{}\"", c))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; columns.len()].join(", ");
            // With a uid this is an upsert. `created_at` is deliberately left out of the update:
            // the row is the same row, and when it was first written is still true.
            let conflict = if uid.is_some() {
                let assignments = values
                    .iter()
                    .map(|(attr, _)| format!("\"{attr}\" = excluded.\"{attr}\""))
                    .chain(std::iter::once(
                        "\"updated_at\" = excluded.\"updated_at\"".to_owned(),
                    ))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" ON CONFLICT(\"uid\") DO UPDATE SET {}", assignments)
            } else {
                String::new()
            };
            let sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES ({}){} RETURNING \"id\"",
                table, quoted, placeholders, conflict
            );
            let id: i64 = conn
                .query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
                    row.get(0)
                })
                .map_err(|e| {
                    storage_err(
                        &format!("failed to write a row of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_written += 1;
            report.ids.push(id);
        },
        Mutation::Update { filter, values, .. } => {
            check_values(schema, values)?;
            let mut params: Vec<DataValue> = Vec::new();
            let mut assignments: Vec<String> = Vec::new();
            for (attr, value) in values {
                assignments.push(format!("\"{}\" = ?", attr));
                params.push(value.clone());
            }
            assignments.push("\"updated_at\" = ?".to_owned());
            params.push(DataValue::Int(now));
            let mut where_sql = String::new();
            compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
            let sql = format!(
                "UPDATE \"{}\" SET {} WHERE {}",
                table,
                assignments.join(", "),
                where_sql
            );
            let changed = conn
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(|e| {
                    storage_err(
                        &format!("failed to update rows of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_written += changed as u64;
        },
        Mutation::Delete { filter, .. } => {
            let mut params: Vec<DataValue> = Vec::new();
            let mut where_sql = String::new();
            compile_pred(filter, schema, &table, &mut where_sql, &mut params)?;
            let sql = format!("DELETE FROM \"{}\" WHERE {}", table, where_sql);
            let changed = conn
                .execute(&sql, rusqlite::params_from_iter(params.iter()))
                .map_err(|e| {
                    storage_err(
                        &format!("failed to delete rows of `{}`", schema.def.name),
                        e,
                    )
                })?;
            report.rows_deleted += changed as u64;
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_data::{SortDir, MAX_FETCH_ROWS};
    use hegel::generators::{self, Generator};
    use tempfile::{tempdir, TempDir};

    const NS: &str = "plugin:test";
    const OWNER: &str = "zellij:test";

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

    // -------------------------------------------------------------- schema

    #[test]
    fn defining_an_entity_reports_that_it_was_created() {
        let (store, _dir) = store();
        let info = store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
        assert!(info.created);
        assert_eq!(info.namespace, NS);
        assert_eq!(info.entity, "agent");
        assert!(info.full_text_rebuilt);
        assert_eq!(
            info.indexes_created,
            vec!["project", "branch", "updated_at_ms"]
        );
    }

    #[test]
    fn redeclaring_an_identical_definition_changes_nothing() {
        let (store, _dir) = defined();
        put(&store, "zellij", "hello", 1);
        let info = store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
        assert!(!info.created);
        assert!(info.attrs_added.is_empty());
        assert!(info.indexes_created.is_empty());
        assert!(!info.full_text_rebuilt);
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn adding_an_attribute_preserves_existing_rows() {
        let (store, _dir) = defined();
        put(&store, "zellij", "hello", 1);
        let extended = agent_def().attr(AttrDef::text("worktree").indexed());
        let info = store.define_entity(NS, OWNER, &extended, false).unwrap();
        assert_eq!(info.attrs_added, vec!["worktree"]);
        assert_eq!(info.indexes_created, vec!["worktree"]);

        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(texts(&rows, "project"), vec!["zellij"]);
        // the new column exists, and is null on the row that predates it
        assert_eq!(column(&rows, "worktree"), vec![DataValue::Null]);
    }

    #[test]
    fn a_required_attribute_added_without_a_default_is_refused() {
        let (store, _dir) = defined();
        let extended = agent_def().attr(AttrDef::text("worktree").required());
        let err = store
            .define_entity(NS, OWNER, &extended, false)
            .unwrap_err();
        assert!(
            matches!(err, DataError::RefusedMigration(_)),
            "got {:?}",
            err
        );
    }

    #[test]
    fn a_required_attribute_added_with_a_default_backfills_the_existing_rows() {
        let (store, _dir) = defined();
        put(&store, "zellij", "hello", 1);
        let extended = agent_def().attr(AttrDef::text("worktree").required().default("primary"));
        store.define_entity(NS, OWNER, &extended, false).unwrap();
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        assert_eq!(texts(&rows, "worktree"), vec!["primary"]);
    }

    #[test]
    fn changing_the_type_of_an_attribute_is_refused() {
        let (store, _dir) = defined();
        let retyped = EntityDef::new("agent", 2)
            .attr(AttrDef::int("project"))
            .attr(AttrDef::text("branch"))
            .attr(AttrDef::text("summary"))
            .attr(AttrDef::int("updated_at_ms"));
        let err = store.define_entity(NS, OWNER, &retyped, false).unwrap_err();
        assert!(
            matches!(err, DataError::RefusedMigration(_)),
            "got {:?}",
            err
        );
    }

    #[test]
    fn dropping_an_attribute_is_refused_unless_it_is_asked_for() {
        let (store, _dir) = defined();
        let shrunk = EntityDef::new("agent", 2)
            .attr(AttrDef::text("project").indexed())
            .attr(AttrDef::text("branch").indexed());
        let err = store.define_entity(NS, OWNER, &shrunk, false).unwrap_err();
        assert!(
            matches!(err, DataError::RefusedMigration(_)),
            "got {:?}",
            err
        );
        // the definition is untouched, so nothing was half-migrated
        assert!(store
            .entity_definition(NS, "agent")
            .unwrap()
            .unwrap()
            .attr_named("summary")
            .is_some());
    }

    #[test]
    fn dropping_an_attribute_also_needs_a_version_above_the_stored_one() {
        let (store, _dir) = defined();
        let shrunk = EntityDef::new("agent", 1)
            .attr(AttrDef::text("project").indexed())
            .attr(AttrDef::text("branch").indexed())
            .attr(AttrDef::int("updated_at_ms").indexed());
        let err = store.define_entity(NS, OWNER, &shrunk, true).unwrap_err();
        assert!(
            matches!(err, DataError::RefusedMigration(_)),
            "got {:?}",
            err
        );
    }

    #[test]
    fn dropping_an_attribute_works_when_asked_for_with_a_new_version() {
        let (store, _dir) = defined();
        put(&store, "zellij", "hello", 1);
        let shrunk = EntityDef::new("agent", 2)
            .attr(AttrDef::text("project").indexed())
            .attr(AttrDef::text("branch").indexed())
            .attr(AttrDef::int("updated_at_ms").indexed());
        store.define_entity(NS, OWNER, &shrunk, true).unwrap();
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        assert!(rows.column_index("summary").is_none());
        assert_eq!(texts(&rows, "project"), vec!["zellij"]);
    }

    #[test]
    fn marking_an_attribute_searchable_later_indexes_the_rows_already_stored() {
        let (store, _dir) = store();
        let plain = EntityDef::new("agent", 1)
            .attr(AttrDef::text("project"))
            .attr(AttrDef::text("summary"));
        store.define_entity(NS, OWNER, &plain, false).unwrap();
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .set("project", "zellij")
                    .set("summary", "a rebase conflict")
                    .build()],
            )
            .unwrap();
        // nothing searchable yet, so searching is a mistake rather than an empty result
        let err = store
            .fetch(NS, &FetchRequest::entity("agent").search("rebase"))
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);

        let searchable = EntityDef::new("agent", 2)
            .attr(AttrDef::text("project"))
            .attr(AttrDef::text("summary").full_text());
        let info = store.define_entity(NS, OWNER, &searchable, false).unwrap();
        assert!(info.full_text_rebuilt);
        // the rebuild backfills, so a row written before the index existed is findable
        let rows = store
            .fetch(NS, &FetchRequest::entity("agent").search("rebase"))
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_second_plugin_may_extend_a_shared_entity_but_not_shrink_it() {
        let (store, _dir) = store();
        let shared = "shared:agents";
        store
            .define_entity(shared, "zellij:hocus", &agent_def(), false)
            .unwrap();
        // extending is fine: the owner's columns are all still there
        let extended = agent_def().attr(AttrDef::text("worktree"));
        store
            .define_entity(shared, "zellij:other", &extended, false)
            .unwrap();
        // shrinking is not, whatever it claims, because the owner is still reading those columns
        let shrunk = EntityDef::new("agent", 9).attr(AttrDef::text("project"));
        let err = store
            .define_entity(shared, "zellij:other", &shrunk, true)
            .unwrap_err();
        assert!(
            matches!(err, DataError::RefusedMigration(_)),
            "got {:?}",
            err
        );
    }

    #[test]
    fn namespaces_are_separate_stores() {
        let (store, _dir) = store();
        store
            .define_entity("plugin:a", "a", &agent_def(), false)
            .unwrap();
        store
            .define_entity("plugin:b", "b", &agent_def(), false)
            .unwrap();
        store
            .apply(
                "plugin:a",
                &[Mutation::put("agent").set("project", "only-in-a").build()],
            )
            .unwrap();
        let a = store
            .fetch("plugin:a", &FetchRequest::entity("agent"))
            .unwrap();
        let b = store
            .fetch("plugin:b", &FetchRequest::entity("agent"))
            .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn a_private_namespace_is_derived_from_the_plugin_url() {
        assert_eq!(private_namespace("zellij:hocus"), "plugin:zellij_hocus");
        // a debug and a release build of the same plugin are different plugins here, exactly as
        // they are for permissions
        assert_ne!(
            private_namespace("file:/target/debug/x.wasm"),
            private_namespace("file:/target/release/x.wasm")
        );
        assert!(!is_shared_namespace(&private_namespace("zellij:hocus")));
        assert!(is_shared_namespace("shared:agents"));
    }

    #[test]
    fn using_an_undeclared_entity_says_so() {
        let (store, _dir) = store();
        let err = store.fetch(NS, &FetchRequest::entity("agent")).unwrap_err();
        assert!(matches!(err, DataError::UnknownEntity(_)), "got {:?}", err);
    }

    // -------------------------------------------------------------- writes

    #[test]
    fn a_put_without_a_uid_always_inserts() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 1);
        put(&store, "zellij", "two", 2);
        assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 2);
    }

    #[test]
    fn a_put_with_a_uid_upserts_and_keeps_the_original_created_at() {
        let (store, _dir) = defined();
        let first = store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("project", "zellij")
                    .set("updated_at_ms", 1)
                    .build()],
            )
            .unwrap();
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        let created = column(&rows, "created_at")[0].clone();

        // enough for the millisecond clock to move, so that `updated_at` can be seen to differ
        std::thread::sleep(std::time::Duration::from_millis(3));
        let second = store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("project", "zellij-fork")
                    .set("updated_at_ms", 2)
                    .build()],
            )
            .unwrap();
        // the same row, not a second one
        assert_eq!(first.report.ids, second.report.ids);
        assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 1);

        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        assert_eq!(texts(&rows, "project"), vec!["zellij-fork"]);
        // when the row was first written is still true after an update
        assert_eq!(column(&rows, "created_at")[0], created);
        assert_ne!(column(&rows, "updated_at")[0], created);
    }

    #[test]
    fn a_batch_is_all_or_nothing() {
        let (store, _dir) = defined();
        put(&store, "zellij", "keep", 1);
        let err = store
            .apply(
                NS,
                &[
                    Mutation::put("agent")
                        .set("project", "written-first")
                        .build(),
                    // an undeclared attribute: the whole batch has to roll back, including the
                    // mutation before it that would otherwise have succeeded
                    Mutation::put("agent").set("nonexistent", "x").build(),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
        let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        // only the row from before the batch: the valid first mutation rolled back with the second
        assert_eq!(texts(&rows, "summary"), vec!["keep"]);
        assert_eq!(texts(&rows, "project"), vec!["zellij"]);
    }

    #[test]
    fn the_stores_own_columns_cannot_be_written_directly() {
        let (store, _dir) = defined();
        for name in ["id", "created_at", "updated_at", "uid"] {
            let err = store
                .apply(NS, &[Mutation::put("agent").set(name, 1).build()])
                .unwrap_err();
            assert!(
                matches!(err, DataError::InvalidName(_) | DataError::InvalidQuery(_)),
                "`{}` should be rejected, got {:?}",
                name,
                err
            );
        }
    }

    #[test]
    fn writing_the_wrong_kind_to_a_column_is_rejected() {
        let (store, _dir) = defined();
        let err = store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .set("updated_at_ms", "not a number")
                    .build()],
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    #[test]
    fn a_json_attribute_is_validated_on_write() {
        let (store, _dir) = store();
        let def = EntityDef::new("blob", 1).attr(AttrDef::json("payload"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        store
            .apply(
                NS,
                &[Mutation::put("blob")
                    .set("payload", r#"{"ok":true}"#)
                    .build()],
            )
            .unwrap();
        let err = store
            .apply(
                NS,
                &[Mutation::put("blob").set("payload", "{not json").build()],
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    #[test]
    fn update_and_delete_report_how_many_rows_they_touched() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 1);
        put(&store, "zellij", "two", 2);
        put(&store, "other", "three", 3);

        let updated = store
            .apply(
                NS,
                &[Mutation::update("agent", Pred::eq("project", "zellij"))
                    .set("branch", "dev")
                    .build()],
            )
            .unwrap();
        assert_eq!(updated.report.rows_written, 2);

        let deleted = store
            .apply(
                NS,
                &[Mutation::delete("agent", Pred::eq("project", "other"))],
            )
            .unwrap();
        assert_eq!(deleted.report.rows_deleted, 1);
        assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 2);
    }

    #[test]
    fn an_apply_reports_which_entities_it_touched() {
        let (store, _dir) = defined();
        let other = EntityDef::new("todo", 1).attr(AttrDef::text("text"));
        store.define_entity(NS, OWNER, &other, false).unwrap();
        let outcome = store
            .apply(
                NS,
                &[
                    Mutation::put("agent").set("project", "zellij").build(),
                    Mutation::put("todo").set("text", "ship it").build(),
                ],
            )
            .unwrap();
        assert_eq!(
            outcome.touched_entities,
            ["agent".to_owned(), "todo".to_owned()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_unique_attribute_rejects_a_duplicate() {
        let (store, _dir) = store();
        let def = EntityDef::new("agent", 1).attr(AttrDef::text("slug").unique());
        store.define_entity(NS, OWNER, &def, false).unwrap();
        store
            .apply(NS, &[Mutation::put("agent").set("slug", "a").build()])
            .unwrap();
        let err = store
            .apply(NS, &[Mutation::put("agent").set("slug", "a").build()])
            .unwrap_err();
        assert!(matches!(err, DataError::Storage(_)), "got {:?}", err);
    }

    // --------------------------------------------------------------- reads

    #[test]
    fn a_filter_selects_rows() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 10);
        put(&store, "zellij", "two", 20);
        put(&store, "other", "three", 30);

        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .filter(Pred::eq("project", "zellij").and(Pred::gt("updated_at_ms", 15))),
            )
            .unwrap();
        assert_eq!(texts(&rows, "summary"), vec!["two"]);
    }

    #[test]
    fn filtering_on_an_undeclared_attribute_is_an_error_rather_than_an_empty_result() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 10);
        let err = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::eq("projet", "zellij")),
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    #[test]
    fn comparing_a_column_against_the_wrong_kind_is_an_error() {
        // SQLite would compare a string to an integer column and simply never match, which reads
        // as "no results" rather than as the typo it is.
        let (store, _dir) = defined();
        let err = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::eq("updated_at_ms", "soon")),
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    #[test]
    fn a_prefix_filter_matches_the_start_and_contains_matches_anywhere() {
        let (store, _dir) = defined();
        put(&store, "zellij-fork", "x", 1);
        put(&store, "my-zellij", "y", 2);

        let prefixed = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::prefix("project", "zellij")),
            )
            .unwrap();
        assert_eq!(texts(&prefixed, "project"), vec!["zellij-fork"]);

        let contained = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::contains("project", "zellij")),
            )
            .unwrap();
        assert_eq!(contained.len(), 2);
    }

    #[test]
    fn like_wildcards_in_a_needle_are_matched_literally() {
        let (store, _dir) = defined();
        put(&store, "100%", "x", 1);
        put(&store, "1000", "y", 2);
        // unescaped, `100%` as a prefix pattern would also match `1000`
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::prefix("project", "100%")),
            )
            .unwrap();
        assert_eq!(texts(&rows, "project"), vec!["100%"]);
    }

    #[test]
    fn prefix_and_contains_are_rejected_on_a_non_text_column() {
        // Left unchecked, SQLite coerces the number to text and `Prefix("updated_at_ms", "1")`
        // quietly matches 1, 10 and 100 — results rather than an error, which is the one thing
        // the kind checks elsewhere exist to prevent.
        let (store, _dir) = defined();
        for pred in [
            Pred::prefix("updated_at_ms", "1"),
            Pred::contains("updated_at_ms", "1"),
        ] {
            let err = store
                .fetch(NS, &FetchRequest::entity("agent").filter(pred.clone()))
                .unwrap_err();
            assert!(
                matches!(err, DataError::InvalidQuery(_)),
                "{:?} should be rejected, got {:?}",
                pred,
                err
            );
        }
    }

    #[test]
    fn a_fetch_can_sort_and_select_the_stores_own_columns() {
        // Sorting by `updated_at` is the commonest query there is, and it is indexed for exactly
        // that — the reserved-name rule that stops an *attribute* being called `updated_at` must
        // not stop a query naming the column.
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 1);
        put(&store, "zellij", "two", 2);
        for column in ["id", "uid", "created_at", "updated_at"] {
            let request = FetchRequest::entity("agent")
                .select([column])
                .sort_by(column, SortDir::Desc);
            assert_eq!(request.validate(), Ok(()), "sorting by `{}`", column);
            assert_eq!(store.fetch(NS, &request).unwrap().len(), 2, "`{}`", column);
        }
    }

    #[test]
    fn prefix_and_contains_ignore_ascii_case_but_eq_does_not() {
        // SQLite's `LIKE` is ASCII-case-insensitive by default and `=` is not, so the two
        // predicates disagree about case. Pinned here because it is surprising, and because the
        // oracle property encodes the same rule — if one is wrong they would agree with each other
        // and both be wrong.
        let (store, _dir) = defined();
        put(&store, "Zellij", "x", 1);
        let matched = |pred: Pred| {
            store
                .fetch(NS, &FetchRequest::entity("agent").filter(pred))
                .unwrap()
                .len()
        };
        assert_eq!(matched(Pred::prefix("project", "zel")), 1);
        assert_eq!(matched(Pred::contains("project", "ELLI")), 1);
        assert_eq!(matched(Pred::eq("project", "zellij")), 0);
        assert_eq!(matched(Pred::eq("project", "Zellij")), 1);
    }

    #[test]
    fn an_underscore_in_a_needle_is_matched_literally_too() {
        let (store, _dir) = defined();
        put(&store, "a_b", "x", 1);
        put(&store, "axb", "y", 2);
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent").filter(Pred::contains("project", "a_b")),
            )
            .unwrap();
        assert_eq!(texts(&rows, "project"), vec!["a_b"]);
    }

    #[test]
    fn sorting_and_selecting_work_together() {
        let (store, _dir) = defined();
        put(&store, "a", "x", 3);
        put(&store, "b", "y", 1);
        put(&store, "c", "z", 2);
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .select(["project"])
                    .sort_by("updated_at_ms", SortDir::Desc),
            )
            .unwrap();
        assert_eq!(rows.columns, vec!["project"]);
        assert_eq!(texts(&rows, "project"), vec!["a", "c", "b"]);
    }

    #[test]
    fn a_bool_column_reads_back_as_a_bool() {
        let (store, _dir) = store();
        let def = EntityDef::new("flag", 1).attr(AttrDef::boolean("on"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        store
            .apply(NS, &[Mutation::put("flag").set("on", true).build()])
            .unwrap();
        let rows = store.fetch(NS, &FetchRequest::entity("flag")).unwrap();
        assert_eq!(column(&rows, "on"), vec![DataValue::Bool(true)]);
    }

    #[test]
    fn a_blob_round_trips() {
        let (store, _dir) = store();
        let def = EntityDef::new("raw", 1).attr(AttrDef::blob("bytes"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        let payload = vec![0u8, 1, 2, 255, 254];
        store
            .apply(
                NS,
                &[Mutation::put("raw").set("bytes", payload.clone()).build()],
            )
            .unwrap();
        let rows = store.fetch(NS, &FetchRequest::entity("raw")).unwrap();
        assert_eq!(column(&rows, "bytes"), vec![DataValue::Blob(payload)]);
    }

    // ----------------------------------------------------------------- fts

    #[test]
    fn search_finds_a_row_by_its_text() {
        let (store, _dir) = defined();
        put(&store, "zellij", "fixing a rebase conflict", 1);
        put(&store, "zellij", "writing the changelog", 2);
        let rows = store
            .fetch(NS, &FetchRequest::entity("agent").search("rebase"))
            .unwrap();
        assert_eq!(texts(&rows, "summary"), vec!["fixing a rebase conflict"]);
    }

    #[test]
    fn the_full_text_index_follows_updates_and_deletes() {
        let (store, _dir) = defined();
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("summary", "a rebase conflict")
                    .build()],
            )
            .unwrap();
        let search = |q: &str| {
            store
                .fetch(NS, &FetchRequest::entity("agent").search(q))
                .unwrap()
                .len()
        };
        assert_eq!(search("rebase"), 1);

        // an update has to remove the old terms as well as add the new ones
        store
            .apply(
                NS,
                &[Mutation::update("agent", Pred::eq("uid", "a1"))
                    .set("summary", "a merge commit")
                    .build()],
            )
            .unwrap();
        assert_eq!(search("rebase"), 0);
        assert_eq!(search("merge"), 1);

        store
            .apply(NS, &[Mutation::delete("agent", Pred::eq("uid", "a1"))])
            .unwrap();
        assert_eq!(search("merge"), 0);
    }

    #[test]
    fn search_combines_with_a_filter() {
        let (store, _dir) = defined();
        put(&store, "zellij", "a rebase conflict", 1);
        put(&store, "other", "a rebase conflict", 2);
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .search("rebase")
                    .filter(Pred::eq("project", "zellij")),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(texts(&rows, "project"), vec!["zellij"]);
    }

    #[test]
    fn searching_an_entity_with_nothing_searchable_says_so() {
        let (store, _dir) = store();
        let def = EntityDef::new("plain", 1).attr(AttrDef::text("name"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        let err = store
            .fetch(NS, &FetchRequest::entity("plain").search("x"))
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    // ---------------------------------------------------------- pagination

    #[test]
    fn a_fetch_is_capped_at_the_maximum_however_much_it_asks_for() {
        let (store, _dir) = defined();
        for i in 0..5 {
            put(&store, "zellij", &format!("s{}", i), i);
        }
        let rows = store
            .fetch(NS, &FetchRequest::entity("agent").limit(u32::MAX))
            .unwrap();
        assert_eq!(rows.len(), 5);
        assert!(!rows.truncated);
    }

    #[test]
    fn a_truncated_fetch_says_so_and_hands_back_a_cursor() {
        let (store, _dir) = defined();
        for i in 0..5 {
            put(&store, "zellij", &format!("s{}", i), i);
        }
        let rows = store
            .fetch(NS, &FetchRequest::entity("agent").limit(2))
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.truncated);
        assert!(rows.next_cursor.is_some());
    }

    #[test]
    fn a_cursor_is_withheld_when_the_ordering_it_pages_through_does_not_apply() {
        let (store, _dir) = defined();
        for i in 0..5 {
            put(&store, "zellij", &format!("s{}", i), i);
        }
        let rows = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .limit(2)
                    .sort_by("updated_at_ms", SortDir::Desc),
            )
            .unwrap();
        assert!(rows.truncated);
        // paging a custom sort by id would skip rows, so no cursor is offered for one
        assert!(rows.next_cursor.is_none());
    }

    #[test]
    fn combining_a_cursor_with_a_sort_is_an_error_rather_than_wrong_results() {
        let (store, _dir) = defined();
        let err = store
            .fetch(
                NS,
                &FetchRequest::entity("agent")
                    .after_id(1)
                    .sort_by("updated_at_ms", SortDir::Desc),
            )
            .unwrap_err();
        assert!(matches!(err, DataError::InvalidQuery(_)), "got {:?}", err);
    }

    #[test]
    fn count_ignores_the_limit() {
        let (store, _dir) = defined();
        for i in 0..5 {
            put(&store, "zellij", &format!("s{}", i), i);
        }
        assert_eq!(
            store
                .count(NS, &FetchRequest::entity("agent").limit(2))
                .unwrap(),
            5
        );
    }

    #[test]
    fn count_respects_the_filter_and_the_search() {
        let (store, _dir) = defined();
        put(&store, "zellij", "a rebase conflict", 1);
        put(&store, "zellij", "a merge commit", 2);
        put(&store, "other", "a rebase conflict", 3);
        assert_eq!(
            store
                .count(
                    NS,
                    &FetchRequest::entity("agent").filter(Pred::eq("project", "zellij"))
                )
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .count(NS, &FetchRequest::entity("agent").search("rebase"))
                .unwrap(),
            2
        );
    }

    // ---------------------------------------------------------- properties

    /// A string that exercises the FTS5 parser: bare words, the operators it recognises, and the
    /// punctuation and unbalanced quotes that make it a syntax error.
    #[hegel::composite]
    fn search_string(tc: &hegel::TestCase) -> String {
        let piece_count = tc.draw(generators::integers::<usize>().max_value(6));
        let mut pieces: Vec<String> = Vec::new();
        for _ in 0..piece_count {
            pieces.push(tc.draw(hegel::one_of!(
                generators::text().max_size(8),
                generators::sampled_from(vec![
                    "AND", "OR", "NOT", "NEAR", "\"", "'", "(", ")", "*", "^", ":", "-", "--",
                    "a b", "",
                ])
                .map(str::to_owned),
            )));
        }
        pieces.join(" ")
    }

    /// Anything a person can paste has to search rather than error — the same property the
    /// recording store holds, for the same reason: punctuation and unbalanced quotes are FTS5
    /// syntax errors, and are also exactly what comes off a log line.
    #[hegel::test(test_cases = 40)]
    fn any_search_string_searches_rather_than_erroring(tc: hegel::TestCase) {
        let query = tc.draw(search_string());
        let (store, _dir) = defined();
        put(&store, "zellij", "fixing a rebase conflict", 1);
        let result = store.fetch(NS, &FetchRequest::entity("agent").search(&query));
        assert!(
            result.is_ok(),
            "searching for {:?} failed: {:?}",
            query,
            result.err()
        );
    }

    /// Declaring an index is a statement about how rows are reached, never about which rows come
    /// back. This is the safety net under the adaptive indexing that comes later.
    #[hegel::test(test_cases = 25)]
    fn declaring_an_index_changes_the_plan_but_never_the_results(tc: hegel::TestCase) {
        let projects: Vec<String> = tc.draw(
            generators::vecs(generators::sampled_from(vec!["a", "b", "c"]).map(str::to_owned))
                .min_size(1)
                .max_size(12),
        );
        let needle = tc.draw(generators::sampled_from(vec!["a", "b", "c"]).map(str::to_owned));

        let results = |indexed: bool| {
            let (store, _dir) = store();
            let attr = if indexed {
                AttrDef::text("project").indexed()
            } else {
                AttrDef::text("project")
            };
            let def = EntityDef::new("agent", 1)
                .attr(attr)
                .attr(AttrDef::int("n"));
            store.define_entity(NS, OWNER, &def, false).unwrap();
            for (i, project) in projects.iter().enumerate() {
                store
                    .apply(
                        NS,
                        &[Mutation::put("agent")
                            .set("project", project.as_str())
                            .set("n", i as i64)
                            .build()],
                    )
                    .unwrap();
            }
            let rows = store
                .fetch(
                    NS,
                    &FetchRequest::entity("agent")
                        .filter(Pred::eq("project", needle.as_str()))
                        .select(["n"]),
                )
                .unwrap();
            column(&rows, "n")
        };

        assert_eq!(results(true), results(false));
    }

    /// Walking a set by cursor has to see every row exactly once. Anything else and a plugin paging
    /// through its own data silently loses or repeats rows.
    #[hegel::test(test_cases = 25)]
    fn paging_by_cursor_yields_every_row_exactly_once(tc: hegel::TestCase) {
        let row_count = tc.draw(generators::integers::<usize>().min_value(1).max_value(25));
        let page_size = tc.draw(generators::integers::<u32>().min_value(1).max_value(6));

        let (store, _dir) = defined();
        for i in 0..row_count {
            put(&store, "zellij", &format!("s{}", i), i as i64);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<i64> = None;
        // one iteration per row plus a margin, so that a bug which fails to advance terminates
        // instead of hanging
        for _ in 0..row_count + 5 {
            let mut request = FetchRequest::entity("agent").limit(page_size);
            if let Some(after) = cursor {
                request = request.after_id(after);
            }
            let rows = store.fetch(NS, &request).unwrap();
            seen.extend(texts(&rows, "summary"));
            match rows.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        let expected: Vec<String> = (0..row_count).map(|i| format!("s{}", i)).collect();
        assert_eq!(seen, expected);
    }

    /// What comes out of a put is what went in, whatever the mix of kinds.
    #[hegel::test(test_cases = 25)]
    fn a_put_round_trips_through_a_fetch(tc: hegel::TestCase) {
        let written: Vec<(String, i64, bool, Vec<u8>)> = tc.draw(
            generators::vecs(hegel::tuples!(
                generators::text().max_size(20),
                generators::integers::<i64>(),
                generators::booleans(),
                generators::binary().max_size(10),
            ))
            .min_size(1)
            .max_size(10),
        );

        let (store, _dir) = store();
        let def = EntityDef::new("mixed", 1)
            .attr(AttrDef::text("t"))
            .attr(AttrDef::int("i"))
            .attr(AttrDef::boolean("b"))
            .attr(AttrDef::blob("raw"));
        store.define_entity(NS, OWNER, &def, false).unwrap();

        for (t, i, b, raw) in &written {
            store
                .apply(
                    NS,
                    &[Mutation::put("mixed")
                        .set("t", t.clone())
                        .set("i", *i)
                        .set("b", *b)
                        .set("raw", raw.clone())
                        .build()],
                )
                .unwrap();
        }

        let fetched = store
            .fetch(NS, &FetchRequest::entity("mixed").limit(MAX_FETCH_ROWS))
            .unwrap();
        assert_eq!(fetched.len(), written.len());
        for (index, (t, i, b, raw)) in written.iter().enumerate() {
            let row = &fetched.rows[index];
            assert_eq!(fetched.get(row, "t"), Some(&DataValue::Text(t.clone())));
            assert_eq!(fetched.get(row, "i"), Some(&DataValue::Int(*i)));
            assert_eq!(fetched.get(row, "b"), Some(&DataValue::Bool(*b)));
            assert_eq!(fetched.get(row, "raw"), Some(&DataValue::Blob(raw.clone())));
        }
    }

    // ------------------------------------------------------------- watches

    const PLUGIN: u32 = 7;
    const CLIENT: ClientId = 1;

    fn touched(entity: &str) -> BTreeSet<String> {
        [entity.to_owned()].into_iter().collect()
    }

    #[test]
    fn a_watch_returns_the_current_rows_when_it_is_registered() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 1);
        let handle = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        // the rows come back with the handle, so there is no gap in which a change could be missed
        assert_eq!(texts(&handle.rows, "summary"), vec!["one"]);
        assert_eq!(store.watch_count(), 1);
    }

    #[test]
    fn a_write_that_changes_the_results_notifies_the_watch() {
        let (store, _dir) = defined();
        let handle = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        put(&store, "zellij", "one", 1);
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].watch_id, handle.watch_id);
        assert_eq!(changes[0].plugin_id, PLUGIN);
        assert_eq!(changes[0].client_id, CLIENT);
        assert_eq!(texts(&changes[0].rows, "summary"), vec!["one"]);
    }

    #[test]
    fn writing_the_same_values_again_is_silent() {
        let (store, _dir) = defined();
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("project", "zellij")
                    .build()],
            )
            .unwrap();
        // `updated_at` is not selected, so an idempotent upsert leaves the result set identical
        store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent").select(["project"]),
                WatchMode::Rows,
            )
            .unwrap();
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("project", "zellij")
                    .build()],
            )
            .unwrap();
        assert!(store
            .refresh_watches(NS, &touched("agent"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_watch_is_only_notified_once_per_change() {
        let (store, _dir) = defined();
        store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        put(&store, "zellij", "one", 1);
        assert_eq!(
            store.refresh_watches(NS, &touched("agent")).unwrap().len(),
            1
        );
        // the digest was updated by the first refresh, so a second one has nothing to report
        assert!(store
            .refresh_watches(NS, &touched("agent"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_write_to_another_entity_does_not_notify() {
        let (store, _dir) = defined();
        let other = EntityDef::new("todo", 1).attr(AttrDef::text("text"));
        store.define_entity(NS, OWNER, &other, false).unwrap();
        store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        store
            .apply(NS, &[Mutation::put("todo").set("text", "ship it").build()])
            .unwrap();
        assert!(store
            .refresh_watches(NS, &touched("todo"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_write_in_another_namespace_does_not_notify() {
        let (store, _dir) = store();
        store
            .define_entity("plugin:a", "a", &agent_def(), false)
            .unwrap();
        store
            .define_entity("plugin:b", "b", &agent_def(), false)
            .unwrap();
        store
            .register_watch(
                "plugin:a",
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        store
            .apply(
                "plugin:b",
                &[Mutation::put("agent").set("project", "elsewhere").build()],
            )
            .unwrap();
        assert!(store
            .refresh_watches("plugin:b", &touched("agent"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_watch_respects_its_filter() {
        let (store, _dir) = defined();
        store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent").filter(Pred::eq("project", "zellij")),
                WatchMode::Rows,
            )
            .unwrap();
        put(&store, "other", "not mine", 1);
        assert!(store
            .refresh_watches(NS, &touched("agent"))
            .unwrap()
            .is_empty());
        put(&store, "zellij", "mine", 2);
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(texts(&changes[0].rows, "summary"), vec!["mine"]);
    }

    #[test]
    fn an_ids_only_watch_sends_just_the_id_column() {
        let (store, _dir) = defined();
        let handle = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Ids,
            )
            .unwrap();
        assert_eq!(handle.rows.columns, vec!["id"]);
        put(&store, "zellij", "one", 1);
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        assert_eq!(changes[0].rows.columns, vec!["id"]);
    }

    #[test]
    fn a_deletion_that_empties_the_results_still_notifies() {
        let (store, _dir) = defined();
        put(&store, "zellij", "one", 1);
        store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        store
            .apply(
                NS,
                &[Mutation::delete("agent", Pred::eq("project", "zellij"))],
            )
            .unwrap();
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].rows.is_empty());
    }

    #[test]
    fn unwatching_stops_the_notifications() {
        let (store, _dir) = defined();
        let handle = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        store.unregister_watch(PLUGIN, handle.watch_id).unwrap();
        assert_eq!(store.watch_count(), 0);
        put(&store, "zellij", "one", 1);
        assert!(store
            .refresh_watches(NS, &touched("agent"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_plugin_cannot_unwatch_another_plugins_watch() {
        let (store, _dir) = defined();
        let handle = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        assert!(store.unregister_watch(PLUGIN + 1, handle.watch_id).is_err());
        assert_eq!(store.watch_count(), 1);
    }

    #[test]
    fn unloading_a_plugin_drops_its_watches_and_leaves_the_others() {
        let (store, _dir) = defined();
        for plugin_id in [PLUGIN, PLUGIN + 1] {
            store
                .register_watch(
                    NS,
                    plugin_id,
                    CLIENT,
                    FetchRequest::entity("agent"),
                    WatchMode::Rows,
                )
                .unwrap();
        }
        assert_eq!(store.watch_count(), 2);
        store.drop_watches_for_plugin(PLUGIN);
        assert_eq!(store.watch_count(), 1);
        put(&store, "zellij", "one", 1);
        let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].plugin_id, PLUGIN + 1);
    }

    #[test]
    fn watch_ids_are_not_reused_after_a_watch_is_dropped() {
        let (store, _dir) = defined();
        let first = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        store.unregister_watch(PLUGIN, first.watch_id).unwrap();
        let second = store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .unwrap();
        // reusing an id would let a stale event from the old watch be attributed to the new one
        assert_ne!(first.watch_id, second.watch_id);
    }

    #[test]
    fn watching_an_undeclared_entity_fails_rather_than_registering() {
        let (store, _dir) = store();
        assert!(store
            .register_watch(
                NS,
                PLUGIN,
                CLIENT,
                FetchRequest::entity("agent"),
                WatchMode::Rows,
            )
            .is_err());
        assert_eq!(store.watch_count(), 0);
    }

    // ------------------------------------------------------ the model machine

    /// The store driven against a `BTreeMap` holding what it ought to contain.
    ///
    /// A `BTreeMap` keyed on `uid` is the right model because `uid` is the store's own upsert key
    /// and the map's iteration order matches `ORDER BY uid` under SQLite's byte-wise collation, so
    /// the two can be compared directly without sorting either side.
    struct StoreModel {
        store: PluginDataStore,
        // held so the temp dir outlives the store
        _dir: TempDir,
        model: BTreeMap<String, (Option<String>, Option<i64>)>,
    }

    impl StoreModel {
        /// A small pool of keys, so that upserts and deletes actually collide instead of every
        /// operation touching a fresh row.
        fn any_uid(tc: &hegel::TestCase) -> String {
            tc.draw(generators::sampled_from(vec!["a", "b", "c", "d"]))
                .to_owned()
        }

        fn any_text(tc: &hegel::TestCase) -> Option<String> {
            tc.draw(generators::optional(
                generators::sampled_from(vec!["x", "y", "%", "_", "Z"]).map(str::to_owned),
            ))
        }

        fn any_number(tc: &hegel::TestCase) -> Option<i64> {
            tc.draw(generators::optional(
                generators::integers::<i64>().min_value(-2).max_value(2),
            ))
        }
    }

    #[hegel::state_machine]
    impl StoreModel {
        #[rule]
        fn put(&mut self, tc: hegel::TestCase) {
            let uid = Self::any_uid(&tc);
            let text = Self::any_text(&tc);
            let number = Self::any_number(&tc);
            self.store
                .apply(
                    NS,
                    &[Mutation::put("row")
                        .uid(uid.clone())
                        .set("t", text.clone())
                        .set("n", number)
                        .build()],
                )
                .unwrap();
            self.model.insert(uid, (text, number));
        }

        #[rule]
        fn delete_by_uid(&mut self, tc: hegel::TestCase) {
            let uid = Self::any_uid(&tc);
            self.store
                .apply(NS, &[Mutation::delete("row", Pred::eq("uid", uid.clone()))])
                .unwrap();
            self.model.remove(&uid);
        }

        #[rule]
        fn update_text_by_uid(&mut self, tc: hegel::TestCase) {
            let uid = Self::any_uid(&tc);
            let text = Self::any_text(&tc);
            self.store
                .apply(
                    NS,
                    &[Mutation::update("row", Pred::eq("uid", uid.clone()))
                        .set("t", text.clone())
                        .build()],
                )
                .unwrap();
            if let Some((stored_text, _)) = self.model.get_mut(&uid) {
                *stored_text = text;
            }
        }

        /// Delete a whole range at once, which no single-row rule would reach.
        #[rule]
        fn delete_where_number_below(&mut self, tc: hegel::TestCase) {
            let cutoff = tc.draw(generators::integers::<i64>().min_value(-2).max_value(2));
            self.store
                .apply(NS, &[Mutation::delete("row", Pred::lt("n", cutoff))])
                .unwrap();
            self.model
                .retain(|_, (_, number)| !matches!(number, Some(n) if *n < cutoff));
        }

        #[invariant]
        fn the_store_holds_what_the_model_holds(&self, _: hegel::TestCase) {
            let rows = self
                .store
                .fetch(
                    NS,
                    &FetchRequest::entity("row")
                        .select(["uid", "t", "n"])
                        .sort_by("uid", SortDir::Asc)
                        .limit(MAX_FETCH_ROWS),
                )
                .unwrap();
            let actual: Vec<(String, Option<String>, Option<i64>)> = rows
                .rows
                .iter()
                .map(|row| {
                    let text_of = |value: Option<&DataValue>| match value {
                        Some(DataValue::Text(s)) => Some(s.clone()),
                        Some(DataValue::Null) | None => None,
                        other => panic!("expected text, got {:?}", other),
                    };
                    let int_of = |value: Option<&DataValue>| match value {
                        Some(DataValue::Int(i)) => Some(*i),
                        Some(DataValue::Null) | None => None,
                        other => panic!("expected an int, got {:?}", other),
                    };
                    (
                        text_of(rows.get(row, "uid")).expect("uid is always set"),
                        text_of(rows.get(row, "t")),
                        int_of(rows.get(row, "n")),
                    )
                })
                .collect();
            let expected: Vec<(String, Option<String>, Option<i64>)> = self
                .model
                .iter()
                .map(|(uid, (text, number))| (uid.clone(), text.clone(), *number))
                .collect();
            assert_eq!(actual, expected);
        }
    }

    /// Any sequence of writes leaves the store holding exactly what a plain map would.
    ///
    /// Sequences are where stale-state bugs live — an upsert that fails to clear a previous value,
    /// a range delete that misses a row, an update that touches rows its filter excluded. No single
    /// example test covers the combinations.
    #[hegel::test(test_cases = 25)]
    fn the_store_agrees_with_a_map_over_any_sequence_of_writes(tc: hegel::TestCase) {
        let dir = tempdir().unwrap();
        let store = PluginDataStore::new(dir.path());
        let def = EntityDef::new("row", 1)
            .attr(AttrDef::text("t"))
            .attr(AttrDef::int("n").indexed());
        store.define_entity(NS, OWNER, &def, false).unwrap();
        let machine = StoreModel {
            store,
            _dir: dir,
            model: BTreeMap::new(),
        };
        hegel::stateful::run(machine, tc);
    }

    // ------------------------------------------- the predicate compiler oracle

    /// A row of the two-column entity the oracle property runs against.
    type OracleRow = (Option<String>, Option<i64>);

    /// Strings chosen to break a naive `LIKE` implementation: the wildcards, the escape character,
    /// mixed case (SQLite's `LIKE` is ASCII-case-insensitive by default while `=` is not), and
    /// multi-byte text so that byte-wise collation is exercised.
    #[hegel::composite]
    fn oracle_text(tc: &hegel::TestCase) -> String {
        tc.draw(generators::sampled_from(vec![
            "", "a", "A", "ab", "aB", "%", "_", "\\", "a%b", "a_b", "100%", "1000", "a\\b", "ß",
            "é", "日本", "aa", "Aa",
        ]))
        .to_owned()
    }

    #[hegel::composite]
    fn oracle_rows(tc: &hegel::TestCase) -> Vec<OracleRow> {
        let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(12));
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            rows.push((
                tc.draw(generators::optional(oracle_text())),
                tc.draw(generators::optional(
                    generators::integers::<i64>().min_value(-3).max_value(3),
                )),
            ));
        }
        rows
    }

    /// A type-correct predicate over the oracle entity's two columns.
    ///
    /// Type-correct because comparing a column against the wrong kind is rejected outright, which
    /// is its own (separately tested) behaviour rather than something this property is about.
    #[hegel::composite]
    fn oracle_pred(tc: &hegel::TestCase, depth: usize) -> Pred {
        let leaves = 11u8;
        let choice = if depth == 0 {
            tc.draw(generators::integers::<u8>().max_value(leaves - 1))
        } else {
            tc.draw(generators::integers::<u8>().max_value(leaves + 2))
        };
        // Whether this leaf talks about the text column or the integer one.
        let on_text = tc.draw(generators::booleans());
        let attr = if on_text { "t" } else { "n" };
        let value = |tc: &hegel::TestCase| -> DataValue {
            if tc.draw(generators::booleans()) {
                DataValue::Null
            } else if on_text {
                DataValue::Text(tc.draw(oracle_text()))
            } else {
                DataValue::Int(tc.draw(generators::integers::<i64>().min_value(-3).max_value(3)))
            }
        };
        // `In` lists exclude null: `x IN (a, NULL)` is three-valued in a way that says more about
        // SQL than about this compiler, and the null cases are covered by the comparisons.
        let non_null = |tc: &hegel::TestCase| -> DataValue {
            if on_text {
                DataValue::Text(tc.draw(oracle_text()))
            } else {
                DataValue::Int(tc.draw(generators::integers::<i64>().min_value(-3).max_value(3)))
            }
        };
        match choice {
            0 => Pred::Eq(attr.to_owned(), value(tc)),
            1 => Pred::Ne(attr.to_owned(), value(tc)),
            2 => Pred::Gt(attr.to_owned(), value(tc)),
            3 => Pred::Gte(attr.to_owned(), value(tc)),
            4 => Pred::Lt(attr.to_owned(), value(tc)),
            5 => Pred::Lte(attr.to_owned(), value(tc)),
            6 => {
                let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
                Pred::In(attr.to_owned(), (0..count).map(|_| non_null(tc)).collect())
            },
            7 => Pred::IsNull(attr.to_owned()),
            8 => Pred::NotNull(attr.to_owned()),
            // Always the text column: a prefix or substring match against the integer one is
            // rejected outright, which `prefix_and_contains_are_rejected_on_a_non_text_column`
            // covers rather than this property.
            9 => Pred::Prefix("t".to_owned(), tc.draw(oracle_text())),
            10 => Pred::Contains("t".to_owned(), tc.draw(oracle_text())),
            11 => {
                let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
                Pred::And(
                    (0..count)
                        .map(|_| tc.draw(oracle_pred(depth - 1)))
                        .collect(),
                )
            },
            12 => {
                let count = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
                Pred::Or(
                    (0..count)
                        .map(|_| tc.draw(oracle_pred(depth - 1)))
                        .collect(),
                )
            },
            _ => Pred::Not(Box::new(tc.draw(oracle_pred(depth - 1)))),
        }
    }

    /// Evaluate a predicate against one row the way SQL would.
    ///
    /// `None` is SQL's third truth value: a comparison involving NULL is unknown, and `WHERE` keeps
    /// a row only on `Some(true)`. `And`/`Or` follow Kleene logic, which is why they cannot simply
    /// be `all`/`any` over booleans.
    fn oracle_eval(pred: &Pred, row: &OracleRow) -> Option<bool> {
        let (text, number) = row;
        // SQLite's default collation compares text byte-wise, which is also how Rust orders `str`.
        let compare = |value: &DataValue| -> Option<std::cmp::Ordering> {
            match value {
                DataValue::Null => None,
                DataValue::Text(v) => text.as_ref().map(|t| t.as_str().cmp(v.as_str())),
                DataValue::Int(v) => number.map(|n| n.cmp(v)),
                other => panic!("the oracle entity has no column for {:?}", other),
            }
        };
        // `LIKE` is ASCII-case-insensitive by default in SQLite, unlike `=`.
        let like = |needle: &str, f: fn(&str, &str) -> bool| -> Option<bool> {
            text.as_ref()
                .map(|t| f(&t.to_ascii_lowercase(), &needle.to_ascii_lowercase()))
        };
        match pred {
            Pred::Eq(_, v) => compare(v).map(|o| o.is_eq()),
            Pred::Ne(_, v) => compare(v).map(|o| o.is_ne()),
            Pred::Gt(_, v) => compare(v).map(|o| o.is_gt()),
            Pred::Gte(_, v) => compare(v).map(|o| o.is_ge()),
            Pred::Lt(_, v) => compare(v).map(|o| o.is_lt()),
            Pred::Lte(_, v) => compare(v).map(|o| o.is_le()),
            Pred::In(_, values) => {
                let mut any_unknown = false;
                let mut found = false;
                for value in values {
                    match compare(value) {
                        Some(std::cmp::Ordering::Equal) => found = true,
                        Some(_) => {},
                        None => any_unknown = true,
                    }
                }
                if found {
                    Some(true)
                } else if any_unknown {
                    None
                } else {
                    Some(false)
                }
            },
            Pred::IsNull(attr) => Some(if attr == "t" {
                text.is_none()
            } else {
                number.is_none()
            }),
            Pred::NotNull(attr) => Some(if attr == "t" {
                text.is_some()
            } else {
                number.is_some()
            }),
            Pred::Prefix(_, v) => like(v, |h, n| h.starts_with(n)),
            Pred::Contains(_, v) => like(v, |h, n| h.contains(n)),
            Pred::And(preds) => {
                let mut unknown = false;
                for inner in preds {
                    match oracle_eval(inner, row) {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        Some(true) => {},
                    }
                }
                if unknown {
                    None
                } else {
                    Some(true)
                }
            },
            Pred::Or(preds) => {
                let mut unknown = false;
                for inner in preds {
                    match oracle_eval(inner, row) {
                        Some(true) => return Some(true),
                        None => unknown = true,
                        Some(false) => {},
                    }
                }
                if unknown {
                    None
                } else {
                    Some(false)
                }
            },
            Pred::Not(inner) => oracle_eval(inner, row).map(|b| !b),
        }
    }

    /// The rows a filter selects are exactly the rows that satisfy it.
    ///
    /// This is the strongest property available for the query compiler: it checks the generated SQL
    /// against an independent evaluator rather than against another query. It covers `LIKE`
    /// escaping, three-valued null logic, collation, and boolean nesting in one place — the
    /// escaping bug that the two hand-written `%`/`_` cases caught is the kind of thing it exists
    /// to find more of.
    #[hegel::test(test_cases = 60)]
    fn a_filter_selects_exactly_the_rows_that_satisfy_it(tc: hegel::TestCase) {
        let rows = tc.draw(oracle_rows());
        let pred = tc.draw(oracle_pred(3));

        let (store, _dir) = store();
        let def = EntityDef::new("oracle", 1)
            .attr(AttrDef::text("t"))
            .attr(AttrDef::int("n"));
        store.define_entity(NS, OWNER, &def, false).unwrap();
        // One batch, so ids are 1..=rows.len() in the order they were generated.
        let mutations: Vec<Mutation> = rows
            .iter()
            .map(|(text, number)| {
                Mutation::put("oracle")
                    .set("t", text.clone())
                    .set("n", *number)
                    .build()
            })
            .collect();
        store.apply(NS, &mutations).unwrap();

        let fetched = store
            .fetch(
                NS,
                &FetchRequest::entity("oracle")
                    .filter(pred.clone())
                    .select(["id"])
                    .limit(MAX_FETCH_ROWS),
            )
            .unwrap();
        let actual: Vec<i64> = column(&fetched, "id")
            .into_iter()
            .map(|v| match v {
                DataValue::Int(i) => i,
                other => panic!("expected an id, got {:?}", other),
            })
            .collect();

        let expected: Vec<i64> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| oracle_eval(&pred, row) == Some(true))
            .map(|(index, _)| index as i64 + 1)
            .collect();

        assert_eq!(
            actual, expected,
            "filter {:?} over rows {:?} selected {:?}, expected {:?}",
            pred, rows, actual, expected
        );
    }

    /// A watch reports exactly when the rows it would fetch differ. Checking the digest against a
    /// fresh fetch is the property the digest exists to provide — a false negative is a UI that
    /// silently stops updating.
    #[hegel::test(test_cases = 25)]
    fn a_watch_reports_a_change_exactly_when_the_rows_differ(tc: hegel::TestCase) {
        #[derive(Debug, Clone)]
        enum Step {
            Insert(String),
            DeleteAll,
            RewriteSame,
        }

        let steps: Vec<Step> = tc.draw(
            generators::vecs(hegel::one_of!(
                generators::sampled_from(vec!["a", "b", "c"]).map(|s| Step::Insert(s.to_owned())),
                generators::just(Step::DeleteAll),
                generators::just(Step::RewriteSame),
            ))
            .min_size(1)
            .max_size(10),
        );

        let (store, _dir) = defined();
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("fixed")
                    .set("project", "fixed")
                    .build()],
            )
            .unwrap();
        let request = FetchRequest::entity("agent").select(["project"]);
        let handle = store
            .register_watch(NS, PLUGIN, CLIENT, request.clone(), WatchMode::Rows)
            .unwrap();
        let mut last_seen = handle.rows.clone();

        for step in steps {
            match step {
                Step::Insert(project) => put(&store, &project, "x", 1),
                Step::DeleteAll => {
                    store
                        .apply(NS, &[Mutation::delete("agent", Pred::not_null("project"))])
                        .unwrap();
                },
                Step::RewriteSame => {
                    store
                        .apply(
                            NS,
                            &[Mutation::put("agent")
                                .uid("fixed")
                                .set("project", "fixed")
                                .build()],
                        )
                        .unwrap();
                },
            }
            let expected = store.fetch(NS, &request).unwrap();
            let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
            if expected == last_seen {
                assert!(
                    changes.is_empty(),
                    "reported a change when the rows were identical: {:?}",
                    expected
                );
            } else {
                assert_eq!(
                    changes.len(),
                    1,
                    "failed to report a change from {:?} to {:?}",
                    last_seen,
                    expected
                );
                assert_eq!(changes[0].rows, expected);
                last_seen = expected;
            }
        }
    }
}
