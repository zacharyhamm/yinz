use super::*;
use std::sync::atomic::AtomicBool;
use yinz_types::data::Event;

fn request(generation: u64, id: usize) -> Request {
    Request::new(
        generation,
        ClientId::new(1),
        vec![PermissionType::RunCommands],
        id.to_string(),
    )
}

#[test]
fn enqueue_compacts_duplicates_and_bounds_completion_ids_in_bytes() {
    let mut permissions = Permissions::default();
    let mut input = request(1, 0);
    input.permissions = vec![PermissionType::RunCommands; 100_000];
    input.permissions.push(PermissionType::ReadFiles);
    input.id = String::with_capacity(100_000);
    input.id.push_str(&"é".repeat(64));
    permissions.enqueue(input).unwrap();
    let queued = permissions.queue.front().unwrap();
    assert_eq!(
        queued.permissions,
        vec![PermissionType::RunCommands, PermissionType::ReadFiles]
    );
    assert_eq!(queued.permissions.capacity(), 2);
    assert_eq!(queued.id.capacity(), MAX_COMPLETION_ID_BYTES);
    let mut oversized = request(1, 1);
    oversized.id = "é".repeat(65);
    assert!(permissions
        .enqueue(oversized)
        .unwrap_err()
        .to_string()
        .contains("128 bytes"));
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
}

#[test]
fn request_limit_spans_dispatch_and_completion_delivery_and_preserves_fifo_grants() {
    let mut permissions = Permissions::default();
    for id in 0..MAX_REQUESTS {
        let mut input = request(1, id);
        if id > 0 {
            input.permissions = vec![PermissionType::ReadFiles];
        }
        permissions.enqueue(input).unwrap();
    }
    assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
    let dispatch = permissions.next().unwrap();
    assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
    assert!(permissions.answered(&dispatch.permissions, PermissionStatus::Granted));
    let completion = permissions.take_answer().unwrap();
    assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
    // Delivery can outlive both the active slot and the dispatch clone.
    drop(dispatch);
    assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
    drop(completion);
    permissions.enqueue(request(1, MAX_REQUESTS)).unwrap();
    for id in 1..=MAX_REQUESTS {
        let dispatch = permissions.next().unwrap();
        assert_eq!(dispatch.id, id.to_string());
        assert_eq!(
            dispatch.permissions,
            vec![PermissionType::RunCommands, PermissionType::ReadFiles]
        );
        drop(permissions.finish(Ok(PermissionStatus::Granted)));
    }
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
}

#[test]
fn reservations_drop_on_dispatch_failure_cancelled_queues_and_shutdown() {
    let mut permissions = Permissions::default();
    permissions.enqueue(request(1, 0)).unwrap();
    drop(permissions.next());
    let failure = permissions.finish(Err("dispatch failed".into())).unwrap();
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
    drop(failure);
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
    permissions.enqueue(request(1, 1)).unwrap();
    permissions.enqueue(request(1, 2)).unwrap();
    drop(permissions.next());
    let completion = permissions.finish(Ok(PermissionStatus::Granted)).unwrap();
    permissions.completions.push_back(completion);
    permissions.forget(1);
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);

    permissions.enqueue(request(2, 3)).unwrap();
    permissions.enqueue(request(2, 4)).unwrap();
    let dispatch = permissions.next().unwrap();
    permissions.forget(2);
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
    permissions.stop();
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
    drop(dispatch);
    assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
}

#[test]
fn dispatcher_failures_release_immediate_and_deferred_requests() {
    let workers = background_worker::Workers::new(1);
    let mut env = crate::test_env::test_env();
    env.senders = Default::default(); // No host receiver: both dispatch branches fail.
    assert!(workers
        .request_permissions(&mut env, request(1, 0))
        .is_err());
    assert_eq!(
        workers
            .permissions
            .lock()
            .unwrap()
            .outstanding
            .load(Ordering::Relaxed),
        0
    );
    workers
        .permissions
        .lock()
        .unwrap()
        .enqueue(request(1, 1))
        .unwrap();
    workers.dispatch_permissions(&mut env, None).unwrap();
    let completions = workers.take_permission_completions(&mut env).unwrap();
    assert_eq!(completions.len(), 1);
    assert!(completions[0].result.is_err());
    assert_eq!(
        workers
            .permissions
            .lock()
            .unwrap()
            .outstanding
            .load(Ordering::Relaxed),
        1
    );
    drop(completions);
    assert_eq!(
        workers
            .permissions
            .lock()
            .unwrap()
            .outstanding
            .load(Ordering::Relaxed),
        0
    );
}

#[test]
fn undeliverable_worker_completions_release_the_reservation() {
    let workers = background_worker::Workers::new(1);
    let outstanding = {
        let mut permissions = workers.permissions.lock().unwrap();
        permissions.enqueue(request(2, 0)).unwrap();
        drop(permissions.next());
        permissions.outstanding.clone()
    };
    let completion = workers
        .permissions
        .lock()
        .unwrap()
        .finish(Ok(PermissionStatus::Granted))
        .unwrap();
    workers.complete_permission(&crate::test_env::test_env(), completion);
    assert_eq!(outstanding.load(Ordering::Relaxed), 0);
}

#[test]
fn stopped_workers_keep_their_inflight_answer_and_disposal_clears_the_fifo() {
    let request = |generation| Request {
        generation,
        client: ClientId::new(1),
        permissions: vec![PermissionType::RunCommands],
        id: "1".into(),
        reservation: None,
    };
    let mut permissions = Permissions::default();
    permissions.enqueue(request(2)).unwrap();
    permissions.enqueue(request(2)).unwrap();
    permissions.enqueue(request(3)).unwrap();
    assert_eq!(permissions.next().unwrap().generation, 2);
    permissions.forget(2);
    assert!(permissions.next().is_none());
    assert!(permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
    assert!(!permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
    assert_eq!(permissions.take_answer().unwrap().request.generation, 2);
    assert_eq!(permissions.next().unwrap().generation, 3);
    permissions.stop();
    assert!(!permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
    assert!(permissions.finish(Err("dispatch stopped".into())).is_none());
    assert!(permissions.take_answer().is_none());
    assert!(permissions.next().is_none());
    assert!(permissions.enqueue(request(4)).is_err());
}

#[test]
fn cancellation_leaves_answered_permissions_queued_for_ticks_and_events() {
    let cancel_client = Arc::new(AtomicBool::new(false));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("permissions.scm");
    std::fs::write(&path, "(void)").unwrap();
    let mut plugin = crate::steel::SteelPlugin::new(
        "(void)",
        &path,
        crate::test_env::test_env(),
        cancel_client.clone(),
    )
    .unwrap();
    let workers = plugin.env().workers.clone().unwrap();
    let first = Request {
        generation: plugin.generation(),
        client: ClientId::new(1),
        permissions: vec![PermissionType::RunCommands],
        id: "first".into(),
        reservation: None,
    };
    let second = Request {
        id: "second".into(),
        ..first.clone()
    };
    {
        let mut permissions = workers.permissions.lock().unwrap();
        permissions.enqueue(first).unwrap();
        permissions.enqueue(second).unwrap();
        let active = permissions.next().unwrap();
        assert!(permissions.answered(&active.permissions, PermissionStatus::Granted));
    }

    plugin.env().cancel.store(true, Ordering::Release);
    for result in [
        plugin.tick().map(|_| ()),
        plugin
            .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
            .map(|_| ()),
    ] {
        assert!(result.unwrap_err().to_string().contains(CANCELLED_BY_HOST));
    }
    assert!(!plugin.env().cancel_tripped.load(Ordering::Acquire));
    let permissions = workers.permissions.lock().unwrap();
    assert_eq!(
        permissions
            .active
            .as_ref()
            .map(|request| request.id.as_str()),
        Some("first")
    );
    assert_eq!(
        permissions.queue.front().map(|request| request.id.as_str()),
        Some("second")
    );
    assert_eq!(permissions.answer, Some(PermissionStatus::Granted));
    drop(permissions);

    plugin.env().cancel.store(false, Ordering::Release);
    cancel_client.store(true, Ordering::Release);
    assert!(!plugin.tick().unwrap());
    assert!(!plugin
        .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
        .unwrap());
}
