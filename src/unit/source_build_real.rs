//! Real command-pane lifecycle around a stub compiler, using the production build driver.
use super::*;
use crate::live_update::{journal::Store, source_build, transport::Connection};

#[derive(serde::Serialize, serde::Deserialize)]
struct BuildFixture {
    socket: PathBuf,
    data: PathBuf,
    operation: String,
    result: PathBuf,
}

#[test]
#[ignore = "subprocess build driver with isolated session environment"]
fn source_build_test_worker() {
    let request: BuildFixture = serde_json::from_slice(
        &fs::read(std::env::var_os("YINZ_TEST_SOURCE_BUILD").unwrap()).unwrap(),
    )
    .unwrap();
    let store = Store::open(&request.socket, &request.data, &request.operation).unwrap();
    let mut connection = Connection::connect(&request.socket).unwrap();
    let inventory = serde_json::from_value(
        connection
            .request(&request.operation, SessionCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    let result = source_build::build(&store, &inventory, &["x86_64-unknown-linux-gnu".into()])
        .map(|record| serde_json::to_value(record).unwrap())
        .map_err(|error| format!("{error:#}"));
    fs::write(request.result, serde_json::to_vec(&result).unwrap()).unwrap();
}

fn wait_worker(child: &mut Child, log: &Path) {
    let completed = Watched::new(None);
    let status = std::thread::scope(|scope| {
        let completed = &completed;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    *completed.lock() = Some(status);
                    return;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    *completed.lock() = Some(child.wait().unwrap());
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(25));
            }
        });
        completed.wait_until(
            "source build driver completes",
            TIMEOUT + Duration::from_secs(5),
            |status| *status,
        )
    });
    assert!(status.success(), "{}", fs::read_to_string(log).unwrap());
}

fn build_in_real_pane(mode: &str) {
    let fixture = Fixture::new();
    let tools = fixture.root.path().join("build-tools");
    fs::create_dir(&tools).unwrap();
    for tool in ["cargo-zigbuild", "zig", "protoc", "file", "readelf"] {
        std::os::unix::fs::symlink("/usr/bin/true", tools.join(tool)).unwrap();
    }
    yinz_test_support::write_warm_script(
        &tools.join("rustup"),
        "#!/bin/sh\nprintf '%s\\n' x86_64-unknown-linux-gnu\n",
    );
    yinz_test_support::write_warm_script(
        &tools.join("cargo"),
        if mode == "success" {
            "#!/bin/sh\n[ \"$1\" = x ] || exit 0\nmkdir -p dist\nprintf 'built-artifact\\n' > \"dist/yinz-$3.tar.gz\"\nprintf 'BUILD-COMPLETE\\n'\n"
        } else if mode == "failure" {
            "#!/bin/sh\n[ \"$1\" = x ] || exit 0\nprintf 'COMPILER-FAILED\\n'\nexit 7\n"
        } else {
            "#!/bin/sh\n[ \"$1\" = x ] || exit 0\nprintf '%s\\n' \"$PPID\" > worker.pid\nsleep 60 &\nprintf '%s\\n' \"$!\" > running.pid\nwait\n"
        },
    );
    let build_path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    fs::write(
        fixture.root.path().join("config.kdl"),
        format!(
            "detachable_panes false\nload_plugins clear-defaults=true\nenv {{ PATH {}; }}\n",
            serde_json::to_string(&build_path).unwrap(),
        ),
    )
    .unwrap();
    fixture.start();
    let operation = uuid::Uuid::new_v4().to_string();
    let directory = fixture.data.join("updates").join(&operation).join("source");
    let snapshot = directory.join("checkout");
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("Cargo.lock"), "pinned dependency lock\n").unwrap();
    // The snapshot phase is covered separately with real Git. Seed the pinned
    // record here so this test isolates the actual pane and build supervision.
    fs::write(
        directory.join("build.json"),
        serde_json::to_vec(&serde_json::json!({
            "operation": operation, "repository": fixture.root.path(), "commit": "0123456789abcdef",
            "version": yinz_runtime::consts::VERSION, "snapshot": snapshot, "targets": [],
            "log": directory.join("build.log"), "artifacts": {}, "sha256": {}, "pane": null,
            "snapshot_ready": true,
        }))
        .unwrap(),
    )
    .unwrap();
    let request_path = fixture.root.path().join("build-request.json");
    let result_path = fixture.root.path().join("build-result.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&BuildFixture {
            socket: fixture.socket(),
            data: fixture.data.clone(),
            operation,
            result: result_path.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    let log_path = fixture.root.path().join("build-driver.log");
    let log = File::create(&log_path).unwrap();
    let environment = fixture.command();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    for (name, value) in environment.get_envs() {
        if let Some(value) = value {
            command.env(name, value);
        }
    }
    let mut child = command
        .current_dir(environment.get_current_dir().unwrap())
        .env("YINZ_TEST_SOURCE_BUILD", &request_path)
        .args([
            "--exact",
            "live_update::tests::client_update_real::source_build_real::source_build_test_worker",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    if matches!(mode, "cancel" | "closed" | "killed") {
        let started = Watched::new(None);
        let pane = std::thread::scope(|scope| {
            let started = &started;
            let directory = &directory;
            scope.spawn(move || {
                let deadline = Instant::now() + TIMEOUT;
                while Instant::now() < deadline {
                    if directory.join("checkout/running.pid").exists()
                        && directory.join("process.json").exists()
                        && let Ok(bytes) = fs::read(directory.join("build.json"))
                    {
                        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        if let Some(pane) = value["pane"].as_str() {
                            *started.lock() = Some(pane.to_owned());
                            return;
                        }
                    }
                    std::thread::park_timeout(Duration::from_millis(25));
                }
            });
            started.wait_until(
                "source compiler running in its recorded pane",
                TIMEOUT,
                Clone::clone,
            )
        });
        if mode == "cancel" {
            fs::write(directory.parent().unwrap().join("cancel"), "").unwrap();
        } else if mode == "killed" {
            let pid: i32 = fs::read_to_string(snapshot.join("worker.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            )
            .unwrap();
        } else {
            fixture.success(&[
                "--session",
                SESSION,
                "action",
                "close-pane",
                "--pane-id",
                &pane,
            ]);
        }
    }
    wait_worker(&mut child, &log_path);
    let result: Result<serde_json::Value, String> =
        serde_json::from_slice(&fs::read(result_path).unwrap()).unwrap();
    let panes: Vec<PaneInfo> = serde_json::from_slice(&fixture.success(&[
        "--session",
        SESSION,
        "action",
        "list-panes",
        "--json",
    ]))
    .unwrap();
    if mode == "success" {
        let record = result.unwrap();
        assert_eq!(record["pane_closed"], true);
        assert_eq!(
            panes.len(),
            2,
            "the successful build pane must close before migration"
        );
        assert!(fs::read_to_string(directory.join("build.log"))
            .unwrap()
            .contains("BUILD-COMPLETE"));
    } else if mode == "failure" {
        assert!(result.unwrap_err().contains("source build failed"));
        assert_eq!(panes.len(), 3, "the failed build pane must remain visible");
        assert!(fs::read_to_string(directory.join("build.log"))
            .unwrap()
            .contains("COMPILER-FAILED"));
    } else {
        let error = result.unwrap_err();
        assert!(
            error.contains(if mode == "cancel" {
                "cancelled"
            } else if mode == "killed" {
                "worker exited without a result"
            } else {
                "was closed"
            }),
            "{error}"
        );
        assert_eq!(panes.len(), if mode == "closed" { 2 } else { 3 });
        if mode == "killed" {
            assert!(!directory.join("result.json").exists());
            assert!(directory.join("build.log").is_file());
            assert!(panes.iter().any(|pane| pane.exited));
        }
        let compiler: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("process.json")).unwrap()).unwrap();
        let child: u32 = fs::read_to_string(snapshot.join("running.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let stopped = Watched::new(false);
        std::thread::scope(|scope| {
            let stopped = &stopped;
            scope.spawn(move || {
                let deadline = Instant::now() + TIMEOUT;
                while Instant::now() < deadline {
                    if [compiler["pid"].as_u64().unwrap() as u32, child]
                        .iter()
                        .all(
                            |pid| match fs::read_to_string(format!("/proc/{pid}/stat")) {
                                Ok(stat) => {
                                    stat.rsplit_once(')').unwrap().1.split_whitespace().next()
                                        == Some("Z")
                                },
                                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                            },
                        )
                    {
                        *stopped.lock() = true;
                        return;
                    }
                    std::thread::park_timeout(Duration::from_millis(25));
                }
            });
            stopped.wait_until(
                "compiler and child stop after build interruption",
                TIMEOUT,
                |done| done.then_some(()),
            );
        });
    }
}

#[test]
fn source_build_success_closes_real_pane_before_migration() {
    build_in_real_pane("success");
}

#[test]
fn source_build_failure_retains_real_pane_and_log() {
    build_in_real_pane("failure");
}

#[test]
fn source_build_cancellation_retains_real_pane_without_installation() {
    build_in_real_pane("cancel");
}

#[test]
fn source_build_closed_pane_stops_without_installation() {
    build_in_real_pane("closed");
}

#[test]
fn source_build_killed_worker_stops_compiler_and_retains_pane_and_log() {
    build_in_real_pane("killed");
}
