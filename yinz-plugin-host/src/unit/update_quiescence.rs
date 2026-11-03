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
