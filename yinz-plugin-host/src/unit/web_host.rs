use super::*;
fn host() -> (WebHost, SiteHandle) {
    let site = SiteHandle::default();
    let host = WebHost::new(Some(site.clone()));
    host.set_target(Some(PluginId::new(1)));
    (host, site)
}
fn register(host: &WebHost, generation: u64) -> Arc<Mutex<WebPending>> {
    let pending = Arc::default();
    host.register(
        PluginId::new(1),
        generation,
        Arc::clone(&pending),
        Arc::default(),
    );
    pending
}
#[test]
fn generation_ownership_retirement_and_nested_prefixes() {
    let (host, site) = host();
    let pending = register(&host, 1);
    register(&host, 2);
    let file = crate::web_assets::freeze(b"page".to_vec(), "text/plain; charset=utf-8");
    host.publish_assets(PluginId::new(1), 1, "/static/".into(), BTreeMap::new())
        .unwrap();
    host.publish_assets(
        PluginId::new(1),
        1,
        "/static/vendor/".into(),
        BTreeMap::new(),
    )
    .unwrap();
    assert!(host
        .publish_page(PluginId::new(1), 2, "/".into(), file.clone())
        .is_err());
    assert!(host
        .publish_page(PluginId::new(1), 1, "/static/x".into(), file.clone())
        .is_err());
    let snapshot = site.current().unwrap();
    let (slot, mut receiver) = ReplySlot::new();
    let clone = slot.clone();
    pending.lock().unwrap().insert(7, slot);
    host.retire(1);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WebReply::Unavailable {
            retry_after_s: 5,
            ..
        }
    ));
    assert!(clone.is_closed());
    host.publish_page(PluginId::new(1), 2, "/".into(), file)
        .unwrap();
    host.retire(1);
    assert_eq!(site.current().unwrap().generation, 2);
    assert_eq!(snapshot.assets.len(), 2);
}
#[test]
fn paths_limits_and_atomic_collision_failures() {
    let (host, site) = host();
    register(&host, 1);
    let file = crate::web_assets::freeze(vec![], "text/plain; charset=utf-8");
    for path in [
        "/api", "/api/x", "/../x", "/x//y", "/a/./b", "/UPPER", "//host",
    ] {
        assert!(
            host.publish_page(PluginId::new(1), 1, path.into(), file.clone())
                .is_err(),
            "{path}"
        );
    }
    host.publish_page(PluginId::new(1), 1, "/static/page".into(), file.clone())
        .unwrap();
    let previous = site.current().unwrap();
    assert!(host
        .publish_assets(PluginId::new(1), 1, "/static/".into(), BTreeMap::new())
        .is_err());
    assert!(Arc::ptr_eq(&previous, &site.current().unwrap()));
    for i in 0..15 {
        host.publish_page(PluginId::new(1), 1, format!("/p{i}"), file.clone())
            .unwrap();
    }
    assert!(host
        .publish_page(PluginId::new(1), 1, "/overflow".into(), file.clone())
        .is_err());
    host.publish_page(PluginId::new(1), 1, "/p0".into(), file)
        .unwrap();
}

#[test]
fn admission_caps_sweeps_duplicates_and_rejects_retired_generations() {
    let (host, _) = host();
    let pending = register(&host, 1);
    let mut receivers = Vec::new();
    for id in 0..64 {
        let (slot, receiver) = ReplySlot::new();
        host.admit(PluginId::new(1), 1, id, slot).unwrap();
        receivers.push(receiver);
    }
    let (overflow, _) = ReplySlot::new();
    assert!(matches!(
        host.admit(PluginId::new(1), 1, 65, overflow),
        Err(WebReply::Unavailable {
            retry_after_s: 2,
            ..
        })
    ));
    drop(receivers.remove(0));
    let (duplicate, receiver) = ReplySlot::new();
    assert!(host.admit(PluginId::new(1), 1, 1, duplicate).is_err());
    drop(receiver);
    let (replacement, receiver) = ReplySlot::new();
    host.admit(PluginId::new(1), 1, 65, replacement).unwrap();
    assert_eq!(pending.lock().unwrap().len(), 64);
    host.retire(1);
    assert!(receiver.blocking_recv().is_ok());
    let (stale, _) = ReplySlot::new();
    assert!(host.admit(PluginId::new(1), 1, 0, stale).is_err());
}

#[test]
fn publication_boundaries_and_reverse_nested_prefix_order() {
    let (host, _) = host();
    register(&host, 1);
    for prefix in ["/static/vendor/", "/static/", "/other/", "/fourth/"] {
        host.publish_assets(PluginId::new(1), 1, prefix.into(), BTreeMap::new())
            .unwrap();
    }
    assert!(host
        .publish_assets(PluginId::new(1), 1, "/fifth/".into(), BTreeMap::new())
        .is_err());
    host.publish_assets(PluginId::new(1), 1, "/static/".into(), BTreeMap::new())
        .unwrap();
    for prefix in ["/api/", "/a/../", "/a//", "/no-trailing", "/a.b/"] {
        assert!(host
            .publish_assets(PluginId::new(1), 1, prefix.into(), BTreeMap::new())
            .is_err());
    }
    let page = crate::web_assets::freeze(vec![0; 512 * 1024], "text/plain; charset=utf-8");
    host.publish_page(PluginId::new(1), 1, "/".into(), page)
        .unwrap();
    let page = crate::web_assets::freeze(vec![0; 512 * 1024 + 1], "text/plain; charset=utf-8");
    assert!(host
        .publish_page(PluginId::new(1), 1, "/".into(), page)
        .is_err());
}

#[test]
fn dropped_executor_delivery_resolves_even_with_an_extra_slot_clone() {
    let (slot, mut receiver) = ReplySlot::new();
    let guard = QueuedReply::new(slot.clone());
    drop(guard);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WebReply::Unavailable { .. }
    ));
    assert!(slot.is_closed());
}
