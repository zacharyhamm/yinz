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
    let gone = Watched::new(false);
    std::thread::scope(|scope| {
        let gone = &gone;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                if yinz_process::liveness::has_exited(original.server_pid).unwrap() {
                    *gone.lock() = true;
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        gone.wait_until(
            "source process to exit before cold recovery",
            TIMEOUT,
            |gone| (*gone).then_some(()),
        );
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
    std::thread::scope(|scope| {
        let result = &result;
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(180);
            while Instant::now() < deadline {
                if let Some(status) = child.try_wait().unwrap() {
                    *result.lock() = Some(status);
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
            child.kill().unwrap();
            *result.lock() = Some(child.wait().unwrap());
        });
        result.wait_until(
            "cold update recovery worker",
            Duration::from_secs(190),
            |status| status.map(|_| ()),
        );
    });
    assert!(
        result.lock().unwrap().success(),
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
