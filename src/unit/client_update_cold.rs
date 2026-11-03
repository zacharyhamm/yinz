use super::*;
use crate::live_update::{
    journal::{Participant, Phase, Route, Store},
    transport::Connection,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct ColdFixture {
    socket: PathBuf,
    data: PathBuf,
    binary: PathBuf,
    operation: String,
    uncertain_adoption: bool,
}

#[test]
#[ignore = "subprocess fixture with an isolated session environment"]
fn cold_recovery_worker() {
    let request: ColdFixture = serde_json::from_slice(
        &fs::read(std::env::var_os("YINZ_TEST_COLD_UPDATE").unwrap()).unwrap(),
    )
    .unwrap();
    let mut store = Store::open(&request.socket, &request.data, &request.operation).unwrap();
    let mut source = Connection::connect(&request.socket).unwrap();
    let original: Inventory = serde_json::from_value(
        source
            .request(&request.operation, SessionCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    let installation: Installation = serde_json::from_value(
        crate::live_update::participate(yinz_types::update_client::ParticipantInvocation {
            executable: request.binary.clone(),
            data_dir: Some(request.data.clone()),
            request: ParticipantRequest::Inventory,
        })
        .unwrap(),
    )
    .unwrap();
    store.journal.source = Some(original.clone());
    store.journal.participants.push(Participant {
        installation,
        route: Route::Server,
        state: "prepared".into(),
    });
    store.journal.migration = Some(
        serde_json::from_value(
            source
                .request(&request.operation, SessionCommand::PrepareMigration)
                .unwrap(),
        )
        .unwrap(),
    );
    store.phase(Phase::Transfer).unwrap();
    source
        .request(
            &request.operation,
            SessionCommand::ReleaseMigration {
                snapshot: store.journal.snapshot.clone(),
            },
        )
        .unwrap();
    Transaction::open(&request.binary, &request.data, &request.operation)
        .unwrap()
        .publish()
        .unwrap();
    store.journal.participants[0].state = "published".into();
    store.save().unwrap();
    let snapshot = store.journal.snapshot.clone();
    if request.uncertain_adoption {
        let mut document: yinz_types::session_migration::SessionMigrationSnapshot =
            serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
        // Synthetic failure receipt: no destination has consumed these buffers.
        document.ownership[0].1 = yinz_types::session_migration::MigrationOwnership::Adopting;
        crate::live_update::journal::atomic_json(&snapshot, &document).unwrap();
    }
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(original.server_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let gone = Watched::new(Ok(false));
    std::thread::scope(|scope| {
        let gone = &gone;
        scope.spawn(move || {
            // This server is not our child, so waitpid cannot observe its exit.
            // There is no portable exit notification for an arbitrary PID; poll
            // liveness within the existing deadline and publish the actual result.
            let deadline = Instant::now() + TIMEOUT;
            loop {
                let state = yinz_process::liveness::has_exited(original.server_pid)
                    .map_err(|error| format!("server PID {}: {error}", original.server_pid));
                let done = !matches!(state, Ok(false));
                *gone.lock() = state;
                if done {
                    return;
                }
                if Instant::now() >= deadline {
                    *gone.lock() = Err(format!(
                        "timed out waiting for server PID {} to exit; last observed alive",
                        original.server_pid
                    ));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        gone.wait_until_described(
            "source process to exit before cold recovery",
            TIMEOUT,
            |state| (!matches!(state, Ok(false))).then(|| state.clone()),
            |state| format!("server PID {}: {state:?}", original.server_pid),
        )
        .expect("source process to exit before cold recovery");
    });
    drop(source);
    drop(store);
    if request.uncertain_adoption {
        let error = crate::live_update::coordinator::run(
            &request.socket,
            &request.data,
            &request.operation,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("unacknowledged adoption"));
        assert_eq!(
            Transaction::open(&request.binary, &request.data, &request.operation)
                .unwrap()
                .status(),
            crate::update::transaction::Status::Published
        );
        // Correct only the synthetic receipt, then recover the untouched actual
        // ownership so the fixture leaves no detached supervisors behind.
        let mut document: yinz_types::session_migration::SessionMigrationSnapshot =
            serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
        document.ownership[0].1 = yinz_types::session_migration::MigrationOwnership::Released;
        crate::live_update::journal::atomic_json(&snapshot, &document).unwrap();
    }
    crate::live_update::coordinator::run(&request.socket, &request.data, &request.operation)
        .unwrap();
    // A lost caller acknowledgement does not re-adopt or replay an active source.
    crate::live_update::coordinator::run(&request.socket, &request.data, &request.operation)
        .unwrap();
}

fn cold_recovery(uncertain_adoption: bool) {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.path().join("config.kdl"),
        "detachable_panes true\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n",
    )
    .unwrap();
    fixture.start();
    let initial = Attached::start(&fixture);
    initial.wait_for("READY-first");
    let pids: Vec<i32> = ["first-input.pid", "second-input.pid"]
        .iter()
        .map(|name| {
            fs::read_to_string(fixture.root.path().join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        })
        .collect();
    let operation = uuid::Uuid::new_v4().to_string();
    let old_inode = fs::metadata(&fixture.binary).unwrap().ino();
    fixture.prepare(&operation);
    let request = fixture.root.path().join("cold-recovery.json");
    fs::write(
        &request,
        serde_json::to_vec(&ColdFixture {
            socket: fixture.socket(),
            data: fixture.data.clone(),
            binary: fixture.binary.clone(),
            operation: operation.clone(),
            uncertain_adoption,
        })
        .unwrap(),
    )
    .unwrap();
    let log_path = fixture.root.path().join("cold-recovery.log");
    let log = File::create(&log_path).unwrap();
    let environment = fixture.command();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    for (key, value) in environment.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    command
        .current_dir(environment.get_current_dir().unwrap())
        .env("YINZ_TEST_COLD_UPDATE", &request)
        .args([
            "--exact",
            "live_update::tests::client_update_real::cold::cold_recovery_worker",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    let mut child = command.spawn().unwrap();
    let result = Watched::new(None);
    let status = std::thread::scope(|scope| {
        let result = &result;
        let child = &mut child;
        scope.spawn(move || {
            // Child::wait has no timeout and holds the child needed for cleanup.
            // Keep bounded try_wait polling so a stuck worker can be killed and
            // reaped without leaving a scoped waiter blocked indefinitely.
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *result.lock() = Some(Ok(status));
                        return;
                    },
                    Ok(None) => {},
                    Err(error) => {
                        let killed = child.kill();
                        let reaped = child.wait();
                        *result.lock() = Some(Err(format!(
                            "failed to observe worker: {error}; kill: {killed:?}; reap: {reaped:?}"
                        )));
                        return;
                    },
                }
                if Instant::now() >= deadline {
                    let killed = child.kill();
                    let reaped = child.wait();
                    *result.lock() = Some(Err(format!(
                        "timed out after 180s waiting for cold update recovery worker; kill: {killed:?}; reap: {reaped:?}"
                    )));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        result.wait_until_described(
            "cold update recovery worker",
            Duration::from_secs(190),
            |state| state.clone(),
            |state| format!(
                "worker outcome: {state:?}; log: {}",
                fs::read_to_string(&log_path).unwrap_or_default()
            ),
        )
    }).unwrap_or_else(|error| panic!(
        "{error}; log: {}",
        fs::read_to_string(&log_path).unwrap_or_default()
    ));
    assert!(
        status.success(),
        "{}",
        fs::read_to_string(log_path).unwrap()
    );
    assert_eq!(fs::metadata(&fixture.binary).unwrap().ino(), old_inode);
    for pid in pids {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).unwrap();
    }
    drop(initial);
    let mut resumed = Attached::start(&fixture);
    resumed.screen.wait_until(
        "restored terminal content",
        Duration::from_secs(15),
        |screen| {
            let text = String::from_utf8_lossy(screen);
            if text.contains("READY-first") {
                Some(())
            } else {
                None
            }
        },
    );
    resumed.master.write_all(b"after-cold-recovery\n").unwrap();
    resumed.wait_for("ECHO-first:after-cold-recovery");
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("first-input")).unwrap(),
        "after-cold-recovery\n"
    );
    let journal: crate::live_update::journal::Journal = serde_json::from_slice(
        &fs::read(
            fixture
                .data
                .join("updates")
                .join(operation)
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal.phase, Phase::Failed);
    assert!(journal.source_restored);
    assert!(journal.recovery_source_identity.is_some());
}

#[test]
fn cold_update_recovers_released_source_after_process_death() {
    cold_recovery(false);
}

#[test]
fn cold_update_refuses_unacknowledged_adoption_without_deleting_backups() {
    cold_recovery(true);
}
