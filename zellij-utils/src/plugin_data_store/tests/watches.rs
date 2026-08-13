use super::*;

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
