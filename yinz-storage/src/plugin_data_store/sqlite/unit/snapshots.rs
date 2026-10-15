use super::*;
use crate::plugin_data_store::registry::require_entity;
use crate::plugin_data_store::sql::quote_ident;
use yinz_types::plugin_data::{AttrDef, Pred};

#[test]
fn injected_clock_stamps_each_atomic_batch_and_preserves_creation_time() {
    use std::sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    };
    use yinz_types::plugin_data::DataValue;
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(1_000));
    let stamp = clock.clone();
    let backend =
        SqlitePluginDataBackend::with_clock(dir.path(), move || stamp.load(Ordering::SeqCst));
    backend
        .define_entity(
            "test",
            "test",
            &EntityDef::new("note", 1).attr(AttrDef::text("body")),
            false,
        )
        .unwrap();
    backend
        .apply(
            "test",
            &[
                Mutation::put("note")
                    .uid("first")
                    .set("body", "one")
                    .build(),
                Mutation::put("note")
                    .uid("second")
                    .set("body", "two")
                    .build(),
            ],
        )
        .unwrap();
    clock.store(2_000, Ordering::SeqCst);
    backend
        .apply(
            "test",
            &[Mutation::update("note", Pred::eq("uid", "first"))
                .set("body", "changed")
                .build()],
        )
        .unwrap();
    let rows = backend
        .fetch("test", &FetchRequest::entity("note"))
        .unwrap();
    for row in &rows.rows {
        assert_eq!(rows.get(row, "created_at"), Some(&DataValue::Int(1_000)));
        let expected = if rows.get(row, "uid") == Some(&DataValue::Text("first".into())) {
            2_000
        } else {
            1_000
        };
        assert_eq!(rows.get(row, "updated_at"), Some(&DataValue::Int(expected)));
    }
}

#[test]
fn asserted_snapshots_exclude_unguarded_writers_until_commit() {
    let dir = tempfile::tempdir().unwrap();
    let backend = SqlitePluginDataBackend::new(dir.path());
    backend
        .define_entity(
            "test",
            "test",
            &EntityDef::new("todo", 1).attr(AttrDef::text("root")),
            false,
        )
        .unwrap();
    let other = Connection::open(&backend.db_path).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    let sql = backend
        .in_transaction("test", |exec| {
            engine::apply(
                exec,
                "test",
                &[Mutation::AssertRows {
                    entity: "todo".into(),
                    filter: Pred::eq("root", "/work"),
                    rows: vec![],
                }],
                1,
            )?;
            let table = quote_ident(&require_entity(exec, "test", "todo")?.table());
            let sql = format!(
                "INSERT INTO {table} (root, created_at, updated_at) VALUES ('/work', 1, 1)"
            );
            let error = other.execute(&sql, []).unwrap_err();
            assert_eq!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseBusy)
            );
            Ok(sql)
        })
        .unwrap();
    assert_eq!(other.execute(&sql, []).unwrap(), 1);
}
