use super::*;

#[test]
fn closed_tab_assignment_deletion_is_scoped_and_durable() {
    let (store, guard) = store();
    store
        .define_entity(
            NS,
            OWNER,
            &EntityDef::new("agent_assignment", 1)
                .attr(AttrDef::text("host_id").required())
                .attr(AttrDef::text("yinz_session_name").required())
                .attr(AttrDef::int("tab_id").required()),
            false,
        )
        .unwrap();
    let owners = [
        ("local", "session", 1_i64),
        ("local", "session", 2),
        ("other", "session", 1),
        ("local", "other", 1),
        ("", "session", 1),
    ];
    let mutations: Vec<_> = owners
        .into_iter()
        .map(|(host, session, tab)| {
            Mutation::put("agent_assignment")
                .set("host_id", host)
                .set("yinz_session_name", session)
                .set("tab_id", tab)
                .build()
        })
        .collect();
    store.apply(NS, &mutations).unwrap();
    store
        .apply(
            NS,
            &[Mutation::delete(
                "agent_assignment",
                Pred::eq("host_id", "local")
                    .and(Pred::eq("yinz_session_name", "session"))
                    .and(Pred::is_in("tab_id", [1_i64])),
            )],
        )
        .unwrap();
    drop(store);
    let store = reopen(&guard);
    let rows = store
        .fetch(
            NS,
            &FetchRequest::entity("agent_assignment").select([
                "host_id",
                "yinz_session_name",
                "tab_id",
            ]),
        )
        .unwrap();
    let expected: Vec<Vec<DataValue>> = owners[1..]
        .iter()
        .map(|(host, session, tab)| vec![(*host).into(), (*session).into(), (*tab).into()])
        .collect();
    assert_eq!(rows.rows, expected);
}

#[test]
fn a_put_without_a_uid_always_inserts() {
    let (store, _dir) = defined();
    put(&store, "yinz", "one", 1);
    put(&store, "yinz", "two", 2);
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
                .set("project", "yinz")
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
                .set("project", "yinz-fork")
                .set("updated_at_ms", 2)
                .build()],
        )
        .unwrap();
    // the same row, not a second one
    assert_eq!(first.report.ids, second.report.ids);
    assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 1);

    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "project"), vec!["yinz-fork"]);
    // when the row was first written is still true after an update
    assert_eq!(column(&rows, "created_at")[0], created);
    assert_ne!(column(&rows, "updated_at")[0], created);
}

#[test]
fn a_batch_is_all_or_nothing() {
    let (store, _dir) = defined();
    put(&store, "yinz", "keep", 1);
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
    assert_err!(err, DataError::InvalidQuery(_));
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    // only the row from before the batch: the valid first mutation rolled back with the second
    assert_eq!(texts(&rows, "summary"), vec!["keep"]);
    assert_eq!(texts(&rows, "project"), vec!["yinz"]);
}

#[test]
fn a_batch_with_a_nul_uid_leaves_stored_data_unchanged() {
    let (store, _dir) = defined();
    put(&store, "yinz", "keep", 1);
    let request = FetchRequest::entity("agent");
    let before = store.fetch(NS, &request).unwrap();
    let err = store
        .apply(
            NS,
            &[
                Mutation::update("agent", Pred::eq("project", "yinz"))
                    .set("summary", "changed")
                    .build(),
                Mutation::put("agent")
                    .uid("bad\0uid")
                    .set("project", "new")
                    .build(),
            ],
        )
        .unwrap_err();
    assert_err!(err, DataError::InvalidQuery(_));
    assert_eq!(store.fetch(NS, &request).unwrap(), before);
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
    assert_err!(err, DataError::InvalidQuery(_));
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
    assert_err!(err, DataError::InvalidQuery(_));
}

#[test]
fn update_and_delete_report_how_many_rows_they_touched() {
    let (store, _dir) = defined();
    put(&store, "yinz", "one", 1);
    put(&store, "yinz", "two", 2);
    put(&store, "other", "three", 3);

    let updated = store
        .apply(
            NS,
            &[Mutation::update("agent", Pred::eq("project", "yinz"))
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
                Mutation::put("agent").set("project", "yinz").build(),
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
    assert_err!(err, DataError::Storage(_));
}

/// `DataValue::fits` lets an integer into a Real attribute, so that a plugin writing `0` to a
/// float column need not know to write `0.0`. SQLite column affinity returns a Real.
#[test]
fn an_int_written_to_a_real_attribute_reads_back_as_a_real() {
    let (store, _dir) = store();
    let def = EntityDef::new("m", 1).attr(AttrDef::real("score").default(0i64));
    store.define_entity(NS, OWNER, &def, false).unwrap();
    store
        .apply(
            NS,
            &[
                Mutation::put("m").set("score", 3i64).build(),
                Mutation::put("m").set("score", DataValue::Null).build(),
            ],
        )
        .unwrap();
    let rows = store
        .fetch(NS, &FetchRequest::entity("m").select(["score"]))
        .unwrap();
    assert_eq!(
        column(&rows, "score"),
        vec![DataValue::Real(3.0), DataValue::Null]
    );
    let found = store
        .fetch(
            NS,
            &FetchRequest::entity("m").filter(Pred::gte("score", 3i64)),
        )
        .unwrap();
    assert_eq!(found.len(), 1);
}
