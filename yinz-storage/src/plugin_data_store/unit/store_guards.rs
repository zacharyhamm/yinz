use super::*;
use yinz_types::internal_ids::DataRevision;

#[test]
fn a_guarded_multirow_update_rolls_back_if_any_matching_revision_moved() {
    let (store, guard) = defined();
    put(&store, "selected", "first", 1);
    put(&store, "selected", "second", 2);
    let other = reopen(&guard);
    other
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("summary", "second"))
                .set("summary", "concurrent")
                .build()],
        )
        .unwrap();
    let before = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    let error = store
        .apply(
            NS,
            &[
                Mutation::put("agent").set("project", "earlier").build(),
                Mutation::update("agent", Pred::eq("project", "selected"))
                    .set("project", "replacement")
                    .set("summary", "replacement")
                    .expect_rev(DataRevision::new(1))
                    .build(),
                Mutation::put("agent").set("project", "later").build(),
            ],
        )
        .unwrap_err();
    assert_err!(error, DataError::Conflict(_));
    assert_eq!(
        store.fetch(NS, &FetchRequest::entity("agent")).unwrap(),
        before
    );
    assert_eq!(
        texts(
            &store
                .fetch(NS, &FetchRequest::entity("agent").search("concurrent"))
                .unwrap(),
            "summary"
        ),
        ["concurrent"]
    );
    assert!(store
        .fetch(NS, &FetchRequest::entity("agent").search("replacement"))
        .unwrap()
        .is_empty());
}

#[test]
fn a_guarded_multirow_update_changes_every_selected_row_and_ignores_other_revisions() {
    let (store, _guard) = defined();
    put(&store, "selected", "first", 1);
    put(&store, "selected", "second", 2);
    put(&store, "excluded", "third", 3);
    store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "excluded"))
                .set("summary", "concurrent")
                .build()],
        )
        .unwrap();
    let report = store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "selected"))
                .set("project", "replacement")
                .expect_rev(DataRevision::new(1))
                .build()],
        )
        .unwrap();
    assert_eq!(report.report.rows_written, 2);
    assert_eq!(report.report.per_mutation, [2]);
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(
        texts(&rows, "project"),
        ["replacement", "replacement", "excluded"]
    );
    assert_eq!(texts(&rows, "summary"), ["first", "second", "concurrent"]);
    assert_eq!(column(&rows, "rev"), vec![DataValue::Int(2); 3]);
}

#[test]
fn guarded_updates_reject_empty_matches_and_empty_values() {
    let (store, _guard) = defined();
    put(&store, "kept", "original", 1);
    let before = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    let error = store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "absent"))
                .set("summary", "replacement")
                .expect_rev(DataRevision::new(1))
                .build()],
        )
        .unwrap_err();
    assert_err!(error, DataError::Conflict(_));
    let error = store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "kept"))
                .expect_rev(DataRevision::new(1))
                .build()],
        )
        .unwrap_err();
    assert_err!(error, DataError::InvalidQuery(_));
    assert_eq!(
        store.fetch(NS, &FetchRequest::entity("agent")).unwrap(),
        before
    );
}

#[test]
fn corrupt_store_versions_are_rejected_without_replaying_migrations() {
    for invalid in ["not-a-version", "", "-1", "18446744073709551616"] {
        let (store, guard) = defined();
        put(&store, "kept", "original", 1);
        store
            .apply(
                NS,
                &[Mutation::update("agent", Pred::eq("project", "kept"))
                    .set("summary", "preserved")
                    .build()],
            )
            .unwrap();
        let before = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
        let catalog = store.catalog(None).unwrap();
        let version = store_version(&guard);
        drop(store);

        let conn = rusqlite::Connection::open(guard.path().join("plugin-data.db")).unwrap();
        conn.execute(
            "UPDATE store_meta SET value = ?1 WHERE key = 'schema_version'",
            [invalid],
        )
        .unwrap();
        let reopened = reopen(&guard);
        let error = reopened
            .fetch(NS, &FetchRequest::entity("agent"))
            .unwrap_err();
        assert!(
            matches!(&error, DataError::Storage(message) if message.contains("invalid plugin data store schema_version")),
            "{invalid:?}: {error:?}"
        );
        let recorded: String = conn
            .query_row(
                "SELECT value FROM store_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, invalid);
        set_store_version(&guard, version);
        assert_eq!(
            reopened.fetch(NS, &FetchRequest::entity("agent")).unwrap(),
            before
        );
        assert_eq!(reopened.catalog(None).unwrap(), catalog);
    }
}
