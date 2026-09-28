use super::*;

#[test]
fn search_finds_a_row_by_its_text() {
    let (store, _dir) = defined();
    put(&store, "yinz", "fixing a rebase conflict", 1);
    put(&store, "yinz", "writing the changelog", 2);
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
    put(&store, "yinz", "a rebase conflict", 1);
    put(&store, "other", "a rebase conflict", 2);
    let rows = store
        .fetch(
            NS,
            &FetchRequest::entity("agent")
                .search("rebase")
                .filter(Pred::eq("project", "yinz")),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(texts(&rows, "project"), vec!["yinz"]);
}

#[test]
fn searching_an_entity_with_nothing_searchable_says_so() {
    let (store, _dir) = store();
    let def = EntityDef::new("plain", 1).attr(AttrDef::text("name"));
    store.define_entity(NS, OWNER, &def, false).unwrap();
    let err = store
        .fetch(NS, &FetchRequest::entity("plain").search("x"))
        .unwrap_err();
    assert_err!(err, DataError::InvalidQuery(_));
}
