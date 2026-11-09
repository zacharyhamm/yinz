use super::*;

#[test]
fn update_pause_replays_only_complete_restored_manifests() {
    use yinz_types::data::{Event, PaneManifest};
    let plugin = Some(crate::PluginId::new(3));
    let client = Some(crate::ClientId::new(1));
    let mut reports = DeferredReports::default();
    let mut empty = vec![
        (plugin, client, Event::PaneUpdate(PaneManifest::default())),
        (plugin, client, Event::TabUpdate(vec![])),
    ];
    reports.retain(&mut empty);
    assert!(empty.is_empty());
    let mut restored = PaneManifest::default();
    restored
        .panes
        .insert(yinz_types::internal_ids::TabPosition::new(1), vec![]);
    let mut updates = vec![
        (plugin, client, Event::PaneUpdate(restored.clone())),
        (plugin, client, Event::InputReceived),
    ];
    reports.retain(&mut updates);
    assert!(matches!(updates.as_slice(), [(_, _, Event::InputReceived)]));
    let replay = reports.take();
    let manifests: Vec<_> = replay
        .iter()
        .filter_map(|(_, _, event)| {
            if let Event::PaneUpdate(manifest) = event {
                Some(manifest)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].panes, restored.panes);
    assert!(reports.take().is_empty());
}

#[test]
fn update_pause_complete_broadcast_supersedes_empty_targeted_manifests() {
    use yinz_types::data::{Event, PaneManifest};
    for (plugin, client) in [
        (None, Some(crate::ClientId::new(1))),
        (Some(crate::PluginId::new(3)), None),
        (None, None),
    ] {
        let mut reports = DeferredReports::default();
        reports.retain(&mut vec![(
            Some(crate::PluginId::new(3)),
            Some(crate::ClientId::new(1)),
            Event::PaneUpdate(PaneManifest::default()),
        )]);
        let mut full = PaneManifest::default();
        full.panes
            .insert(yinz_types::internal_ids::TabPosition::new(1), vec![]);
        reports.retain(&mut vec![(plugin, client, Event::PaneUpdate(full.clone()))]);
        let replay = reports.take();
        assert_eq!(replay.len(), 1);
        let Event::PaneUpdate(manifest) = &replay[0].2 else {
            panic!("expected full manifest")
        };
        assert_eq!(manifest.panes, full.panes);
    }
}

#[test]
fn update_pause_retry_retains_outstanding_native_writes() {
    let tasks = NativeTasks::default();
    let write = tasks.start();
    assert!(tasks.wait_until(Instant::now()).is_err());
    // A failed first quiescence must not consume the pending write's completion.
    assert!(tasks.wait_until(Instant::now()).is_err());
    drop(write);
    tasks.wait_until(Instant::now()).unwrap();
}

#[test]
fn update_pause_retirement_releases_waiters_without_allowing_guest_work() {
    let gate = PauseGate::default();
    let worker_gate = gate.clone();
    let (reply, arrival) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        worker_gate.arrive(reply);
        assert!(worker_gate.retired());
    });
    arrival
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    assert!(!gate.resumed());
    gate.retire();
    worker.join().unwrap();
}

#[test]
fn web_agents_native_snapshot_blocks_quiescence_and_paused_calls_never_dispatch() {
    use yinz_bus::{screen::ScreenInstruction, thread_bus::ThreadSenders, web::AgentViewCall};
    use yinz_runtime::channels::{self, SenderWithContext};
    let directory = tempfile::tempdir().unwrap();
    let data = std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
        directory.path(),
    ));
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    let (to_screen, screen) = channels::unbounded();
    let (to_plugin, plugin) = channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(to_screen)),
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        ..Default::default()
    };
    let call = AgentViewCall {
        plugin_id: crate::PluginId::new(8),
        client: crate::ClientId::new(2),
        generation: Some(91),
        id: "native-list".into(),
        request: r#"{"op":"list"}"#.into(),
    };
    let tasks = NativeTasks::default();
    let (shutdown, _shutdown_rx) = tokio::sync::mpsc::channel(1);
    let job = crate::web_agent_tasks::dispatch(
        call.clone(),
        false,
        &tasks,
        senders.clone(),
        data.clone(),
        shutdown.clone(),
    )
    .unwrap();
    let (ScreenInstruction::WebLiveAgents { reply }, _) = screen
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
    else {
        panic!("real native snapshot")
    };
    assert!(tasks.wait_until(Instant::now()).is_err());
    // The reply sender is the observed barrier: until released the native read is outstanding.
    assert!(
        crate::web_agent_tasks::dispatch(call.clone(), true, &tasks, senders, data, shutdown)
            .is_none()
    );
    assert!(screen.is_empty());
    let (paused, _) = plugin
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    let crate::PluginInstruction::PluginEvent(
        id,
        client,
        generation,
        yinz_types::data::Event::CustomMessage(name, body),
    ) = paused
    else {
        panic!("correlated paused reply")
    };
    assert_eq!(id, call.plugin_id);
    assert_eq!(client, call.client);
    assert_eq!(generation, 91);
    assert_eq!(name, "agent-view-response");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["id"], call.id);
    assert_eq!(body["reply"]["error"]["code"], "paused");
    assert!(tasks.wait_until(Instant::now()).is_err());
    reply.send(vec![]).unwrap();
    yinz_bus::global_async_runtime::get_tokio_runtime()
        .block_on(job)
        .unwrap();
    tasks.wait_until(Instant::now()).unwrap();
    let (done, _) = plugin
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    let crate::PluginInstruction::PluginEvent(
        _,
        _,
        91,
        yinz_types::data::Event::CustomMessage(_, body),
    ) = done
    else {
        panic!("native completion")
    };
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["reply"]["ok"]["agents"], serde_json::json!([]));
}
