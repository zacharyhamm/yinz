use super::*;

#[test]
fn archive_order_indexes_support_fresh_and_upgraded_history_queries() {
    use crate::plugin_data_store::{
        query::build_fetch_sql, schema::EntitySchema, sqlite::dialect::SQLITE, value::SqlValue,
    };
    use yinz_types::agent::history::Cursor;
    let namespace = "shared:yinz";
    for upgrade in [false, true] {
        let (store, dir) = store();
        let mut def = EntityDef::new("agent_run_entry", 7)
            .attr(AttrDef::text("run_id").indexed())
            .attr(AttrDef::int("revision"))
            .attr(AttrDef::int("created_at_ms").indexed())
            .attr(AttrDef::text("kind").indexed());
        if upgrade {
            def.version = 6;
            store.define_entity(namespace, OWNER, &def, false).unwrap();
            store
                .apply(
                    namespace,
                    &[Mutation::put(&def.name)
                        .set("run_id", "r")
                        .set("revision", 1_i64)
                        .build()],
                )
                .unwrap();
            def.version = 7;
        }
        store.define_entity(namespace, OWNER, &def, false).unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("plugin-data.db")).unwrap();
        let schema = EntitySchema {
            row_id: conn
                .query_row(
                    "SELECT id FROM store_entities WHERE namespace=?1 AND entity=?2",
                    [namespace, &def.name],
                    |row| row.get(0),
                )
                .unwrap(),
            def,
            owner_plugin: OWNER.into(),
        };
        let base = FetchRequest::entity(&schema.def.name).filter(Pred::eq("run_id", "r"));
        let latest = base
            .clone()
            .sort_by("revision", SortDir::Desc)
            .select(["revision", "run_id", "kind"])
            .limit(1);
        let page = base
            .filter(Pred::Not(Box::new(Pred::is_in(
                "kind",
                ["tool_call", "tool_result"],
            ))))
            .sort_by("created_at_ms", SortDir::Desc)
            .sort_by("revision", SortDir::Desc)
            .limit(100);
        let cursor = Cursor {
            created_at_ms: Some(50),
            revision: 7,
        };
        for (request, index) in [
            (latest, "archive_revision"),
            (page.clone(), "archive_chronology"),
            (page.filter(cursor.before()), "archive_chronology"),
        ] {
            let query = build_fetch_sql(&SQLITE, &schema, &request, None).unwrap();
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {}", query.sql))
                .unwrap();
            let detail = stmt
                .query_map(
                    rusqlite::params_from_iter(query.params.iter().map(SqlValue)),
                    |r| r.get::<_, String>(3),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            assert!(
                detail.contains(index) && !detail.contains("TEMP B-TREE"),
                "{detail}"
            );
        }
        assert_eq!(
            store
                .count(namespace, &FetchRequest::entity(&schema.def.name))
                .unwrap(),
            u64::from(upgrade)
        );
    }
}
