//! The SQL each dialect renders, checked without a database.
//!
//! These tests pin the SQL produced by the SQLite dialect.

use super::*;
use crate::plugin_data_store::dialect::Dialect;
use crate::plugin_data_store::query::{build_count_sql, build_fetch_sql};
use crate::plugin_data_store::schema::EntitySchema;
use crate::plugin_data_store::sqlite::dialect::SQLITE;
use crate::plugin_data_store::write::{build_delete, build_put, build_update};

fn schema() -> EntitySchema {
    EntitySchema {
        row_id: 7,
        def: EntityDef::new("agent", 1)
            .attr(AttrDef::text("project"))
            .attr(AttrDef::text("summary").full_text())
            .attr(AttrDef::text("notes").full_text())
            .attr(AttrDef::real("score"))
            .attr(AttrDef::boolean("on")),
        owner_plugin: OWNER.to_owned(),
    }
}

#[test]
fn sqlite_numbers_its_placeholders() {
    let request = FetchRequest::entity("agent")
        .filter(Pred::eq("project", "yinz").and(Pred::is_in("score", [1i64, 2])))
        .select(["project"])
        .limit(10)
        .offset(3);
    let fetch = build_fetch_sql(&SQLITE, &schema(), &request, None).unwrap();
    assert_eq!(
        fetch.sql,
        "SELECT \"e_7\".\"id\", \"e_7\".\"project\" FROM \"e_7\" \
         WHERE (\"e_7\".\"project\" = ?1 AND \"e_7\".\"score\" IN (?2, ?3)) \
         ORDER BY \"e_7\".\"id\" ASC LIMIT ?4 OFFSET ?5"
    );
    assert_eq!(
        fetch.params,
        vec![
            DataValue::Text("yinz".into()),
            DataValue::Int(1),
            DataValue::Int(2),
            DataValue::Int(11),
            DataValue::Int(3),
        ]
    );
}

#[test]
fn null_placement_is_spelled_out_on_every_sort() {
    let request = FetchRequest::entity("agent")
        .sort_by("score", SortDir::Desc)
        .sort_by("project", SortDir::Asc);
    let fetch = build_fetch_sql(&SQLITE, &schema(), &request, None).unwrap();
    assert!(
        fetch.sql.contains(
            "ORDER BY \"e_7\".\"score\" DESC NULLS LAST, \"e_7\".\"project\" ASC NULLS FIRST, \
             \"e_7\".\"id\" ASC"
        ),
        "{}",
        fetch.sql
    );
}

#[test]
fn sqlite_matches_patterns_with_like_and_a_named_escape() {
    let request = FetchRequest::entity("agent").filter(Pred::prefix("project", "100%"));
    let fetch = build_fetch_sql(&SQLITE, &schema(), &request, None).unwrap();
    assert!(
        fetch
            .sql
            .contains("WHERE \"e_7\".\"project\" LIKE ?1 ESCAPE '\\'"),
        "{}",
        fetch.sql
    );
    assert_eq!(fetch.params[0], DataValue::Text("100\\%%".into()));
}

#[test]
fn sqlite_searches_through_its_fts_table() {
    let request = FetchRequest::entity("agent").search("rebase");
    let (sql, params) = build_count_sql(&SQLITE, &schema(), &request, Some("rebase")).unwrap();
    assert_eq!(
        sql,
        "SELECT COUNT(*) FROM \"e_7\" JOIN \"e_7_fts\" ON \"e_7_fts\".rowid = \"e_7\".\"id\" \
         WHERE \"e_7_fts\" MATCH ?1"
    );
    assert_eq!(params, vec![DataValue::Text("rebase".into())]);
    let fetch = build_fetch_sql(&SQLITE, &schema(), &request, Some("rebase")).unwrap();
    assert!(
        fetch
            .sql
            .contains("ORDER BY bm25(\"e_7_fts\") ASC, \"e_7\".\"id\" ASC"),
        "{}",
        fetch.sql
    );
}

#[test]
fn a_put_with_a_uid_is_an_upsert_that_keeps_created_at() {
    let values = vec![("project".to_owned(), DataValue::Text("yinz".into()))];
    let (sql, params) = build_put(&SQLITE, &schema(), Some("a1"), &values, 5, None);
    assert_eq!(
        sql,
        "INSERT INTO \"e_7\" (\"uid\", \"created_at\", \"updated_at\", \"project\") \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(\"uid\") DO UPDATE SET \
         \"project\" = excluded.\"project\", \"updated_at\" = excluded.\"updated_at\", \
         \"rev\" = \"e_7\".\"rev\" + 1 RETURNING \"id\""
    );
    assert_eq!(
        params,
        vec![
            DataValue::Text("a1".into()),
            DataValue::Int(5),
            DataValue::Int(5),
            DataValue::Text("yinz".into()),
        ]
    );
    let (sql, _) = build_put(&SQLITE, &schema(), None, &values, 5, None);
    assert!(!sql.contains("ON CONFLICT"), "{}", sql);
}

#[test]
fn updates_and_deletes_bind_their_filters_after_their_assignments() {
    let values = vec![("project".to_owned(), DataValue::Text("x".into()))];
    let (sql, params) =
        build_update(&SQLITE, &schema(), &Pred::eq("uid", "a1"), &values, 9, None).unwrap();
    assert_eq!(
        sql,
        "UPDATE \"e_7\" SET \"project\" = ?1, \"updated_at\" = ?2, \"rev\" = \"rev\" + 1 WHERE \"e_7\".\"uid\" = ?3"
    );
    assert_eq!(params.len(), 3);
    let (sql, params) = build_delete(&SQLITE, &schema(), &Pred::not_null("project")).unwrap();
    assert_eq!(
        sql,
        "DELETE FROM \"e_7\" WHERE \"e_7\".\"project\" IS NOT NULL"
    );
    assert!(params.is_empty());
}

#[test]
fn sqlite_renders_defaults_as_its_own_literals() {
    assert_eq!(SQLITE.literal(&DataValue::Bool(true)).unwrap(), "1");
    assert_eq!(
        SQLITE.literal(&DataValue::Blob(vec![0, 255])).unwrap(),
        "X'00ff'"
    );
    assert_eq!(
        SQLITE.literal(&DataValue::Text("it's".into())).unwrap(),
        "'it''s'"
    );
    assert_eq!(SQLITE.literal(&DataValue::Real(1.5)).unwrap(), "1.5");
    assert_eq!(SQLITE.literal(&DataValue::Null).unwrap(), "NULL");
    assert_err!(
        SQLITE.literal(&DataValue::Real(f64::NAN)).unwrap_err(),
        DataError::InvalidSchema(_)
    );
}

#[test]
fn sqlite_full_text_ddl_names_every_column_three_times() {
    let ddl = SQLITE.full_text_create(&schema(), &["summary", "notes"]);
    assert!(ddl.contains("CREATE VIRTUAL TABLE \"e_7_fts\" USING fts5("));
    assert!(ddl.contains("content='e_7', content_rowid='id'"));
    assert!(ddl.contains("VALUES (new.id, new.\"summary\", new.\"notes\")"));
    assert!(ddl.contains("VALUES('delete', old.id, old.\"summary\", old.\"notes\")"));
    let drop = SQLITE.full_text_drop(&schema());
    assert!(drop.contains("DROP TRIGGER IF EXISTS \"e_7_fts_au\""));
    assert!(drop.contains("DROP TABLE IF EXISTS \"e_7_fts\""));
}
