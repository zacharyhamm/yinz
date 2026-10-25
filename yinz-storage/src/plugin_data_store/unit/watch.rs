use super::*;

#[test]
fn exhausted_watch_ids_do_not_reuse_an_identity_or_add_a_watch() {
    let watches = WatchRegistry::new();
    watches.lock().unwrap().1 = DataWatchId::new(u64::MAX);
    let result = watches.register(
        "test",
        PluginId::new(1),
        ClientId::new(1),
        FetchRequest::entity("row"),
        DataRows::default(),
    );
    assert!(matches!(result, Err(DataError::Storage(_))));
    assert_eq!(watches.count(), 0);
    assert_eq!(watches.lock().unwrap().1, DataWatchId::new(u64::MAX));
}
