use super::*;
use crate::live_update::{
    journal::{Phase, Store},
    transport::Connection,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct RetryFixture {
    coordinator: CoordinatorFixture,
    server_binary: PathBuf,
    client_binary: PathBuf,
    client_data: PathBuf,
    client: ClientId,
    disconnected_before_attempt: ClientId,
}

#[test]
#[ignore = "subprocess fixture with an isolated session environment"]
fn failed_readiness_worker() {
    let request: RetryFixture = serde_json::from_slice(
        &fs::read(std::env::var_os("YINZ_TEST_UPDATE_RETRY").unwrap()).unwrap(),
    )
    .unwrap();
    let coordinator = &request.coordinator;
    // Only the coordinator's readiness deadline is shortened. The fixture runs
    // actual production clients, servers, supervisors, and participant helpers.
    crate::live_update::coordinator::CLIENT_READY_TIMEOUT.set(Duration::from_secs(3));
    let mut store = Store::open(
        &coordinator.socket,
        &coordinator.data,
        &coordinator.operation,
    )
    .unwrap();
    let mut source = Connection::connect(&coordinator.socket).unwrap();
    let mut inventory: Inventory = serde_json::from_value(
        source
            .request(&coordinator.operation, SessionCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    let source_pid = inventory.server_pid;
    let mut control = Peer::connect(&coordinator.socket);
    control
        .sender
        .send_client_msg(ClientToServerMsg::DetachSession {
            client_ids: vec![request.disconnected_before_attempt],
        })
        .unwrap();
    // DetachSession closes the command connection after dispatching its action.
    let mut control = Peer::connect(&coordinator.socket);
    let detached = Watched::new(false);
    std::thread::scope(|scope| {
        let detached = &detached;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                if !control
                    .inventory(&coordinator.operation)
                    .clients
                    .iter()
                    .any(|client| client.id == request.disconnected_before_attempt)
                {
                    *detached.lock() = true;
                    return;
                }
            }
        });
        detached.wait_until(
            "ordinary attachment to detach before update preparation",
            TIMEOUT,
            |detached| detached.then_some(()),
        );
    });
    inventory.running_version = "0.0.0".into();
    let error = crate::live_update::coordinator::execute_release(
        &mut store,
        &mut source,
        inventory,
        coordinator.release.clone(),
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("did not acknowledge replacement readiness"),
        "{error:#}"
    );
    assert!(store.journal.restart_attempted.contains(&request.client));
    assert!(store.journal.restart_required.contains(&request.client));
    assert!(!store.journal.disconnected_clients.contains(&request.client));
    assert!(store
        .journal
        .disconnected_clients
        .contains(&request.disconnected_before_attempt));
    assert!(!store
        .journal
        .restart_attempted
        .contains(&request.disconnected_before_attempt));
    assert_eq!(store.journal.phase, Phase::RecoveryRequired);
    drop(store);
    drop(source);
    // The original attachment is now gone. Repeating the update must not treat
    // that disappearance as permission to commit the unacknowledged handoff.
    let error = crate::live_update::coordinator::run(
        &coordinator.socket,
        &coordinator.data,
        &coordinator.operation,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("did not acknowledge replacement readiness"),
        "{error:#}"
    );
    let store = Store::open(
        &coordinator.socket,
        &coordinator.data,
        &coordinator.operation,
    )
    .unwrap();
    assert_eq!(store.journal.phase, Phase::RecoveryRequired);
    assert!(store.journal.handoff_committed);
    assert_eq!(store.journal.restart_attempted, vec![request.client]);
    assert!(store.journal.restart_required.contains(&request.client));
    assert!(!store.journal.disconnected_clients.contains(&request.client));
    assert!(store
        .journal
        .disconnected_clients
        .contains(&request.disconnected_before_attempt));
    assert!(store.journal.restarted_clients.is_empty());
    for (binary, data) in [
        (&request.server_binary, &coordinator.data),
        (&request.client_binary, &request.client_data),
    ] {
        assert_eq!(
            Transaction::open(binary, data, &coordinator.operation)
                .unwrap()
                .status(),
            crate::update::transaction::Status::Published
        );
    }
    drop(store);
    // A later source crash cannot resolve an already attempted restart either.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(source_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let dead = Watched::new(Ok(false));
    std::thread::scope(|scope| {
        let dead = &dead;
        scope.spawn(move || {
            // This server is not our child, so waitpid cannot observe its exit.
            // There is no portable exit notification for an arbitrary PID; poll
            // liveness within the existing deadline and publish the actual result.
            let deadline = Instant::now() + TIMEOUT;
            loop {
                let state = yinz_process::liveness::has_exited(source_pid)
                    .map_err(|error| format!("server PID {}: {error}", source_pid));
                let done = !matches!(state, Ok(false));
                *dead.lock() = state;
                if done {
                    return;
                }
                if Instant::now() >= deadline {
                    *dead.lock() = Err(format!(
                        "timed out waiting for server PID {} to exit; last observed alive",
                        source_pid
                    ));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        dead.wait_until_described(
            "source server death before committed recovery",
            TIMEOUT,
            |state| (!matches!(state, Ok(false))).then(|| state.clone()),
            |state| format!("server PID {}: {state:?}", source_pid),
        )
        .expect("source server death before committed recovery");
    });
    let error = crate::live_update::coordinator::run(
        &coordinator.socket,
        &coordinator.data,
        &coordinator.operation,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("before 1 clients acknowledged restart"),
        "{error:#}"
    );
    let store = Store::open(
        &coordinator.socket,
        &coordinator.data,
        &coordinator.operation,
    )
    .unwrap();
    assert!(store.journal.restart_required.contains(&request.client));
    assert!(!store.journal.disconnected_clients.contains(&request.client));
    assert_eq!(
        Transaction::open(
            &request.server_binary,
            &coordinator.data,
            &coordinator.operation
        )
        .unwrap()
        .status(),
        crate::update::transaction::Status::Published
    );
}

pub(super) fn receipt(
    data: &Path,
    binary: &Path,
    operation: &str,
    release: &crate::update::Release,
) {
    use sha2::{Digest, Sha256};
    let directory = data.join("updates").join(operation).join("participants");
    fs::create_dir_all(&directory).unwrap();
    let binary = binary.canonicalize().unwrap();
    let key = crate::update::hex(&Sha256::digest(binary.as_os_str().as_encoded_bytes()));
    fs::write(
        directory.join(format!("{key}.json")),
        serde_json::to_vec(release).unwrap(),
    )
    .unwrap();
}

#[test]
fn coordinator_keeps_backups_after_client_startup_failure_and_repeated_recovery() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.path().join("config.kdl"),
        "detachable_panes true\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n",
    )
    .unwrap();
    fixture.start();
    let client_install = Fixture::new();
    let mut command = client_install.command();
    command
        .env("YINZ_SOCKET_DIR", fixture.root.path().join("sockets"))
        .args(["attach", SESSION]);
    let mut attached = Attached::start_command(command);
    attached.wait_for("READY-first");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&fixture.socket());
    let before = source.inventory(&operation);
    let client = before
        .clients
        .iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let ordinary = Attached::start(&fixture);
    ordinary.wait_for("READY-first");
    let disconnected_before_attempt = source
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|entry| entry.supports_update && entry.id != client)
        .unwrap()
        .id;
    fixture.prepare(&operation);
    let release = crate::update::Release {
        channel: yinz_types::session_update::ReleaseChannel::Prerelease,
        source: None,
        tag_name: yinz_runtime::consts::VERSION.into(),
        assets: vec![],
    };
    receipt(&fixture.data, &fixture.binary, &operation, &release);
    let candidate = client_install.root.path().join("candidate");
    fs::create_dir(&candidate).unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &candidate).unwrap();
    // A client-only fixture executable serves every installation helper request
    // through the production binary, but its actual resumed process cannot start.
    // This models a failure after exec that no old process can acknowledge away.
    let helper = fixture.binary.to_string_lossy().replace('\'', "'\\''");
    yinz_test_support::write_warm_script(&candidate.join("yinz"), &format!("#!/bin/sh\n[ \"$#\" -gt 0 ] || exit 0\n[ \"$1\" != --resume-client ] || exit 97\nexec '{helper}' \"$@\"\n"));
    drop(
        Transaction::prepare(
            &candidate,
            &client_install.binary,
            &client_install.data,
            &operation,
        )
        .unwrap(),
    );
    receipt(
        &client_install.data,
        &client_install.binary,
        &operation,
        &release,
    );
    let request_path = fixture.root.path().join("retry.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&RetryFixture {
            coordinator: CoordinatorFixture {
                stale_clients: false,
                socket: fixture.socket(),
                data: fixture.data.clone(),
                operation: operation.clone(),
                release,
            },
            server_binary: fixture.binary.clone(),
            client_binary: client_install.binary.clone(),
            client_data: client_install.data.clone(),
            client,
            disconnected_before_attempt,
        })
        .unwrap(),
    )
    .unwrap();
    let log_path = fixture.root.path().join("retry.log");
    let log = File::create(&log_path).unwrap();
    let environment = fixture.command();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    for (name, value) in environment.get_envs() {
        if let Some(value) = value {
            command.env(name, value);
        }
    }
    command
        .current_dir(environment.get_current_dir().unwrap())
        .env("YINZ_TEST_UPDATE_RETRY", request_path)
        .args([
            "--exact",
            "live_update::tests::client_update_real::retry::failed_readiness_worker",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    let mut worker = command.spawn().unwrap();
    let completed = Watched::new(None);
    let status = std::thread::scope(|scope| {
        let completed = &completed;
        let worker = &mut worker;
        scope.spawn(move || {
            // Child::wait has no timeout and holds the child needed for cleanup.
            // Keep bounded try_wait polling so a stuck worker can be killed and
            // reaped without leaving a scoped waiter blocked indefinitely.
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                match worker.try_wait() {
                    Ok(Some(status)) => {
                        *completed.lock() = Some(Ok(status));
                        return;
                    },
                    Ok(None) => {},
                    Err(error) => {
                        let killed = worker.kill();
                        let reaped = worker.wait();
                        *completed.lock() = Some(Err(format!(
                            "failed to observe worker: {error}; kill: {killed:?}; reap: {reaped:?}"
                        )));
                        return;
                    },
                }
                if Instant::now() >= deadline {
                    let killed = worker.kill();
                    let reaped = worker.wait();
                    *completed.lock() = Some(Err(format!(
                        "timed out after 45s waiting for failed client readiness and recovery retry; kill: {killed:?}; reap: {reaped:?}"
                    )));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        completed.wait_until_described(
            "failed client readiness and recovery retry",
            Duration::from_secs(50),
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
    assert_eq!(attached.child.try_wait().unwrap().unwrap().code(), Some(97));
    let mut destination = Peer::connect(&fixture.socket());
    let after = destination.inventory(&operation);
    assert_ne!(before.server_pid, after.server_pid);
    assert!(after.clients.iter().all(|client| client.resumed.is_none()));
    // Retaining backups must not freeze the adopted terminals or make the new
    // public endpoint unusable while the missing client needs manual recovery.
    let mut recovered = Attached::start(&fixture);
    recovered.wait_for("READY-first");
    recovered
        .master
        .write_all(b"after-failed-client-readiness\n")
        .unwrap();
    recovered.wait_for("ECHO-first:after-failed-client-readiness");
    let wrong = destination
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let original_id = client.get().to_string();
    let wrong_id = wrong.get().to_string();
    let refusal = fixture
        .command()
        .args([
            "update",
            "--resume-client",
            &original_id,
            "--client",
            &wrong_id,
            "--session",
            SESSION,
        ])
        .output()
        .unwrap();
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("different host or installation"),
        "{refusal:?}"
    );
    assert_eq!(
        Transaction::open(&fixture.binary, &fixture.data, &operation)
            .unwrap()
            .status(),
        crate::update::transaction::Status::Published
    );

    // Repair the deliberately broken candidate in this isolated installation,
    // then attach it normally. A manual attachment alone cannot prove readiness.
    fs::copy(&fixture.binary, &client_install.binary).unwrap();
    let mut command = client_install.command();
    command
        .env("YINZ_SOCKET_DIR", fixture.root.path().join("sockets"))
        .args(["attach", SESSION]);
    let mut manual = Attached::start_command(command);
    manual.wait_for("READY-first");
    let current = destination
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update && client.id != wrong)
        .unwrap();
    assert!(current.resumed.is_none());
    let current_id = current.id.get().to_string();
    let process = manual.child.id();
    let result = fixture
        .command()
        .args([
            "update",
            "--resume-client",
            &original_id,
            "--client",
            &current_id,
            "--session",
            SESSION,
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(manual.child.id(), process);
    let ready = destination
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|entry| entry.resumed == Some((operation.clone(), client)))
        .unwrap();
    assert_ne!(
        ready.id, current.id,
        "recovery must actually reconnect after exec"
    );
    assert!(manual.child.try_wait().unwrap().is_none());
    let journal: crate::live_update::journal::Journal = serde_json::from_slice(
        &fs::read(
            fixture
                .data
                .join("updates")
                .join(&operation)
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal.phase, Phase::Complete);
    assert!(journal.restart_required.is_empty());
    assert_eq!(journal.restarted_clients, vec![client]);
    for (binary, data) in [
        (&fixture.binary, &fixture.data),
        (&client_install.binary, &client_install.data),
    ] {
        assert_eq!(
            Transaction::open(binary, data, &operation)
                .unwrap()
                .status(),
            crate::update::transaction::Status::Committed
        );
    }
    // Retrying the exact CLI invocation after its acknowledgement was lost is
    // harmless even though CURRENT_ID belonged to the pre-exec attachment.
    let repeated = fixture
        .command()
        .args([
            "update",
            "--resume-client",
            &original_id,
            "--client",
            &current_id,
            "--session",
            SESSION,
        ])
        .output()
        .unwrap();
    assert!(repeated.status.success(), "{repeated:?}");
    assert!(destination
        .inventory(&operation)
        .clients
        .iter()
        .any(|entry| entry.id == ready.id));
    manual
        .master
        .write_all(b"after-explicit-client-recovery\n")
        .unwrap();
    manual.wait_for("ECHO-first:after-explicit-client-recovery");
}
