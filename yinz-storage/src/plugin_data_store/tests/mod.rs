mod dialect;
mod sqlite;

use super::*;
// The flat module's own `use` block used to reach the tests through `use super::*`. Now that the
// engine is split up, the types plugins speak in are imported here directly.
use std::collections::BTreeMap;
use yinz_types::plugin_data::*;

/// Assert that an error has a particular shape, and say which shape was wanted when it doesn't.
///
/// Takes a pattern rather than a variant so that a test which accepts either of two errors can say
/// so: `assert_err!(err, DataError::InvalidName(_) | DataError::InvalidQuery(_))`.
macro_rules! assert_err {
    ($err:expr_2021, $($pattern:tt)+) => {
        assert!(
            matches!($err, $($pattern)+),
            "expected {}, got {:?}",
            stringify!($($pattern)+),
            $err
        )
    };
}
pub(crate) use assert_err;

const NS: &str = "plugin:test";
const OWNER: &str = "yinz:test";

/// The plugin and client a watch is registered for. Shared with the watch property test in
/// `oracle`.
const PLUGIN: PluginId = PluginId::new(7);
const CLIENT: ClientId = ClientId::new(1);

/// The touched-entity set `refresh_watches` filters on, for a write to one entity.
fn touched(entity: &str) -> BTreeSet<String> {
    [entity.to_owned()].into_iter().collect()
}

/// The entity most tests use: two indexed columns, one searchable one, one plain.
fn agent_def() -> EntityDef {
    EntityDef::new("agent", 1)
        .attr(AttrDef::text("project").indexed())
        .attr(AttrDef::text("branch").indexed())
        .attr(AttrDef::text("summary").full_text())
        .attr(AttrDef::int("updated_at_ms").indexed())
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
