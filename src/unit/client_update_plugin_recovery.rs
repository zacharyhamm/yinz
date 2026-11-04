use super::*;
use crate::live_update::{
    journal::{Phase, Store},
    transport::Connection,
};

const BROKEN: &str = "(define (load config) (error \"UPDATE_PLUGIN_REPAIR_REQUIRED\"))";

#[derive(serde::Serialize, serde::Deserialize)]
struct PluginRecoveryFixture {
    coordinator: CoordinatorFixture,
    initial: bool,
}

#[test]
#[ignore = "subprocess fixture with an isolated session environment"]
fn plugin_recovery_worker() {
    let request: PluginRecoveryFixture = serde_json::from_slice(
        &fs::read(std::env::var_os("YINZ_TEST_UPDATE_PLUGIN_RECOVERY").unwrap()).unwrap(),
    )
    .unwrap();
    let request_ref = &request.coordinator;
    if request.initial {
        let mut store = Store::open(
            &request_ref.socket,
            &request_ref.data,
            &request_ref.operation,
        )
        .unwrap();
        let mut source = Connection::connect(&request_ref.socket).unwrap();
        let mut inventory: Inventory = serde_json::from_value(
            source
                .request(&request_ref.operation, SessionCommand::Inventory)
                .unwrap(),
        )
        .unwrap();
        inventory.running_version = "0.0.0".into();
        let error = crate::live_update::coordinator::execute_release(
            &mut store,
            &mut source,
            inventory,
            request_ref.release.clone(),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("UPDATE_PLUGIN_REPAIR_REQUIRED"),
            "{error:#}"
        );
        assert_eq!(store.journal.phase, Phase::RecoveryRequired);
        assert!(store.journal.handoff_committed);
        assert_eq!(
            store.journal.restarted_clients.len(),
            2,
            "clients must reach the repairable replacement despite plugin failure"
        );
    } else {
        crate::live_update::coordinator::run(
            &request_ref.socket,
            &request_ref.data,
            &request_ref.operation,
        )
        .unwrap();
    }
}

fn run_worker(fixture: &Fixture, operation: &str, release: &crate::update::Release, initial: bool) {
    let suffix = if initial { "failed" } else { "repaired" };
    let request_path = fixture
        .root
        .path()
        .join(format!("plugin-recovery-{suffix}.json"));
    fs::write(
        &request_path,
        serde_json::to_vec(&PluginRecoveryFixture {
            coordinator: CoordinatorFixture {
                socket: fixture.socket(),
                data: fixture.data.clone(),
                operation: operation.into(),
                release: release.clone(),
            },
            initial,
        })
        .unwrap(),
    )
    .unwrap();
    let log_path = fixture
        .root
        .path()
        .join(format!("plugin-recovery-{suffix}.log"));
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
        .env("YINZ_TEST_UPDATE_PLUGIN_RECOVERY", request_path)
        .args([
            "--exact",
            "live_update::tests::client_update_real::plugin_recovery::plugin_recovery_worker",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    let mut worker = command.spawn().unwrap();
    let completed = Watched::new(None);
    std::thread::scope(|scope| {
        let completed = &completed;
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(40);
            while Instant::now() < deadline {
                if let Some(status) = worker.try_wait().unwrap() {
                    *completed.lock() = Some(status);
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
            worker.kill().unwrap();
            *completed.lock() = Some(worker.wait().unwrap());
        });
        completed.wait_until(
            "plugin update recovery coordinator",
            Duration::from_secs(45),
            |status| status.map(|_| ()),
        );
    });
    assert!(
        completed.lock().unwrap().success(),
        "coordinator: {}\njournal: {}",
        fs::read_to_string(log_path).unwrap(),
        fs::read_to_string(
            fixture
                .data
                .join("updates")
                .join(operation)
                .join("journal.json")
        )
        .unwrap_or_default()
    );
}

fn wait_for_plugin_log(fixture: &Fixture, marker: &str) {
    let observed = Watched::new(String::new());
    std::thread::scope(|scope| {
        let observed = &observed;
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                let list: serde_json::Value = serde_json::from_slice(&fixture.success(&[
                    "--session",
                    SESSION,
                    "plugins",
                    "list",
                    "--running",
                    "--json",
                ]))
                .unwrap();
                let plugin = list["plugins"].as_array().unwrap().iter().find(|plugin| {
                    plugin["location"]
                        .as_str()
                        .is_some_and(|location| location.ends_with("repair-fixture.scm"))
                });
                if let Some(plugin) = plugin {
                    let id = format!("plugin_{}", plugin["id"].as_u64().unwrap());
                    let log: serde_json::Value = serde_json::from_slice(&fixture.success(&[
                        "--session",
                        SESSION,
                        "plugins",
                        "log",
                        &id,
                        "--json",
                    ]))
                    .unwrap();
                    let text = log["output"].as_str().unwrap().to_owned();
                    let found = text.contains(marker);
                    *observed.lock() = text;
                    if found {
                        return;
                    }
                }
            }
        });
        observed.wait_until_described(
            marker,
            Duration::from_secs(20),
            |log| log.contains(marker).then_some(()),
            Clone::clone,
        );
    });
}

#[test]
fn plugin_readiness_failure_reconnects_clients_for_interactive_repair_and_retry() {
    let fixture = Fixture::new();
    let plugin = fixture.root.path().join("repair-fixture.scm");
    fs::write(
        &plugin,
        "(define (load config) (yinz-log \"SOURCE_PLUGIN_READY\"))",
    )
    .unwrap();
    let location = url::Url::from_file_path(&plugin).unwrap().to_string();
    fs::write(fixture.root.path().join("config.kdl"), format!("detachable_panes true\nmirror_session false\ndefault_mode \"locked\"\nplugins {{ repair-fixture location={location:?}; }}\nload_plugins clear-defaults=true {{ repair-fixture; }}\n")).unwrap();
    fixture.start();
    let mut first = Attached::start(&fixture);
    first.wait_for("READY-first");
    wait_for_plugin_log(&fixture, "SOURCE_PLUGIN_READY");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&fixture.socket());
    let before = source.inventory(&operation);
    let first_id = before
        .clients
        .iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let mut second = Attached::start(&fixture);
    second.wait_for("READY-first");
    let second_id = source
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update && client.id != first_id)
        .unwrap()
        .id;
    source
        .sender
        .send_client_msg(ClientToServerMsg::Action {
            action: yinz_types::input::actions::Action::GoToTab {
                index: TabNumber::new(2),
            },
            terminal_id: None,
            client_id: Some(second_id),
            is_cli_client: true,
        })
        .unwrap();
    second.wait_for("READY-second");
    let command_pids: Vec<i32> = ["first-input.pid", "second-input.pid"]
        .iter()
        .map(|name| {
            fs::read_to_string(fixture.root.path().join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        })
        .collect();
    fixture.prepare(&operation);
    let release = crate::update::Release {
        channel: yinz_types::session_update::ReleaseChannel::Prerelease,
        source: None,
        tag_name: yinz_runtime::consts::VERSION.into(),
        assets: vec![],
    };
    super::retry::receipt(&fixture.data, &fixture.binary, &operation, &release);
    // The source already has valid running code. Only replacement startup reads
    // the changed file, so the failure occurs after terminal ownership commits.
    fs::write(&plugin, BROKEN).unwrap();
    run_worker(&fixture, &operation, &release, true);
    let mut destination = Peer::connect(&fixture.socket());
    let failed = destination.inventory(&operation);
    assert_ne!(failed.server_pid, before.server_pid);
    for original in [first_id, second_id] {
        assert!(failed
            .clients
            .iter()
            .any(|client| client.resumed == Some((operation.clone(), original))));
    }
    assert_eq!(
        Transaction::open(&fixture.binary, &fixture.data, &operation)
            .unwrap()
            .status(),
        crate::update::transaction::Status::Published
    );
    first
        .master
        .write_all(b"while-plugin-broken-first\n")
        .unwrap();
    second
        .master
        .write_all(b"while-plugin-broken-second\n")
        .unwrap();
    first.wait_for("ECHO-first:while-plugin-broken-first");
    second.wait_for("ECHO-second:while-plugin-broken-second");
    for pid in &command_pids {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None).unwrap();
    }
    // Use the public live reload command while the update remains incomplete.
    fs::write(
        &plugin,
        "(define (load config) (yinz-log \"PLUGIN_REPAIRED\"))",
    )
    .unwrap();
    fixture.success(&[
        "--session",
        SESSION,
        "action",
        "start-or-reload-plugin",
        &location,
    ]);
    wait_for_plugin_log(&fixture, "PLUGIN_REPAIRED");
    run_worker(&fixture, &operation, &release, false);
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
    assert_eq!(journal.restarted_clients.len(), 2);
    assert!(journal.restart_required.is_empty());
    assert_eq!(
        Transaction::open(&fixture.binary, &fixture.data, &operation)
            .unwrap()
            .status(),
        crate::update::transaction::Status::Committed
    );
    for pid in command_pids {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).unwrap();
    }
    first.master.write_all(b"after-plugin-repair\n").unwrap();
    first.wait_for("ECHO-first:after-plugin-repair");
}
