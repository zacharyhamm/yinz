use super::*;
use yinz_types::{
    internal_ids::{ClientId, DataRevision, PluginId},
    plugin_data::{AttrDef, DataRows, EntityDef, FetchRequest, Pred, WatchMode},
};

#[test]
fn remote_data_changes_use_the_existing_plugin_event_route_once() {
    use crate::yinz_exports::{notify_data_watchers, poll_data_watchers};
    use yinz_bus::{plugins::PluginInstruction, thread_bus::ThreadSenders};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::data::Event;

    let dir = tempfile::tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    let other = PluginDataStore::new(dir.path());
    let namespace = "shared:queue";
    store
        .define_entity(
            namespace,
            "queue",
            &EntityDef::new("item", 1).attr(AttrDef::text("text")),
            false,
        )
        .unwrap();
    let plugin = PluginId::new(4);
    let client = ClientId::new(1);
    let query = FetchRequest::entity("item");
    let watch = store
        .register_watch(namespace, plugin, client, query.clone(), WatchMode::Rows)
        .unwrap();
    let (send, receive) = unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(send)),
        ..Default::default()
    };
    let generations = Default::default();
    other
        .apply(
            namespace,
            &[Mutation::put("item").set("text", "queued").build()],
        )
        .unwrap();
    poll_data_watchers(&store, &generations, &senders);
    let (instruction, _) = receive.try_recv().unwrap();
    let PluginInstruction::Update(events) = instruction else {
        panic!("wrong event route")
    };
    assert_eq!(
        events,
        vec![(
            Some(plugin),
            Some(client),
            Event::PluginDataChanged {
                watch_id: watch.watch_id,
                truncated: false,
                rows: store.fetch(namespace, &query).unwrap(),
            }
        )]
    );
    notify_data_watchers(
        &store,
        &generations,
        &senders,
        namespace,
        &BTreeSet::from(["item".into()]),
    );
    poll_data_watchers(&store, &generations, &senders);
    assert!(receive.try_recv().is_err());
}

#[test]
fn cli_admin_pages_private_data_and_delivers_only_committed_atomic_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    let namespace = "plugin:file:/fixture.ts";
    store
        .define_entity(
            namespace,
            "fixture",
            &EntityDef::new("item", 1).attr(AttrDef::text("text").required()),
            false,
        )
        .unwrap();
    let query = FetchRequest::entity("item");
    let watch = store
        .register_watch(
            namespace,
            PluginId::new(4),
            ClientId::new(1),
            query.clone(),
            WatchMode::Rows,
        )
        .unwrap();
    assert!(watch.rows.rows.is_empty());
    let mut deliveries = vec![];
    let apply = |mutations| DataRequest::Apply {
        namespace: namespace.into(),
        mutations,
    };
    run(
        &store,
        apply(vec![
            Mutation::put("item")
                .uid("one")
                .set("text", "first")
                .expect_rev(DataRevision::new(0))
                .build(),
            Mutation::put("item")
                .uid("two")
                .set("text", "second")
                .expect_rev(DataRevision::new(0))
                .build(),
        ]),
        |ns, entities| deliveries.extend(store.refresh_watches(ns, entities).unwrap()),
    )
    .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].watch_id, watch.watch_id);
    assert_eq!(deliveries[0].rows.rows.len(), 2);
    let fetch = |request| {
        serde_json::from_str::<DataRows>(
            &run(&store, DataRequest::Fetch(request), |_, _| {
                panic!("read notified watchers")
            })
            .unwrap(),
        )
        .unwrap()
    };
    let page = fetch(query.clone().in_namespace(namespace).limit(1));
    assert!(page.truncated);
    let next = fetch(
        query
            .clone()
            .in_namespace(namespace)
            .limit(1)
            .after_id(page.next_cursor.unwrap()),
    );
    assert_eq!(next.rows.len(), 1);
    assert_ne!(next.rows, page.rows);
    let count = || {
        run(
            &store,
            DataRequest::Count(query.clone().in_namespace(namespace)),
            |_, _| panic!(),
        )
        .unwrap()
    };
    assert_eq!(count(), "2");
    let error = run(
        &store,
        apply(vec![
            Mutation::put("item").set("text", "rolled back").build(),
            Mutation::put("item")
                .uid("one")
                .set("text", "stale")
                .expect_rev(DataRevision::new(0))
                .build(),
        ]),
        |_, _| panic!("failed batch notified watchers"),
    );
    assert!(error.is_err());
    assert_eq!(count(), "2");
    assert!(run(
        &store,
        apply(vec![Mutation::put("item")
            .uid("one")
            .set("text", "unguarded")
            .build()]),
        |_, _| panic!()
    )
    .is_err());
    let filter = Pred::eq("uid", "one");
    assert!(run(
        &store,
        apply(vec![Mutation::Delete {
            entity: "item".into(),
            filter: filter.clone()
        }]),
        |_, _| panic!()
    )
    .is_err());
    let rows = store
        .fetch(
            namespace,
            &query.clone().filter(filter.clone()).select(["id", "rev"]),
        )
        .unwrap();
    let id_rev = match rows.rows[0].as_slice() {
        [yinz_types::plugin_data::DataValue::Int(id), yinz_types::plugin_data::DataValue::Int(rev)] => {
            (
                yinz_types::internal_ids::DataRowId::new(*id),
                DataRevision::new(*rev),
            )
        },
        _ => panic!("id and revision"),
    };
    run(
        &store,
        apply(vec![
            Mutation::AssertRows {
                entity: "item".into(),
                filter: filter.clone(),
                rows: vec![id_rev],
            },
            Mutation::Delete {
                entity: "item".into(),
                filter,
            },
        ]),
        |ns, entities| {
            assert_eq!(store.refresh_watches(ns, entities).unwrap().len(), 1);
        },
    )
    .unwrap();
    assert_eq!(count(), "1");
    assert!(run(&store, DataRequest::Fetch(query.clone()), |_, _| panic!()).is_err());
    assert!(run(
        &store,
        DataRequest::Fetch(query.in_namespace("missing")),
        |_, _| panic!()
    )
    .is_err());
}
