use super::*;

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
    assert_err!(err, DataError::InvalidQuery(_));
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
