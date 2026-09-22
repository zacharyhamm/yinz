use super::*;
use crate::data_access::PluginDataBackend;
use std::sync::{Arc, Mutex};

struct MemoryBackend(Mutex<bool>);

impl PluginDataBackend for MemoryBackend {
    fn define_entity(
        &self,
        _: &str,
        _: &str,
        _: &EntityDef,
        _: bool,
    ) -> Result<DataSchemaInfo, DataError> {
        Ok(DataSchemaInfo::default())
    }

    fn catalog(&self, _: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
        Ok(Vec::new())
    }

    fn fetch(&self, _: &str, _: &FetchRequest) -> Result<DataRows, DataError> {
        let present = *self.0.lock().unwrap();
        Ok(DataRows {
            columns: vec!["id".to_owned()],
            rows: present
                .then(|| vec![DataValue::Int(1)])
                .into_iter()
                .collect(),
            truncated: false,
            next_cursor: None,
        })
    }

    fn count(&self, _: &str, _: &FetchRequest) -> Result<u64, DataError> {
        Ok(*self.0.lock().unwrap() as u64)
    }

    fn apply(&self, _: &str, _: &[Mutation]) -> Result<ApplyReport, DataError> {
        *self.0.lock().unwrap() = true;
        Ok(ApplyReport::default())
    }
}

#[test]
fn watches_work_with_a_backend_that_has_no_sqlite_types() {
    let store = PluginDataStore::from_backend(Arc::new(MemoryBackend(Mutex::new(false))));
    let handle = store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("thing"),
            WatchMode::Rows,
        )
        .unwrap();
    assert!(handle.rows.is_empty());

    let outcome = store
        .apply(NS, &[Mutation::put("thing").set("name", "new").build()])
        .unwrap();
    let changes = store
        .refresh_watches(NS, &outcome.touched_entities)
        .unwrap();
    assert_eq!(changes[0].rows.rows, vec![vec![DataValue::Int(1)]]);
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
fn blob_contents_cannot_hide_a_watch_change_as_a_field_boundary() {
    let (store, _dir) = store();
    let def = EntityDef::new("binary", 1)
        .attr(AttrDef::blob("left"))
        .attr(AttrDef::blob("right"));
    store.define_entity(NS, OWNER, &def, false).unwrap();
    let put = |left, right| {
        store
            .apply(
                NS,
                &[Mutation::put("binary")
                    .uid("one")
                    .set("left", DataValue::Blob(left))
                    .set("right", DataValue::Blob(right))
                    .build()],
            )
            .unwrap();
    };
    put(vec![1], vec![2, 0xfe, 5, 3]);
    let handle = store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("binary").select(["left", "right"]),
            WatchMode::Rows,
        )
        .unwrap();
    // These rows used to hash the same bytes: the separator and Blob tag also occur in data.
    put(vec![1, 0xfe, 5, 2], vec![3]);
    let changes = store.refresh_watches(NS, &touched("binary")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].watch_id, handle.watch_id);
    assert_eq!(
        changes[0].rows.rows,
        vec![vec![
            DataValue::Blob(vec![1, 0xfe, 5, 2]),
            DataValue::Blob(vec![3])
        ]]
    );
    assert!(store
        .refresh_watches(NS, &touched("binary"))
        .unwrap()
        .is_empty());
}

#[test]
fn replacing_identical_rows_notifies_a_watch_when_its_page_cursor_changes() {
    let (store, _dir) = defined();
    for _ in 0..3 {
        put(&store, "zellij", "same", 1);
    }
    let handle = store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("agent").select(["summary"]).limit(1),
            WatchMode::Rows,
        )
        .unwrap();
    let cursor = handle.rows.next_cursor.unwrap();
    store
        .apply(NS, &[Mutation::delete("agent", Pred::eq("id", cursor))])
        .unwrap();
    let changes = store.refresh_watches(NS, &touched("agent")).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].rows.rows, handle.rows.rows);
    assert_ne!(changes[0].rows.next_cursor, Some(cursor));
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
    assert!(store
        .unregister_watch(PluginId::new(PLUGIN.get() + 1), handle.watch_id)
        .is_err());
    assert_eq!(store.watch_count(), 1);
}

#[test]
fn unloading_a_plugin_drops_its_watches_and_leaves_the_others() {
    let (store, _dir) = defined();
    for plugin_id in [PLUGIN, PluginId::new(PLUGIN.get() + 1)] {
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
    assert_eq!(changes[0].plugin_id, PluginId::new(PLUGIN.get() + 1));
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
