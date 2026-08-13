use super::*;
// The flat module's own `use` block used to reach the tests through `use super::*`. Now that the
// engine is split up, the types plugins speak in are imported here directly.
use crate::plugin_data::*;
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
