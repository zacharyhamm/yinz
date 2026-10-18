use super::*;
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[test]
fn remote_commit_reaches_the_polling_worker_with_the_same_payload_as_fetch() {
    let (store, dir) = defined();
    let store = Arc::new(store);
    let other = reopen(&dir);
    let request = FetchRequest::entity("agent");
    let handle = store
        .register_watch(NS, PLUGIN, CLIENT, request.clone(), WatchMode::Rows)
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _poller = store
        .start_watch_polling(move |store| {
            for change in store.poll_external_watches().unwrap() {
                send.send(change).unwrap();
            }
        })
        .unwrap();
    put(&other, "yinz", "remote", 1);
    // One 250 ms tick plus scheduler/query headroom on loaded test machines.
    let change = receive
        .recv_timeout(DATA_WATCH_POLL_INTERVAL + Duration::from_secs(2))
        .unwrap();
    assert_eq!(change.watch_id, handle.watch_id);
    assert_eq!(change.plugin_id, PLUGIN);
    assert_eq!(change.client_id, CLIENT);
    assert_eq!(change.rows, store.fetch(NS, &request).unwrap());
    assert_eq!(texts(&change.rows, "summary"), ["remote"]);
}

#[test]
fn local_and_remote_refreshes_share_deduplication_in_either_order() {
    let (store, dir) = defined();
    let other = reopen(&dir);
    store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("agent"),
            WatchMode::Rows,
        )
        .unwrap();
    put(&store, "yinz", "local", 1);
    assert_eq!(
        store.refresh_watches(NS, &touched("agent")).unwrap().len(),
        1
    );
    assert!(store.poll_external_watches().unwrap().is_empty());
    // Even a remote commit to an unrelated entity cannot duplicate the local result.
    other
        .define_entity(
            NS,
            OWNER,
            &EntityDef::new("other", 1).attr(AttrDef::text("text")),
            false,
        )
        .unwrap();
    assert!(store.poll_external_watches().unwrap().is_empty());
    put(&other, "yinz", "remote", 2);
    put(&store, "yinz", "local again", 3);
    assert_eq!(store.poll_external_watches().unwrap().len(), 1);
    assert!(store
        .refresh_watches(NS, &touched("agent"))
        .unwrap()
        .is_empty());
    assert!(store.poll_external_watches().unwrap().is_empty());
}

#[test]
fn remote_writes_respect_namespace_entity_record_query_and_ids_filters() {
    let (store, dir) = defined();
    let other = reopen(&dir);
    other
        .define_entity("shared:elsewhere", OWNER, &agent_def(), false)
        .unwrap();
    other
        .define_entity(
            NS,
            OWNER,
            &EntityDef::new("other", 1).attr(AttrDef::text("text")),
            false,
        )
        .unwrap();
    let requests = [
        (NS, FetchRequest::entity("agent"), WatchMode::Rows),
        (
            NS,
            FetchRequest::entity("agent").filter(Pred::eq("uid", "chosen")),
            WatchMode::Rows,
        ),
        (
            NS,
            FetchRequest::entity("agent").filter(Pred::eq("project", "yinz")),
            WatchMode::Rows,
        ),
        (
            NS,
            FetchRequest::entity("agent").filter(Pred::eq("project", "yinz")),
            WatchMode::Ids,
        ),
        (NS, FetchRequest::entity("other"), WatchMode::Rows),
        (
            "shared:elsewhere",
            FetchRequest::entity("agent"),
            WatchMode::Rows,
        ),
    ];
    let handles: Vec<_> = requests
        .iter()
        .map(|(ns, request, mode)| {
            store
                .register_watch(ns, PLUGIN, CLIENT, request.clone(), *mode)
                .unwrap()
        })
        .collect();
    other
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("ignored")
                .set("project", "elsewhere")
                .build()],
        )
        .unwrap();
    let changes = store.poll_external_watches().unwrap();
    assert_eq!(
        changes.iter().map(|c| c.watch_id).collect::<Vec<_>>(),
        [handles[0].watch_id]
    );
    other
        .apply(
            NS,
            &[Mutation::put("agent")
                .uid("chosen")
                .set("project", "yinz")
                .build()],
        )
        .unwrap();
    let changes = store.poll_external_watches().unwrap();
    assert_eq!(
        changes.iter().map(|c| c.watch_id).collect::<Vec<_>>(),
        handles[..4].iter().map(|h| h.watch_id).collect::<Vec<_>>()
    );
    assert_eq!(changes[3].rows.columns, ["id"]);
    other
        .apply(NS, &[Mutation::delete("agent", Pred::eq("uid", "chosen"))])
        .unwrap();
    let changes = store.poll_external_watches().unwrap();
    assert_eq!(changes.len(), 4);
    assert!(changes[1..].iter().all(|c| c.rows.is_empty()));
}

#[test]
fn remote_writes_coalesce_to_the_latest_query_result() {
    let (store, dir) = defined();
    let other = reopen(&dir);
    store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("agent").select(["summary"]),
            WatchMode::Rows,
        )
        .unwrap();
    for summary in ["first", "second", "third"] {
        other
            .apply(
                NS,
                &[Mutation::put("agent")
                    .uid("one")
                    .set("summary", summary)
                    .build()],
            )
            .unwrap();
    }
    let changes = store.poll_external_watches().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(texts(&changes[0].rows, "summary"), ["third"]);
    assert!(store.poll_external_watches().unwrap().is_empty());
}

#[test]
fn polling_is_idle_without_watches_and_resumes_after_the_last_watch_is_removed() {
    let (store, dir) = store();
    let store = Arc::new(store);
    assert!(store.poll_external_watches().unwrap().is_empty());
    assert!(!dir.path().join("plugin-data.db").exists());
    let (send, receive) = mpsc::channel();
    let poller = store
        .start_watch_polling(move |store| {
            send.send(store.poll_external_watches().unwrap()).unwrap();
        })
        .unwrap();
    assert!(matches!(
        receive.recv_timeout(DATA_WATCH_POLL_INTERVAL * 2),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    let handle = store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("agent"),
            WatchMode::Rows,
        )
        .unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    store.unregister_watch(PLUGIN, handle.watch_id).unwrap();
    // An already-running callback may finish; joining first makes shutdown deterministic.
    drop(poller);
    while receive.try_recv().is_ok() {}
    assert!(matches!(
        receive.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let other = reopen(&dir);
    put(&other, "yinz", "while idle", 1);
    let handle = store
        .register_watch(
            NS,
            PLUGIN,
            CLIENT,
            FetchRequest::entity("agent"),
            WatchMode::Rows,
        )
        .unwrap();
    assert_eq!(texts(&handle.rows, "summary"), ["while idle"]);
    assert!(store.poll_external_watches().unwrap().is_empty());
    put(&other, "yinz", "after restart", 2);
    assert_eq!(store.poll_external_watches().unwrap().len(), 1);
    store.drop_watches_for_plugin(PLUGIN);
    assert!(store.poll_external_watches().unwrap().is_empty());
}
