use super::*;
#[test]
fn guest_hash_last_query_value_and_header_secrecy() {
    let request = WebRequest {
        id: 1,
        method: yinz_types::web::WebMethod::Get,
        path: "/api/x".into(),
        segments: vec!["api".into(), "x".into()],
        query: vec![("x".into(), "a".into()), ("x".into(), "b".into())],
        headers: vec![
            ("accept".into(), "application/json".into()),
            ("authorization".into(), "secret".into()),
            ("cookie".into(), "secret".into()),
        ],
        body: None,
        auth: yinz_types::web::WebAuth {
            session: "public".into(),
            label: "phone".into(),
        },
        deadline_ms: 100,
    };
    let session = crate::ServerSession::Fixed("test-session".into());
    let guest = guest_request(&request, None, &session);
    assert_eq!(guest["query"]["x"], "b");
    assert_eq!(guest["query_pairs"].as_array().unwrap().len(), 2);
    assert_eq!(guest["headers"].as_object().unwrap().len(), 1);
    assert_eq!(guest["server"]["session"], "test-session");
    assert!(guest["client"].is_null());
    assert_eq!(guest["attached"], false);
    assert!(!guest.to_string().contains("secret"));
    assert_eq!(
        guest_request(&request, Some(ClientId::new(3)), &session)["client"],
        3
    );
}

#[test]
fn restart_interval_is_controlled_without_waiting() {
    let host = WebHost::default();
    let mut requests = WebRequests::new(None, &host);
    assert!(requests.start_due());
    requests.last_start = Some(Instant::now());
    assert!(!requests.start_due());
    requests.last_start = Some(Instant::now() - std::time::Duration::from_secs(31));
    assert!(requests.start_due());
}

#[test]
fn unavailable_target_is_throttled_then_actually_unloaded_before_restart() {
    use std::{path::PathBuf, sync::Arc, time::Duration};
    use yinz_types::input::layout::{RunPlugin, RunPluginLocation, RunPluginOrAlias};
    let data = tempfile::tempdir().unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), data.path()).unwrap();
    let source = data.path().join("fixture.scm");
    std::fs::write(&source, "(define (load config) #f)").unwrap();
    let site = yinz_types::web::SiteHandle::default();
    let host = WebHost::new(Some(site.clone()));
    let mut bridge = PluginBridge::new(
        yinz_bus::thread_bus::ThreadSenders::default().silently_fail_on_send(),
        data.path().to_owned(),
        Arc::new(yinz_storage::data_access::DataAccess::sqlite(data.path())),
        PathBuf::from("/bin/sh"),
        data.path().to_owned(),
        Default::default(),
        None,
        None,
        None,
        vec![],
        vec![],
        Default::default(),
        Default::default(),
        Default::default(),
        crate::ServerSession::Fixed("restart-test".into()),
    )
    .unwrap();
    bridge.web = host.clone();
    bridge.add_client(ClientId::new(1)).unwrap();
    let mut requests = WebRequests::new(
        Some(WebPluginParams {
            plugin: RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::File(source),
                ..Default::default()
            }),
            site,
        }),
        &host,
    );
    fn settled(bridge: &mut PluginBridge, id: PluginId) {
        for apply in [true, false] {
            let (send, recv) = std::sync::mpsc::channel();
            assert!(bridge.plugin_executor.execute_for_plugin_tagged(
                id,
                "web fixture barrier",
                false,
                move |_, _, _| {
                    send.send(()).unwrap();
                }
            ));
            recv.recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            if apply {
                let (shutdown, _) = tokio::sync::mpsc::channel(1);
                bridge
                    .apply_cached_events(vec![id], false, shutdown)
                    .unwrap();
            }
        }
    }
    requests.activate(&mut bridge, ClientId::new(1));
    let old = requests.target.unwrap();
    settled(&mut bridge, old);
    let previous_generation = host.selected(old).unwrap();
    let instance = bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(old, None)
        .unwrap();
    instance.lock().unwrap().mark_failed();
    requests.activate(&mut bridge, ClientId::new(1));
    assert_eq!(requests.target, Some(old));
    requests.last_start = Some(Instant::now() - Duration::from_secs(31));
    requests.activate(&mut bridge, ClientId::new(1));
    let new = requests.target.unwrap();
    assert_ne!(old, new);
    settled(&mut bridge, new);
    assert_ne!(host.selected(new).unwrap(), previous_generation);
    assert!(bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(old, None)
        .is_none());
    bridge.cleanup(Instant::now() + Duration::from_secs(5));
}
