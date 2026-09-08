use super::*;
use crate::plugin_data_store::registry::require_entity;
use crate::plugin_data_store::tests::postgres::Guard;
use zellij_types::plugin_data::{AttrDef, Pred};

#[test]
fn guarded_batches_exclude_unguarded_writers_on_all_touched_tables_until_commit() {
    let guard = Guard::fresh();
    let backend = guard.backend();
    for entity in ["todo", "link"] {
        backend
            .define_entity(
                "test",
                "test",
                &EntityDef::new(entity, 1).attr(AttrDef::text("root")),
                false,
            )
            .unwrap();
    }
    let mut other = ::postgres::Client::connect(&guard.backend_url(), ::postgres::NoTls).unwrap();
    let tables = backend
        .in_transaction("test", |exec| {
            engine::apply(
                exec,
                "test",
                &[
                    Mutation::AssertRows {
                        entity: "todo".into(),
                        filter: Pred::eq("root", "/work"),
                        rows: vec![],
                    },
                    Mutation::delete("link", Pred::eq("root", "/work")),
                ],
            )?;
            let mut tables = Vec::new();
            for entity in ["todo", "link"] {
                let table = format!(
                    "{}.{}",
                    quote_ident(&guard.schema),
                    quote_ident(&require_entity(exec, "test", entity)?.table())
                );
                // ROW EXCLUSIVE is the lock an ordinary INSERT/UPDATE/DELETE needs.
                // NOWAIT makes this a deterministic check without a timing assumption.
                let mut tx = other.transaction().unwrap();
                let error = tx
                    .batch_execute(&format!("LOCK TABLE {table} IN ROW EXCLUSIVE MODE NOWAIT"))
                    .unwrap_err();
                assert_eq!(
                    error.code(),
                    Some(&::postgres::error::SqlState::LOCK_NOT_AVAILABLE)
                );
                tx.rollback().unwrap();
                tables.push(table);
            }
            Ok(tables)
        })
        .unwrap();
    let mut tx = other.transaction().unwrap();
    for table in tables {
        tx.batch_execute(&format!("LOCK TABLE {table} IN ROW EXCLUSIVE MODE NOWAIT"))
            .unwrap();
    }
    tx.commit().unwrap();
}
