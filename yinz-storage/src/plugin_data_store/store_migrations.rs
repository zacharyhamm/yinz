//! The store's own tables, and how a database created by an older yinz is brought forward.
//!
//! Distinct from [`super::migrate`], which is about a *plugin's* entity changing shape. This is
//! about the store itself changing shape, and it applies to every entity at once.
//!
//! The steps are Rust rather than SQL strings because the interesting ones are not expressible as
//! a statement: adding a column to every entity table means reading the registry first to learn
//! which tables exist. Each step is written once against [`Executor`] using backend-provided SQL,
//! asking [`Dialect`] for backend-specific syntax — the same arrangement the rest of
//! the store uses.
//!
//! A step is run once per database, guarded by the version in `store_meta`; steps need not be
//! idempotent. Steps are append-only: editing one already released means a database
//! that ran the old version never gets the new one.

use yinz_types::plugin_data::{AttrKind, DataError, DataValue};

use super::executor::{query_all, Executor};
use super::sql::{quote_ident, StoreContext};

/// One step forward. Applied in order, from the recorded version to [`current_version`].
type Step = fn(&mut dyn Executor) -> Result<(), DataError>;

const MIGRATIONS: &[Step] = &[
    create_registry,
    add_rev_to_every_entity,
    separate_private_namespaces,
    rename_builtin_project_entities,
];

/// The key `store_meta` holds the schema version under.
pub(super) const VERSION_KEY: &str = "schema_version";

/// The version a database is at once every step has been applied.
pub(super) fn current_version() -> i64 {
    MIGRATIONS.len() as i64
}

/// Apply every step past `recorded`, and report the version that leaves the database at.
///
/// A database *newer* than this binary is refused rather than downgraded: the steps only go
/// forward, and a yinz that does not know what a later one did cannot safely write to what it
/// produced. Servers sharing a database may run different versions.
pub(super) fn run(exec: &mut dyn Executor, recorded: i64) -> Result<i64, DataError> {
    let current = current_version();
    if recorded > current {
        return Err(DataError::Storage(format!(
            "the plugin data store is at version {}, newer than this yinz's {}; upgrade yinz \
             or point it at another database",
            recorded, current
        )));
    }
    for (index, step) in MIGRATIONS.iter().enumerate().skip(recorded.max(0) as usize) {
        step(exec).with_context(|| format!("failed to apply store migration {}", index + 1))?;
    }
    Ok(current)
}

/// 1: the entity registry.
fn create_registry(exec: &mut dyn Executor) -> Result<(), DataError> {
    let ddl = exec.dialect().registry_ddl();
    exec.execute_batch(ddl)
}

/// 2: `rev`, the row revision every entity table carries.
///
/// New tables get it from `Dialect::store_columns`; this is for the ones already out there. The
/// default puts every existing row at revision 1, which is where a row written after this lands
/// too — an unwritten row and a once-written row are the same thing to a reader, and pretending
/// existing rows had a history they never had would be worse.
fn add_rev_to_every_entity(exec: &mut dyn Executor) -> Result<(), DataError> {
    let column = exec.dialect().rev_column_added();
    let rows = query_all(exec, "SELECT id FROM store_entities", &[], &[AttrKind::Int])
        .context("failed to list the entity tables")?;
    for row in rows {
        let Some(DataValue::Int(row_id)) = row.first() else {
            return Err(DataError::Storage(
                "the entity registry returned a row id that is not an integer".to_owned(),
            ));
        };
        let table = quote_ident(&format!("e_{}", row_id));
        exec.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN \"rev\" {column};"))
            .with_context(|| format!("failed to add `rev` to entity table {}", table))?;
    }
    Ok(())
}

/// 3: separate private namespaces. Before this release migrates the database, stop every
/// server using it. Mixed-version live operation
/// is intentionally unsupported; this release adds no coordination mechanism.
fn separate_private_namespaces(exec: &mut dyn Executor) -> Result<(), DataError> {
    let rows = query_all(
        exec,
        "SELECT id, namespace, owner_plugin FROM store_entities",
        &[],
        &[AttrKind::Int, AttrKind::Text, AttrKind::Text],
    )?;
    for row in rows {
        let [DataValue::Int(id), DataValue::Text(namespace), DataValue::Text(owner)] =
            row.as_slice()
        else {
            return Err(DataError::Storage(
                "invalid entity ownership in registry".to_owned(),
            ));
        };
        let legacy: String = owner
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if namespace != &format!("plugin:{legacy}") {
            continue;
        }
        // A collided namespace belongs to the recorded declarer. Never copy its rows to
        // another plugin that happened to address the same legacy namespace.
        let mut params = super::dialect::Params::new(exec.dialect());
        let namespace = params.bind(DataValue::Text(super::private_namespace(owner)));
        let id = params.bind(DataValue::Int(*id));
        exec.execute(
            &format!("UPDATE store_entities SET namespace = {namespace} WHERE id = {id}"),
            &params.into_values(),
        )
        .context("failed to separate a private plugin namespace")?;
    }
    Ok(())
}

/// 4: rename builtin metadata without touching entity tables or their rows.
/// All servers sharing this database must be stopped before upgrading.
fn rename_builtin_project_entities(exec: &mut dyn Executor) -> Result<(), DataError> {
    let rows = query_all(
        exec,
        "SELECT id, namespace, owner_plugin, entity FROM store_entities",
        &[],
        &[
            AttrKind::Int,
            AttrKind::Text,
            AttrKind::Text,
            AttrKind::Text,
        ],
    )?;
    let renames = [
        ("yinz:hocus", "yinz:retired-chat"),
        ("yinz:hocus-agents", "yinz:agents"),
        ("yinz:hocus-agentd", "yinz:agent-service"),
        ("yinz:hocus-todos", "yinz:todos"),
        ("yinz:hocus-new-agent", "yinz:new-agent"),
        ("yinz:hocus-store", "yinz:project-store"),
        ("yinz:hocus-proto", "yinz:agent-proto"),
        ("yinz:hocus-data", "yinz:project-data"),
    ];
    let mut destinations = std::collections::BTreeSet::new();
    let mut updates = Vec::new();
    for row in rows {
        let [DataValue::Int(id), DataValue::Text(namespace), DataValue::Text(owner), DataValue::Text(name)] =
            row.as_slice()
        else {
            return Err(DataError::Storage(
                "invalid entity ownership in registry".into(),
            ));
        };
        let mut destination = namespace.clone();
        if namespace == "shared:hocus" {
            destination = if name == "job" {
                "plugin:yinz:retired-chat"
            } else {
                "shared:yinz"
            }
            .into();
        }
        let mut new_owner = owner.clone();
        for (old, new) in renames {
            if owner == old {
                new_owner = new.into();
            }
            if namespace == &super::private_namespace(old) {
                destination = super::private_namespace(new);
            }
        }
        if !destinations.insert((destination.clone(), name.clone())) {
            return Err(DataError::Storage(format!(
                "builtin rename collides with entity `{name}` in `{destination}`"
            )));
        }
        if destination != *namespace || new_owner != *owner {
            updates.push((*id, destination, new_owner));
        }
    }
    for (id, namespace, owner) in updates {
        let mut params = super::dialect::Params::new(exec.dialect());
        let namespace = params.bind(DataValue::Text(namespace));
        let owner = params.bind(DataValue::Text(owner));
        let id = params.bind(DataValue::Int(id));
        exec.execute(&format!("UPDATE store_entities SET namespace = {namespace}, owner_plugin = {owner} WHERE id = {id}"), &params.into_values())
            .context("failed to rename builtin entity metadata")?;
    }
    Ok(())
}

/// Read the recorded version, treating a database with no `store_meta` row as version 0.
///
/// Reject corrupt versions: replaying migrations can fail or alter an already-migrated database.
pub(super) fn recorded_version(exec: &mut dyn Executor) -> Result<i64, DataError> {
    let meta_ddl = exec.dialect().store_meta_ddl();
    exec.execute_batch(meta_ddl)
        .context("failed to create the store's metadata table")?;
    let mut params = super::dialect::Params::new(exec.dialect());
    let key = params.bind(DataValue::Text(VERSION_KEY.to_owned()));
    let sql = format!("SELECT value FROM store_meta WHERE key = {key}");
    let rows = query_all(exec, &sql, &params.into_values(), &[AttrKind::Text])
        .context("failed to read the store's schema version")?;
    let version = match rows.first().and_then(|row| row.first()) {
        None => return Ok(0),
        Some(DataValue::Text(value)) => value.parse::<i64>().ok(),
        _ => None,
    };
    version.filter(|version| *version >= 0).ok_or_else(|| {
        DataError::Storage(
            "invalid plugin data store schema_version; expected a nonnegative integer".into(),
        )
    })
}

/// Record the version reached, if it moved.
pub(super) fn record_version(exec: &mut dyn Executor, version: i64) -> Result<(), DataError> {
    let mut params = super::dialect::Params::new(exec.dialect());
    let key = params.bind(DataValue::Text(VERSION_KEY.to_owned()));
    let value = params.bind(DataValue::Text(version.to_string()));
    let sql = format!(
        "INSERT INTO store_meta (key, value) VALUES ({key}, {value}) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value"
    );
    exec.execute(&sql, &params.into_values())
        .context("failed to record the store's schema version")?;
    Ok(())
}

/// Bring the store's own tables up to date, in one pass.
///
/// The caller supplies the transaction and has already taken whatever lock serializes schema
/// changes on its engine, because two servers opening one database at once must not both decide
/// a step has not run.
pub(super) fn migrate_store(exec: &mut dyn Executor) -> Result<(), DataError> {
    let recorded = recorded_version(exec)?;
    let reached = run(exec, recorded)?;
    if reached != recorded {
        record_version(exec, reached)?;
    }
    Ok(())
}
