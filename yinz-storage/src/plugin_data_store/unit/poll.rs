use super::*;
use yinz_types::internal_ids::{ClientId, PluginId};
use yinz_types::plugin_data::{AttrDef, EntityDef, FetchRequest, WatchMode};

#[test]
fn the_timer_is_active_exactly_while_watches_exist() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    store
        .define_entity(
            "shared:test",
            "test",
            &EntityDef::new("item", 1).attr(AttrDef::text("name")),
            false,
        )
        .unwrap();
    let register = |plugin| {
        store
            .register_watch(
                "shared:test",
                PluginId::new(plugin),
                ClientId::new(1),
                FetchRequest::entity("item"),
                WatchMode::Rows,
            )
            .unwrap()
    };
    assert!(!*store.activity.active.lock().unwrap());
    let first = register(1);
    let second = register(2);
    assert!(*store.activity.active.lock().unwrap());
    store
        .unregister_watch(PluginId::new(1), first.watch_id)
        .unwrap();
    assert!(*store.activity.active.lock().unwrap());
    assert!(store
        .unregister_watch(PluginId::new(1), second.watch_id)
        .is_err());
    assert!(*store.activity.active.lock().unwrap());
    store
        .unregister_watch(PluginId::new(2), second.watch_id)
        .unwrap();
    assert!(!*store.activity.active.lock().unwrap());
    register(3);
    assert!(*store.activity.active.lock().unwrap());
    store.drop_watches_for_plugin(PluginId::new(3));
    assert!(!*store.activity.active.lock().unwrap());
}
