use super::*;

#[test]
fn a_filter_selects_rows() {
    let (store, _dir) = defined();
    put(&store, "yinz", "one", 10);
    put(&store, "yinz", "two", 20);
    put(&store, "other", "three", 30);

    let rows = store
        .fetch(
            NS,
            &FetchRequest::entity("agent")
                .filter(Pred::eq("project", "yinz").and(Pred::gt("updated_at_ms", 15))),
        )
        .unwrap();
    assert_eq!(texts(&rows, "summary"), vec!["two"]);
}

#[test]
fn filtering_on_an_undeclared_attribute_is_an_error_rather_than_an_empty_result() {
    let (store, _dir) = defined();
    put(&store, "yinz", "one", 10);
    let err = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").filter(Pred::eq("projet", "yinz")),
        )
        .unwrap_err();
    assert_err!(err, DataError::InvalidQuery(_));
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
    assert_err!(err, DataError::InvalidQuery(_));
}

#[test]
fn a_prefix_filter_matches_the_start_and_contains_matches_anywhere() {
    let (store, _dir) = defined();
    put(&store, "yinz-fork", "x", 1);
    put(&store, "my-yinz", "y", 2);

    let prefixed = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").filter(Pred::prefix("project", "yinz")),
        )
        .unwrap();
    assert_eq!(texts(&prefixed, "project"), vec!["yinz-fork"]);

    let contained = store
        .fetch(
            NS,
            &FetchRequest::entity("agent").filter(Pred::contains("project", "yinz")),
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
    put(&store, "yinz", "one", 1);
    put(&store, "yinz", "two", 2);
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
    put(&store, "Yinz", "x", 1);
    let matched = |pred: Pred| {
        store
            .fetch(NS, &FetchRequest::entity("agent").filter(pred))
            .unwrap()
            .len()
    };
    assert_eq!(matched(Pred::prefix("project", "yin")), 1);
    assert_eq!(matched(Pred::contains("project", "INZ")), 1);
    assert_eq!(matched(Pred::eq("project", "yinz")), 0);
    assert_eq!(matched(Pred::eq("project", "Yinz")), 1);
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

/// The engines' defaults are opposite (SQLite puts nulls first ascending, Postgres last), so the
/// store says which it means. SQLite's is the one kept, and a plugin paging through a sorted set
/// sees the same order wherever its data lives.
#[test]
fn nulls_sort_first_ascending_and_last_descending() {
    let (store, _dir) = store();
    let def = EntityDef::new("row", 1).attr(AttrDef::int("n"));
    store.define_entity(NS, OWNER, &def, false).unwrap();
    store
        .apply(
            NS,
            &[
                Mutation::put("row").set("n", 2i64).build(),
                Mutation::put("row").set("n", DataValue::Null).build(),
                Mutation::put("row").set("n", 1i64).build(),
            ],
        )
        .unwrap();
    let ordered = |dir: SortDir| {
        column(
            &store
                .fetch(
                    NS,
                    &FetchRequest::entity("row").select(["n"]).sort_by("n", dir),
                )
                .unwrap(),
            "n",
        )
    };
    assert_eq!(
        ordered(SortDir::Asc),
        vec![DataValue::Null, DataValue::Int(1), DataValue::Int(2)]
    );
    assert_eq!(
        ordered(SortDir::Desc),
        vec![DataValue::Int(2), DataValue::Int(1), DataValue::Null]
    );
}
