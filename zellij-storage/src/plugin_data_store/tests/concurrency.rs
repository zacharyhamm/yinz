//! `rev`, the row revision the store keeps so that two writers can tell a row they read from a
//! row somebody else has since changed.
//!
//! Runs against both engines, which is the point: a compare-and-set that behaved differently on
//! SQLite and Postgres would be worse than none, because the case it exists for — several zellij
//! servers sharing one database — only arises on the engine the tests would have skipped.

use super::*;
use zellij_types::internal_ids::DataRevision;

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

// --------------------------------------------------------------- expect_rev

#[test]
fn a_guarded_write_at_the_current_revision_applies() {
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
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "mine")
                .expect_rev(DataRevision::new(seen))
                .build()],
        )
        .unwrap();
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "project"), vec!["mine"]);
    assert_eq!(column(&rows, "rev"), vec![DataValue::Int(seen + 1)]);
}

#[test]
fn a_guarded_write_at_a_stale_revision_conflicts_and_changes_nothing() {
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
    // Somebody else writes between the read and the write.
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "theirs")
                .build()],
        )
        .unwrap();

    let err = store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "mine")
                .expect_rev(DataRevision::new(seen))
                .build()],
        )
        .unwrap_err();
    assert_err!(err, DataError::Conflict(_));

    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "project"), vec!["theirs"]);
    assert_eq!(
        column(&rows, "rev"),
        vec![DataValue::Int(seen + 1)],
        "a refused write must not have advanced the revision"
    );
}

/// The whole reason a guarded put is an update rather than an upsert: resurrecting a row somebody
/// deleted after the read loses their delete exactly as an unguarded write loses their edit.
#[test]
fn a_guarded_write_against_a_deleted_row_conflicts_rather_than_resurrecting_it() {
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
    store
        .apply(NS, &[Mutation::delete("agent", Pred::eq("uid", "a1"))])
        .unwrap();

    let err = store
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "back-from-the-dead")
                .expect_rev(DataRevision::new(seen))
                .build()],
        )
        .unwrap_err();
    assert_err!(err, DataError::Conflict(_));
    assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 0);
}

#[test]
fn expect_absent_inserts_once_and_conflicts_after() {
    let (store, _dir) = defined();
    let claim = || {
        store.apply(
            NS,
            &[Mutation::put("agent")
                .uid("a1")
                .set("project", "zellij")
                .expect_absent()
                .build()],
        )
    };
    claim().unwrap();
    assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 1);
    assert_err!(claim().unwrap_err(), DataError::Conflict(_));
    assert_eq!(
        store.count(NS, &FetchRequest::entity("agent")).unwrap(),
        1,
        "the second claim must not have written a second row"
    );
}

#[test]
fn a_guarded_update_conflicts_when_the_revision_has_moved() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    let seen = only_rev(&store);
    let guarded = |rev: i64| {
        Mutation::update("agent", Pred::eq("project", "zellij"))
            .set("summary", "mine")
            .expect_rev(DataRevision::new(rev))
            .build()
    };
    store.apply(NS, &[guarded(seen)]).unwrap();
    // The revision has moved on, so the same guard no longer matches.
    let err = store.apply(NS, &[guarded(seen)]).unwrap_err();
    assert_err!(err, DataError::Conflict(_));
}

/// A guarded update still means whatever its filter meant: the revision narrows the match rather
/// than replacing it.
#[test]
fn a_guarded_update_still_honours_its_filter() {
    let (store, _dir) = defined();
    put(&store, "keep", "one", 1);
    put(&store, "change", "two", 2);
    store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "change"))
                .set("summary", "mine")
                .expect_rev(DataRevision::new(1))
                .build()],
        )
        .unwrap();
    let rows = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").sort_by("project", SortDir::Asc),
        )
        .unwrap();
    assert_eq!(texts(&rows, "summary"), vec!["mine", "one"]);
}

/// A batch is one transaction, so a conflict anywhere in it takes the whole batch with it. That
/// is what makes a read-modify-write over several rows safe to express as one apply.
#[test]
fn a_conflict_rolls_back_the_whole_batch() {
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
    let err = store
        .apply(
            NS,
            &[
                Mutation::put("agent")
                    .uid("a2")
                    .set("project", "written-first")
                    .build(),
                Mutation::put("agent")
                    .uid("a1")
                    .set("project", "conflicting")
                    .expect_rev(DataRevision::new(99))
                    .build(),
            ],
        )
        .unwrap_err();
    assert_err!(err, DataError::Conflict(_));
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(
        texts(&rows, "project"),
        vec!["zellij"],
        "the row written before the conflict should have been rolled back"
    );
}

/// An *unguarded* write that matches nothing is not a conflict — it is the answer to the question
/// it asked, and reporting it as a failure would break every caller that deletes optimistically.
#[test]
fn an_unguarded_write_that_matches_nothing_is_not_a_conflict() {
    let (store, _dir) = defined();
    let report = store
        .apply(
            NS,
            &[
                Mutation::update("agent", Pred::eq("project", "absent"))
                    .set("summary", "x")
                    .build(),
                Mutation::delete("agent", Pred::eq("project", "absent")),
            ],
        )
        .unwrap();
    assert_eq!(report.report.rows_written, 0);
    assert_eq!(report.report.rows_deleted, 0);
}

// ------------------------------------------------------------- per_mutation

/// The batch totals cannot say which mutation matched; this can.
#[test]
fn per_mutation_counts_line_up_with_the_mutations_given() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    put(&store, "zellij", "two", 2);
    put(&store, "other", "three", 3);

    let report = store
        .apply(
            NS,
            &[
                // two rows
                Mutation::update("agent", Pred::eq("project", "zellij"))
                    .set("summary", "changed")
                    .build(),
                // none
                Mutation::update("agent", Pred::eq("project", "absent"))
                    .set("summary", "changed")
                    .build(),
                // one
                Mutation::delete("agent", Pred::eq("project", "other")),
                // one, an insert
                Mutation::put("agent").set("project", "new").build(),
            ],
        )
        .unwrap()
        .report;
    assert_eq!(report.per_mutation, vec![2, 0, 1, 1]);
    // and the totals still add up the way they always did
    assert_eq!(report.rows_written, 3);
    assert_eq!(report.rows_deleted, 1);
}

/// The counts are what lets a caller guard by hand and still tell a miss from a match, without
/// asking for the batch to be rolled back.
#[test]
fn per_mutation_distinguishes_a_hand_written_guard_that_missed() {
    let (store, _dir) = defined();
    put(&store, "zellij", "one", 1);
    let report = store
        .apply(
            NS,
            &[Mutation::update(
                "agent",
                Pred::eq("project", "zellij").and(Pred::eq("rev", 99i64)),
            )
            .set("summary", "mine")
            .build()],
        )
        .unwrap()
        .report;
    assert_eq!(report.per_mutation, vec![0]);
}

// -------------------------------------------------------------- validation

/// Shapes that could only ever fail are refused where they are written, rather than conflicting
/// forever at runtime.
#[test]
fn guards_that_could_never_hold_are_rejected() {
    let (store, _dir) = defined();
    let refused = |mutation: Mutation| {
        let err = store.apply(NS, &[mutation]).unwrap_err();
        assert_err!(err, DataError::InvalidQuery(_))
    };
    // A put with no uid always inserts; there is no row for a revision to be about.
    refused(
        Mutation::put("agent")
            .set("project", "zellij")
            .expect_rev(DataRevision::new(1))
            .build(),
    );
    // `REV_ABSENT` on an update means "the row is not there", which an update cannot honour.
    refused(
        Mutation::update("agent", Pred::eq("project", "zellij"))
            .set("summary", "x")
            .expect_rev(REV_ABSENT)
            .build(),
    );
    // No row is ever at a negative revision.
    refused(
        Mutation::put("agent")
            .uid("a1")
            .set("project", "zellij")
            .expect_rev(DataRevision::new(-1))
            .build(),
    );
}
