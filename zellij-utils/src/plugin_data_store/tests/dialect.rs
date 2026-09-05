//! The SQL each dialect renders, checked without a database.
//!
//! Everything the two engines are made to agree on is decided at composition time; these tests
//! pin the text so that a change to one dialect is a visible diff rather than a red run of the
//! docker-backed suite later.

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
        .filter(Pred::eq("project", "zellij").and(Pred::is_in("score", [1i64, 2])))
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
            DataValue::Text("zellij".into()),
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
    let values = vec![("project".to_owned(), DataValue::Text("zellij".into()))];
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
            DataValue::Text("zellij".into()),
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

// ------------------------------------------------------------------------------------ Postgres

use crate::plugin_data_store::postgres::dialect::POSTGRES;

#[test]
fn index_names_respect_each_engines_limit_without_losing_the_suffix() {
    for length in [1, 62, 63, 64, 90] {
        let name = "a".repeat(length);
        assert_eq!(SQLITE.index_name(&name), name);
        let normalized = POSTGRES.index_name(&name);
        assert_eq!(POSTGRES.index_name(&name), normalized);
        if length <= 63 {
            assert_eq!(normalized, name);
        } else {
            assert_eq!(normalized.len(), 63);
            assert_eq!(&normalized[..31], format!("{}_", "a".repeat(30)));
            assert!(normalized[31..].bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
    let prefix = "a".repeat(62);
    let schema = schema();
    let names = [
        schema.attr_index(&format!("{prefix}b")),
        schema.attr_index(&format!("{prefix}c")),
        schema.unique_index(&format!("{prefix}b")),
        schema.unique_index(&format!("{prefix}c")),
    ];
    let normalized: BTreeSet<_> = names.iter().map(|name| POSTGRES.index_name(name)).collect();
    assert_eq!(normalized.len(), names.len());
    for name in names {
        assert_eq!(SQLITE.index_name(&name), name);
    }
}

#[test]
fn postgres_numbers_its_placeholders_with_dollars() {
    let request = FetchRequest::entity("agent")
        .filter(Pred::eq("project", "zellij").and(Pred::is_in("score", [1i64, 2])))
        .select(["project"])
        .limit(10)
        .offset(3);
    let fetch = build_fetch_sql(&POSTGRES, &schema(), &request, None).unwrap();
    assert_eq!(
        fetch.sql,
        "SELECT \"e_7\".\"id\", \"e_7\".\"project\" FROM \"e_7\" \
         WHERE (\"e_7\".\"project\" = $1 AND \"e_7\".\"score\" IN ($2, $3)) \
         ORDER BY \"e_7\".\"id\" ASC LIMIT $4 OFFSET $5"
    );
    assert_eq!(fetch.params.len(), 5);
}

#[test]
fn postgres_matches_patterns_with_ilike_and_the_default_escape() {
    let request = FetchRequest::entity("agent").filter(Pred::contains("project", "a_b"));
    let fetch = build_fetch_sql(&POSTGRES, &schema(), &request, None).unwrap();
    assert!(
        fetch.sql.contains("WHERE \"e_7\".\"project\" ILIKE $1 "),
        "{}",
        fetch.sql
    );
    assert_eq!(fetch.params[0], DataValue::Text("%a\\_b%".into()));
}

/// The search term is bound once and named twice: in the condition and again in the ranking.
#[test]
fn postgres_searches_its_generated_tsvector_column() {
    let request = FetchRequest::entity("agent").search("rebase");
    let (sql, params) = build_count_sql(&POSTGRES, &schema(), &request, Some("rebase")).unwrap();
    assert_eq!(
        sql,
        "SELECT COUNT(*) FROM \"e_7\" \
         WHERE \"e_7\".\"_fts\" @@ websearch_to_tsquery('simple', $1)"
    );
    assert_eq!(params, vec![DataValue::Text("rebase".into())]);
    let fetch = build_fetch_sql(&POSTGRES, &schema(), &request, Some("rebase")).unwrap();
    assert!(
        fetch.sql.contains(
            "ORDER BY ts_rank_cd(\"e_7\".\"_fts\", websearch_to_tsquery('simple', $1)) DESC, \
             \"e_7\".\"id\" ASC LIMIT $2 OFFSET $3"
        ),
        "{}",
        fetch.sql
    );
    assert_eq!(fetch.params.len(), 3);
}

#[test]
fn postgres_declares_typed_collated_columns() {
    let columns = POSTGRES.store_columns();
    assert_eq!(
        columns[0],
        "id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY"
    );
    assert_eq!(columns[1], "uid TEXT COLLATE \"C\" UNIQUE");
    assert_eq!(POSTGRES.column_type(AttrKind::Text), "TEXT COLLATE \"C\"");
    assert_eq!(POSTGRES.column_type(AttrKind::Json), "TEXT COLLATE \"C\"");
    assert_eq!(POSTGRES.column_type(AttrKind::Int), "BIGINT");
    assert_eq!(POSTGRES.column_type(AttrKind::Real), "DOUBLE PRECISION");
    assert_eq!(POSTGRES.column_type(AttrKind::Bool), "BOOLEAN");
    assert_eq!(POSTGRES.column_type(AttrKind::Blob), "BYTEA");
}

#[test]
fn postgres_renders_defaults_as_its_own_literals() {
    assert_eq!(POSTGRES.literal(&DataValue::Bool(true)).unwrap(), "TRUE");
    assert_eq!(POSTGRES.literal(&DataValue::Bool(false)).unwrap(), "FALSE");
    assert_eq!(
        POSTGRES.literal(&DataValue::Blob(vec![0, 255])).unwrap(),
        "'\\x00ff'::bytea"
    );
    // The rest is shared with SQLite.
    assert_eq!(
        POSTGRES.literal(&DataValue::Text("it's".into())).unwrap(),
        "'it''s'"
    );
    assert_eq!(POSTGRES.literal(&DataValue::Int(-3)).unwrap(), "-3");
}

#[test]
fn postgres_full_text_is_a_generated_column_with_a_gin_index() {
    let ddl = POSTGRES.full_text_create(&schema(), &["summary", "notes"]);
    assert_eq!(
        ddl,
        "ALTER TABLE \"e_7\" ADD COLUMN \"_fts\" tsvector GENERATED ALWAYS AS \
         (to_tsvector('simple', coalesce(\"summary\", '') || ' ' || coalesce(\"notes\", ''))) \
         STORED;
             CREATE INDEX \"e_7_fts\" ON \"e_7\" USING GIN (\"_fts\");"
    );
    assert_eq!(
        POSTGRES.full_text_drop(&schema()),
        "ALTER TABLE \"e_7\" DROP COLUMN IF EXISTS \"_fts\";"
    );
    assert!(!POSTGRES.retries_search_as_phrase());
    assert!(SQLITE.retries_search_as_phrase());
}
