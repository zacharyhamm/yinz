use super::*;
use crate::plugin_data_store::registry::require_entity;
use crate::plugin_data_store::sql::quote_ident;
use zellij_types::plugin_data::{AttrDef, Pred};

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
