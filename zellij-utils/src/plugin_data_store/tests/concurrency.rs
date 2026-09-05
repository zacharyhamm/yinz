//! `rev`, the row revision the store keeps so that two writers can tell a row they read from a
//! row somebody else has since changed.
//!
//! Runs against both engines, which is the point: a compare-and-set that behaved differently on
//! SQLite and Postgres would be worse than none, because the case it exists for — several zellij
//! servers sharing one database — only arises on the engine the tests would have skipped.

use super::*;

/// The `rev` of the single row an `agent` fetch returns.
fn only_rev(store: &PluginDataStore) -> i64 {
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(rows.rows.len(), 1, "expected exactly one row");
    match column(&rows, "rev")[0] {
        DataValue::Int(rev) => rev,
        ref other => panic!("`rev` came back as {:?}, not an integer", other),
    }
}

#[test]
fn a_new_row_starts_at_revision_one() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    assert_eq!(only_rev(&store), 1);
}

#[test]
fn an_upsert_advances_the_revision_rather_than_resetting_it() {
    let (store, _dir) = defined();
    let write = |project: &str| {
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("a1")
                    .set("project", project)
                    .build()],
            )
            .unwrap();
    };
    write("one");
    assert_eq!(only_rev(&store), 1);
    // The incoming row carries the column default, so an upsert that read `excluded."rev"` would
    // put every write back at 1. This is that regression.
    write("two");
    assert_eq!(only_rev(&store), 2);
    write("three");
    assert_eq!(only_rev(&store), 3);
}

#[test]
fn an_update_advances_the_revision() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    for expected in 2..=4 {
        store
            .apply(
                NS,
                &[Mutation::update("agent", Pred::eq("project", "zellij"))
                    .set("summary", "changed")
                    .build()],
            )
            .unwrap();
        assert_eq!(only_rev(&store), expected);
    }
}

/// An update that matches nothing leaves every revision alone — which is what makes `rev` usable
/// as a guard rather than merely as a change counter.
#[test]
fn an_update_that_matches_nothing_advances_no_revision() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    let before = only_rev(&store);
    let report = store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "absent"))
                .set("summary", "changed")
                .build()],
        )
        .unwrap();
    assert_eq!(report.report.rows_written, 0);
    assert_eq!(only_rev(&store), before);
}

/// Rows keep their own counters. A busy row must not age a quiet one.
#[test]
fn revisions_are_per_row() {
    let (store, _dir) = defined();
    for uid in ["a1", "a2"] {
        store
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid(uid)
                    .set("project", "zellij")
                    .build()],
            )
            .unwrap();
    }
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "moved")
                .build()],
        )
        .unwrap();
    let rows = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").sort_by("uid", SortDir::Asc),
        )
        .unwrap();
    assert_eq!(
        column(&rows, "rev"),
        vec![DataValue::Int(2), DataValue::Int(1)]
    );
}

/// `rev` is a store column, so it is selectable, filterable and sortable exactly as `created_at`
/// is. This is the whole mechanism a caller needs to write a compare-and-set by hand, and the
/// next commit's `expect_rev` is sugar over it.
#[test]
fn rev_can_be_selected_filtered_and_sorted_on() {
    let (store, _dir) = defined();
    put(&store, "quiet", "one", 1);
    store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "quiet"))
                .set("project", "busy")
                .build()],
        )
        .unwrap();
    put(&store, "quiet", "two", 2);

    let selected = store
        .fetch(NS, &FetchRequest::entity("agent").select(["rev"]))
        .unwrap();
    assert_eq!(selected.columns, vec!["rev".to_owned()]);

    let filtered = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").filter(Pred::gt("rev", 1i64)),
        )
        .unwrap();
    assert_eq!(texts(&filtered, "project"), vec!["busy"]);

    let sorted = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").sort_by("rev", SortDir::Desc),
        )
        .unwrap();
    assert_eq!(texts(&sorted, "project"), vec!["busy", "quiet"]);
}

/// A guarded update spelled by hand: the whole point of the column, and the shape the hocus
/// read-modify-write paths take.
#[test]
fn an_update_guarded_on_a_stale_rev_matches_nothing() {
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
    let seen = only_rev(&store);

    // Somebody else writes first.
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "theirs")
                .build()],
        )
        .unwrap();

    let guarded = |rev: i64| {
        Mutation::update("agent", Pred::eq("uid", "a1").and(Pred::eq("rev", rev)))
            .set("project", "mine")
            .build()
    };
    let stale = store.apply(NS, &[guarded(seen)]).unwrap();
    assert_eq!(
        stale.report.rows_written, 0,
        "a write guarded on a revision that has moved should match nothing"
    );
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "project"), vec!["theirs"]);

    // Re-read, and the same write lands.
    let fresh = store.apply(NS, &[guarded(only_rev(&store))]).unwrap();
    assert_eq!(fresh.report.rows_written, 1);
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "project"), vec!["mine"]);
}

/// A plugin may not declare an attribute called `rev`, for the same reason it may not declare one
/// called `updated_at`: the store maintains it.
#[test]
fn rev_is_reserved_against_declaration_and_direct_writes() {
    let (store, _dir) = store();
    let clashing = EntityDef::new("agent", 1).attr(AttrDef::int("rev"));
    assert_err!(
        store
            .define_entity(NS, OWNER, &clashing, false)
            .unwrap_err(),
        DataError::InvalidSchema(_) | DataError::InvalidName(_)
    );

    // Refused as a name before the mutation is ever compiled, which is why this is `InvalidName`
    // rather than the `InvalidQuery` an undeclared-but-legal attribute would get.
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    let err = store
        .apply(NS, &[Mutation::put("agent").set("rev", 99i64).build()])
        .unwrap_err();
    assert_err!(err, DataError::InvalidName(_) | DataError::InvalidQuery(_));
}
