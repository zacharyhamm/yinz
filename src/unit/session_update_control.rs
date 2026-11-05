use super::*;

fn record_interrupted_update(session: &Session, source: &mut Peer, phase: &str) -> String {
    let operation = uuid::Uuid::new_v4().to_string();
    let data = session.0.path().join("data/yinz");
    let directory = data.join("updates").join(&operation);
    fs::create_dir_all(&directory).unwrap();
    let inventory = source
        .request(&operation, UpdateCommand::Inventory)
        .unwrap();
    let socket = yinz_runtime::session_update::canonical_socket(&session.socket()).unwrap();
    let journal = serde_json::json!({
        "format":1, "operation":operation, "phase":phase, "source_build":false, "channel":"Stable",
        "source_socket":socket, "replacement_socket":socket.with_file_name(format!(".update-{operation}")),
        "source":inventory, "migration":null, "release":{"tag_name":"v0.1.1", "assets":[]},
        "participants":[], "snapshot":directory.join("snapshot.json"), "mapping":null,
        "restarted_clients":[], "disconnected_clients":[], "restart_required":[],
        "errors":["source server is newer than the selected release; refusing downgrade"]
    });
    fs::write(
        directory.join("journal.json"),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();
    fs::write(
        yinz_runtime::session_update::record(&data, &socket).unwrap(),
        serde_json::to_vec(&operation).unwrap(),
    )
    .unwrap();
    operation
}

fn wait_for_cancelled(source: &mut Peer, operation: &str) {
    let observed = yinz_test_support::Watched::new(serde_json::Value::Null);
    std::thread::scope(|scope| {
        let observed = &observed;
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                let status = source.request(operation, UpdateCommand::Status).unwrap();
                let done = status["phase"] == "Cancelled";
                *observed.lock() = status;
                if done {
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        observed.wait_until(
            "idle coordinator to finish cancellation",
            Duration::from_secs(35),
            |status| (status["phase"] == "Cancelled").then_some(()),
        );
    });
}

#[test]
fn cancel_stuck_discover_without_a_coordinator_finishes_cancelled() {
    let (session, pids) = setup();
    let mut source = Peer::connect(&session.socket());
    let operation = record_interrupted_update(&session, &mut source, "Discover");
    source.request(&operation, UpdateCommand::Cancel).unwrap();
    wait_for_cancelled(&mut source, &operation);
    assert_alive(&pids);
}

#[test]
fn cancel_interrupted_prepare_restarts_cleanup_without_a_coordinator() {
    let (session, pids) = setup();
    let mut source = Peer::connect(&session.socket());
    let operation = record_interrupted_update(&session, &mut source, "Prepare");
    source
        .request(&operation, UpdateCommand::PrepareMigration)
        .unwrap();
    source.request(&operation, UpdateCommand::Cancel).unwrap();
    wait_for_cancelled(&mut source, &operation);
    assert_alive(&pids);
}

#[test]
fn active_stable_update_rejects_prerelease_and_source_starts_even_during_launch() {
    let (session, pids) = setup();
    let mut source = Peer::connect(&session.socket());
    let operation = record_interrupted_update(&session, &mut source, "Discover");
    let data = session.0.path().join("data/yinz");
    let record = yinz_runtime::session_update::record(&data, &session.socket()).unwrap();
    // Both fast paths must enforce the same choice: launch in progress and an
    // already running coordinator, as well as an idle interrupted operation.
    for extension in [None, Some("launch"), Some("lock")] {
        let _lock = extension.map(|extension| {
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(record.with_extension(extension))
                .unwrap();
            lock.lock().unwrap();
            lock
        });
        for command in [UpdateCommand::StartPrerelease, UpdateCommand::StartSource] {
            let error = source
                .request(&uuid::Uuid::new_v4().to_string(), command)
                .unwrap_err();
            assert!(
                error.contains("stable update is already in progress; cancel it first"),
                "{error}"
            );
        }
        let status = source.request(&operation, UpdateCommand::Status).unwrap();
        assert_eq!(status["operation"], operation);
        assert_eq!(status["phase"], "Discover");
        assert_eq!(status["channel"], "Stable");
    }
    assert_alive(&pids);
}
