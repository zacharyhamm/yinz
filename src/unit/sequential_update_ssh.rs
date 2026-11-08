//! Two independent sessions, one client installation, and real SSH. Run with:
//! cargo x remote-test --build --update-fixtures
//! cargo x remote-test --test -- --exact live_update::tests::client_update_real::sequential_ssh::local_then_remote
//! (or remote_then_local). No GitHub discovery, user installation, or live session is used.
use super::*;
use crate::live_update::journal::{Journal, Phase, Store};
use sha2::{Digest, Sha256};
use yinz_types::session_update::ReleaseChannel;

const HOST: &str = "yinz-remote-pane";
const WORKER: &str = "live_update::tests::client_update_real::sequential_ssh::prepare_worker";
const OLD_PLUGIN: &str = "(define (load config) (yinz-log \"SEQUENTIAL_BUILTIN_OLD\"))";
const NEW_PLUGIN: &str = "(define (load config) (yinz-log \"SEQUENTIAL_BUILTIN_NEW\"))";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
fn docker(args: &[&str]) -> Vec<u8> {
    let result = Command::new("docker")
        .args([
            "compose",
            "-p",
            "yinz-remote-panes",
            "-f",
            "docker-compose.remote-panes.yml",
            "exec",
            "-T",
            "--user",
            "test",
            "remote",
        ])
        .args(args)
        .current_dir(repo())
        .output()
        .unwrap();
    assert!(result.status.success(), "docker {args:?}: {result:?}");
    result.stdout
}
fn remote(script: &str) -> String {
    String::from_utf8(docker(&["sh", "-ec", script])).unwrap()
}
fn write_remote(path: &Path, text: &str) {
    remote(&format!(
        "mkdir -p {}; printf %s {} > {}",
        quote(path.parent().unwrap().to_str().unwrap()),
        quote(text),
        quote(path.to_str().unwrap())
    ));
}
fn mounted(path: &Path) -> PathBuf {
    Path::new("/usr/src/yinz").join(path.strip_prefix(repo().join("target")).unwrap())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Preparation {
    executable: PathBuf,
    data: PathBuf,
    candidate: PathBuf,
    socket: Option<PathBuf>,
    operation: String,
    release: crate::update::Release,
}
impl Preparation {
    fn run(&self) {
        let output = Command::new(self.candidate.join("yinz"))
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("yinz {}", self.release.tag_name)
        );
        crate::live_update::validate_capabilities(&self.candidate.join("yinz")).unwrap();
        // The fixture uses exactly the durable transaction and preparation receipt
        // that a checksum-verified release download leaves behind. Everything
        // after discovery, including version inventories, runs in production.
        drop(
            Transaction::prepare(
                &self.candidate,
                &self.executable,
                &self.data,
                &self.operation,
            )
            .unwrap(),
        );
        let receipts = self
            .data
            .join("updates")
            .join(&self.operation)
            .join("participants");
        fs::create_dir_all(&receipts).unwrap();
        let executable = self.executable.canonicalize().unwrap();
        let key = crate::update::hex(&Sha256::digest(executable.as_os_str().as_encoded_bytes()));
        fs::write(
            receipts.join(format!("{key}.json")),
            serde_json::to_vec(&self.release).unwrap(),
        )
        .unwrap();
        if let Some(socket) = &self.socket {
            let mut store = Store::open_with_channel(
                socket,
                &self.data,
                &self.operation,
                false,
                ReleaseChannel::Prerelease,
            )
            .unwrap();
            store.journal.release = Some(serde_json::to_value(&self.release).unwrap());
            store.save().unwrap();
        }
    }
}
#[test]
#[ignore = "remote subprocess preparation helper, invoked only by sequential SSH fixtures"]
fn prepare_worker() {
    let request: Preparation = serde_json::from_slice(
        &fs::read(std::env::var_os("YINZ_SEQUENTIAL_PREPARATION").unwrap()).unwrap(),
    )
    .unwrap();
    request.run();
}

struct Sessions {
    local: Fixture,
    far: PathBuf,
    remote_socket: PathBuf,
    candidate: PathBuf,
    version: String,
    _installation: tempfile::TempDir,
}
impl Sessions {
    fn new() -> Self {
        // Executable generations and runtime logs live on disk. Keep the
        // socket/home root short even when the worktree has a long absolute path.
        let installation = tempfile::Builder::new()
            .prefix("seq-install-")
            .tempdir_in(repo().join("target/remote-test"))
            .unwrap();
        let local = Fixture::new_in(Path::new("/var/tmp"), Some(installation.path()));
        let far = PathBuf::from(format!(
            "/tmp/ys-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        let candidate = installation.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        let metadata: serde_json::Value = serde_json::from_slice(
            &fs::read(repo().join("target/remote-test/update-fixtures.json"))
                .expect("run cargo x remote-test --build --update-fixtures"),
        )
        .unwrap();
        let newer = PathBuf::from(metadata["binary"].as_str().unwrap());
        assert!(
            newer.is_file(),
            "run cargo x remote-test --build --update-fixtures"
        );
        fs::copy(&newer, candidate.join("yinz")).unwrap();
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &candidate).unwrap();
        fs::write(
            local.data.join("plugins/builtins/session-palette.scm"),
            OLD_PLUGIN,
        )
        .unwrap();
        fs::write(
            candidate.join("plugins/builtins/session-palette.scm"),
            NEW_PLUGIN,
        )
        .unwrap();
        let version = String::from_utf8(
            Command::new(&newer)
                .arg("--version")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let version = version.trim().strip_prefix("yinz ").unwrap().to_owned();
        assert!(
            crate::update::version(&version).unwrap()
                > crate::update::version(yinz_runtime::consts::VERSION).unwrap()
        );
        assert_ne!(
            crate::live_update::artifacts::digest_file(&local.binary).unwrap(),
            crate::live_update::artifacts::digest_file(&newer).unwrap()
        );
        fs::write(
            local.root.path().join("config.kdl"),
            "detachable_panes true\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n",
        )
        .unwrap();
        remote(&format!("mkdir -p {f}/home {f}/tmp {f}/sockets {f}/cache {f}/config {f}/runtime {f}/data {f}/bin {f}/candidate; cp {} {f}/yinz; cp {} {f}/candidate/yinz; cp -R /usr/src/yinz/remote-test/data/plugins {f}/data/; cp -R {f}/data/plugins {f}/candidate/", quote("/usr/src/yinz/remote-test/yinz"), quote(mounted(&newer).to_str().unwrap()), f=far.display()));
        write_remote(
            &far.join("config.kdl"),
            "detachable_panes true\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n",
        );
        write_remote(
            &far.join("data/plugins/builtins/session-palette.scm"),
            OLD_PLUGIN,
        );
        write_remote(
            &far.join("candidate/plugins/builtins/session-palette.scm"),
            NEW_PLUGIN,
        );
        let remote_environment = format!("HOME={f}/home TMPDIR={f}/tmp XDG_CONFIG_HOME={f}/config XDG_DATA_HOME={f}/data XDG_CACHE_HOME={f}/cache XDG_RUNTIME_DIR={f}/runtime YINZ_TEST_CACHE_DIR={f}/cache YINZ_SOCKET_DIR={f}/sockets", f=far.display());
        write_remote(&far.join("bin/yinz"), &format!("#!/bin/sh\nexec env {remote_environment} {f}/yinz --config {f}/config.kdl --data-dir {f}/data \"$@\"\n", f=far.display()));
        remote(&format!("chmod +x {}/bin/yinz", far.display()));
        let ssh_bin = local.root.path().join("ssh-bin");
        fs::create_dir(&ssh_bin).unwrap();
        // Only bootstrap commands get a disposable remote environment. Forwarding
        // arguments and the real ssh transport are passed through unchanged.
        yinz_test_support::write_warm_script(&ssh_bin.join("ssh"), &format!("#!/bin/sh\nfor arg do\n shift\n case \"$arg\" in \"'yinz' \"*|\"yinz \"*) arg={}\"$arg\" ;; esac\n set -- \"$@\" \"$arg\"\ndone\nexec /usr/bin/ssh -F {} \"$@\"\n", quote(&format!("env {remote_environment} PATH={}/bin:/usr/local/bin:/usr/bin:/bin ", far.display())), quote(repo().join("target/remote-test/ssh_config").to_str().unwrap())));
        let local_path = format!("{}:{}", ssh_bin.display(), std::env::var("PATH").unwrap());
        // Both clients and the local server inherit this PATH through their config.
        let config = format!("detachable_panes true\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\nenv {{ PATH {}; }}\n", serde_json::to_string(&local_path).unwrap());
        fs::write(local.root.path().join("config.kdl"), config).unwrap();
        let remote_socket = far
            .join("sockets")
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join(SESSION);
        let fixture = Self {
            local,
            far,
            remote_socket,
            candidate,
            version,
            _installation: installation,
        };
        fixture.start_session(false);
        fixture.start_session(true);
        fixture
    }
    fn command(&self, far: bool, args: &[&str]) -> Vec<u8> {
        if far {
            let command = args
                .iter()
                .map(|arg| quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            remote(&format!("{}/bin/yinz {command}", self.far.display())).into_bytes()
        } else {
            self.local.success(args)
        }
    }
    fn root(&self, far: bool) -> &Path {
        if far {
            &self.far
        } else {
            self.local.root.path()
        }
    }
    fn start_session(&self, far: bool) {
        let root = self.root(far);
        let script = format!("echo $$ > {}/pane.pid; n=0; trap 'n=$((n+1)); printf \"REPLAY-%s\\n\" \"$n\"' USR1; printf 'READY-{}\\n'; while :; do IFS= read -r line || continue; printf '%s\\n' \"$line\" >> {}/input; printf 'ECHO-{}:%s\\n' \"$line\"; done", root.display(), if far {"REMOTE"} else {"LOCAL"}, root.display(), if far {"REMOTE"} else {"LOCAL"});
        let layout = format!(
            "layout {{ pane command=\"/bin/sh\" {{ args \"-c\" {}; }}; }}",
            serde_json::to_string(&script).unwrap()
        );
        self.command(
            far,
            &[
                "--layout-string",
                &layout,
                "attach",
                "--create-background",
                SESSION,
            ],
        );
    }
    fn attach(&self, far: bool, marker: &str) -> Attached {
        let original = self.local.command();
        let mut command = Command::new(&self.local.binary);
        command.env_clear().envs(
            original
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        );
        command.current_dir(original.get_current_dir().unwrap());
        let mut args = original.get_args();
        while let Some(arg) = args.next() {
            if far && arg == "--data-dir" {
                args.next();
            } else {
                command.arg(arg);
            }
        }
        command.env("XDG_DATA_HOME", self.local.data.parent().unwrap());
        // Config env also reaches bootstrap; setting PATH here covers discovery
        // before effective session configuration has been received.
        command.env(
            "PATH",
            format!(
                "{}:{}",
                self.local.root.path().join("ssh-bin").display(),
                std::env::var("PATH").unwrap()
            ),
        );
        if far {
            command.args(["--remote", HOST]);
        }
        command.args(["attach", SESSION]);
        let client = Attached::start_command(command);
        client.screen.wait_until_described(
            marker,
            TIMEOUT,
            |screen| {
                assert!(
                    !yinz_process::liveness::has_exited(client.child.id()).unwrap(),
                    "client exited before {marker}: {}",
                    String::from_utf8_lossy(screen)
                );
                String::from_utf8_lossy(screen)
                    .contains(marker)
                    .then_some(())
            },
            |screen| String::from_utf8_lossy(screen).into_owned(),
        );
        client
    }
    fn peer(&self, far: bool) -> (Peer, Option<yinz_process::ssh::HostForward>) {
        let forward = far.then(|| {
            let socket = self
                .local
                .root
                .path()
                .join(format!("p-{}.sock", uuid::Uuid::new_v4().simple()));
            // Do not mutate the harness's process-wide PATH.
            let env = yinz_types::envs::EnvironmentVariables::from_data(
                std::collections::HashMap::from([(
                    "PATH".into(),
                    format!(
                        "{}:{}",
                        self.local.root.path().join("ssh-bin").display(),
                        std::env::var("PATH").unwrap()
                    ),
                )]),
            );
            yinz_process::ssh::HostForward::start_until(
                HOST,
                &Default::default(),
                &socket,
                self.remote_socket.to_str().unwrap(),
                None,
                Instant::now() + TIMEOUT,
                &env,
            )
            .unwrap()
        });
        let local_socket = self.local.socket();
        let socket = forward
            .as_ref()
            .map(|f| f.socket())
            .unwrap_or(&local_socket);
        (Peer::connect(socket), forward)
    }
    fn read(&self, far: bool, path: &Path) -> String {
        if far {
            remote(&format!("cat {}", quote(path.to_str().unwrap())))
        } else {
            fs::read_to_string(path).unwrap()
        }
    }
    fn pid(&self, far: bool) -> i32 {
        self.read(far, &self.root(far).join("pane.pid"))
            .trim()
            .parse()
            .unwrap()
    }
    fn alive(&self, far: bool, pid: i32) {
        assert_eq!(self.pid(far), pid);
        if far {
            remote(&format!("kill -0 {pid}"));
        } else {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).unwrap();
        }
    }
    fn signal(&self, far: bool, pid: i32) {
        if far {
            remote(&format!("kill -USR1 {pid}"));
        } else {
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGUSR1,
            )
            .unwrap();
        }
    }
    fn prepare(&self, far: bool, operation: &str) {
        let release = crate::update::Release {
            channel: ReleaseChannel::Prerelease,
            source: None,
            tag_name: self.version.clone(),
            assets: vec![],
        };
        let local = Preparation {
            executable: self.local.binary.clone(),
            data: self.local.data.clone(),
            candidate: self.candidate.clone(),
            socket: (!far).then(|| self.local.socket()),
            operation: operation.into(),
            release: release.clone(),
        };
        local.run();
        if far {
            let request = Preparation {
                executable: self.far.join("yinz"),
                data: self.far.join("data"),
                candidate: self.far.join("candidate"),
                socket: Some(self.remote_socket.clone()),
                operation: operation.into(),
                release,
            };
            let path = self.far.join("preparation.json");
            write_remote(&path, &serde_json::to_string(&request).unwrap());
            docker(&[
                "env",
                &format!("YINZ_SEQUENTIAL_PREPARATION={}", path.display()),
                mounted(&PathBuf::from(
                    serde_json::from_slice::<serde_json::Value>(
                        &fs::read(repo().join("target/remote-test/update-fixtures.json")).unwrap(),
                    )
                    .unwrap()["worker"]
                        .as_str()
                        .unwrap(),
                ))
                .to_str()
                .unwrap(),
                "--exact",
                WORKER,
                "--ignored",
                "--nocapture",
            ]);
        }
    }
    fn builtin_observation(&self) -> Option<String> {
        let list: serde_json::Value = serde_json::from_slice(&self.local.success(&[
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
                .is_some_and(|location| location.contains("session-palette"))
        })?;
        let id = format!("plugin_{}", plugin["id"].as_u64().unwrap());
        let log: serde_json::Value = serde_json::from_slice(&self.local.success(&[
            "--session",
            SESSION,
            "plugins",
            "log",
            &id,
            "--json",
        ]))
        .unwrap();
        assert_ne!(plugin["state"], "failed", "{plugin}");
        Some(log["output"].as_str().unwrap().to_owned())
    }
    fn coordinator_version(&self, far: bool, operation: &str) -> String {
        if far {
            let script = format!("import glob,subprocess\nfor path in glob.glob('/proc/[0-9]*/cmdline'):\n try:\n  args=open(path,'rb').read().split(b'\\0')\n  if {operation:?}.encode() in args and {arg:?}.encode() in args:\n   print(subprocess.check_output([path.replace('cmdline','exe'),'--version']).decode().strip())\n except (OSError, subprocess.CalledProcessError): pass", arg=yinz_types::session_update::COORDINATOR_ARG);
            let output = String::from_utf8(docker(&["python3", "-c", &script])).unwrap();
            output.trim().strip_prefix("yinz ").unwrap().to_owned()
        } else {
            let matches: Vec<_> = fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .filter_map(|entry| {
                    let args = fs::read(entry.path().join("cmdline")).ok()?;
                    let args: Vec<_> = args.split(|byte| *byte == 0).collect();
                    (args.contains(&operation.as_bytes())
                        && args.contains(&yinz_types::session_update::COORDINATOR_ARG.as_bytes()))
                    .then(|| entry.path().join("exe"))
                })
                .collect();
            assert_eq!(
                matches.len(),
                1,
                "one independent coordinator for {operation}"
            );
            installed_version(&matches[0])
        }
    }
    fn journal(&self, far: bool, operation: &str) -> Journal {
        let data = if far {
            self.far.join("data")
        } else {
            self.local.data.clone()
        };
        serde_json::from_str(&self.read(
            far,
            &data.join("updates").join(operation).join("journal.json"),
        ))
        .unwrap()
    }
}
impl Drop for Sessions {
    fn drop(&mut self) {
        // The entire far environment belongs to this test, including replacement
        // and retired endpoints after a failure. Never kill by the shared name.
        let script = format!("for s in {f}/sockets/cv*/*; do [ -S \"$s\" ] || continue; {f}/bin/yinz kill-session \"${{s##*/}}\" >/dev/null 2>&1 || true; done; rm -rf {f}", f=self.far.display());
        let _ = Command::new("docker")
            .args([
                "exec",
                "--user",
                "test",
                "yinz-remote-pane",
                "sh",
                "-c",
                &script,
            ])
            .output();
    }
}

// Poll real external state, publishing observations through Watched. Bounded
// polling is necessary for remote files and process state, which have no shared
// condition variable with this test process.
fn observe<T: Clone + Send + std::fmt::Debug>(
    what: &str,
    mut poll: impl FnMut() -> Option<T> + Send,
) -> T {
    let watched = Watched::new(None);
    std::thread::scope(|scope| {
        let watched = &watched;
        scope.spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let deadline = Instant::now() + TIMEOUT * 3;
                while Instant::now() < deadline {
                    if let Some(value) = poll() {
                        return value;
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                }
                panic!("external observation timed out");
            }))
            .map_err(|panic| {
                panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                    .unwrap_or_else(|| "observer panicked".into())
            });
            *watched.lock() = Some(result);
        });
        watched
            .wait_until_described(
                what,
                TIMEOUT * 3 + Duration::from_secs(1),
                Clone::clone,
                |state| format!("{state:?}"),
            )
            .unwrap_or_else(|error| panic!("{what}: {error}"))
    })
}

fn echo(client: &mut Attached, far: bool, marker: &str) {
    client
        .master
        .write_all(format!("{marker}\r").as_bytes())
        .unwrap();
    client.wait_for(&format!(
        "ECHO-{}:{marker}",
        if far { "REMOTE" } else { "LOCAL" }
    ));
}
fn forwarding_paths(fixture: &Fixture) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(fixture.root.path().join("sockets/client"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}
fn running_inode(pid: u32) -> u64 {
    fs::metadata(format!("/proc/{pid}/exe")).unwrap().ino()
}
fn installed_version(binary: &Path) -> String {
    let result = Command::new(binary).arg("--version").output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .strip_prefix("yinz ")
        .unwrap()
        .to_owned()
}

#[test]
#[ignore = "requires Docker SSH fixture and --build --update-fixtures"]
fn local_then_remote() {
    sequence(false);
}
#[test]
#[ignore = "requires Docker SSH fixture and --build --update-fixtures"]
fn remote_then_local() {
    sequence(true);
}

fn sequence(remote_first: bool) {
    let fixture = Sessions::new();
    let mut clients = [
        fixture.attach(false, "READY-LOCAL"),
        fixture.attach(true, "READY-REMOTE"),
    ];
    let client_pids = [clients[0].child.id(), clients[1].child.id()];
    let mut client_inodes = client_pids.map(running_inode);
    let mut replay = [0; 2];
    for index in 0..2 {
        assert_eq!(
            installed_version(Path::new(&format!("/proc/{}/exe", client_pids[index]))),
            yinz_runtime::consts::VERSION
        );
        replay[index] += 1;
        fixture.signal(index == 1, fixture.pid(index == 1));
        clients[index].wait_for("REPLAY-1");
    }
    let pids = [fixture.pid(false), fixture.pid(true)];
    let operation = uuid::Uuid::new_v4().to_string();
    let mut inventories = [false, true].map(|far| fixture.peer(far).0.inventory(&operation));
    assert_ne!(
        inventories[0].server_instance,
        inventories[1].server_instance
    );
    for inventory in &inventories {
        assert_eq!(inventory.running_version, yinz_runtime::consts::VERSION);
        assert_eq!(inventory.session, SESSION);
        assert_eq!(inventory.clients.len(), 1);
    }
    assert_eq!(inventories[0].executable, fixture.local.binary);
    echo(&mut clients[0], false, "BEFORE-LOCAL");
    echo(&mut clients[1], true, "BEFORE-REMOTE");
    for (stage, far) in [remote_first, !remote_first].into_iter().enumerate() {
        let target = usize::from(far);
        let other = 1 - target;
        let operation = uuid::Uuid::new_v4().to_string();
        let installed_before = fs::metadata(&fixture.local.binary).unwrap().ino();
        assert_eq!(
            installed_version(&fixture.local.binary),
            if stage == 0 {
                yinz_runtime::consts::VERSION
            } else {
                &fixture.version
            }
        );
        let forwards_before = forwarding_paths(&fixture.local);
        fixture.prepare(far, &operation);
        // Stop only the target client at inventory. This provides an observable
        // Prepare window, without slowing SSH, replacing the coordinator, or
        // guessing how many renders an update should produce.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(client_pids[target] as i32),
            nix::sys::signal::Signal::SIGSTOP,
        )
        .unwrap();
        let (mut source, _forward) = fixture.peer(far);
        let before = source.inventory(&operation);
        source
            .request(&operation, SessionCommand::StartPrerelease)
            .unwrap();
        observe("coordinator reached participant inventory", || {
            let journal = fixture.journal(far, &operation);
            assert!(
                !matches!(journal.phase, Phase::Failed | Phase::RecoveryRequired),
                "phase {:?}: {:?}",
                journal.phase,
                journal.errors
            );
            (journal.phase == Phase::Prepare).then_some(())
        });
        assert_eq!(
            fixture.coordinator_version(far, &operation),
            if !far && stage == 1 {
                &fixture.version
            } else {
                yinz_runtime::consts::VERSION
            }
        );
        echo(&mut clients[other], !far, &format!("IN-FLIGHT-{stage}"));
        let unchanged = fixture.peer(!far).0.inventory(&operation);
        assert_eq!(
            unchanged.server_instance,
            inventories[other].server_instance
        );
        assert_eq!(running_inode(client_pids[other]), client_inodes[other]);
        replay[target] += 1;
        fixture.signal(far, pids[target]);
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(client_pids[target] as i32),
            nix::sys::signal::Signal::SIGCONT,
        )
        .unwrap();
        let mut rounds = 0;
        let journal = observe("coordinator readiness and commit", || {
            let journal = fixture.journal(far, &operation);
            assert!(
                !matches!(journal.phase, Phase::Failed | Phase::RecoveryRequired),
                "phase {:?}: {:?}",
                journal.phase,
                journal.errors
            );
            if journal.phase == Phase::Complete {
                return Some(journal);
            }
            rounds += 1;
            echo(
                &mut clients[other],
                !far,
                &format!("DURING-{stage}-{rounds}"),
            );
            None
        });
        assert!(
            journal.handoff_committed,
            "already-current must not skip the running-old session"
        );
        assert_eq!(
            journal.source.as_ref().unwrap().running_version,
            yinz_runtime::consts::VERSION
        );
        assert!(journal.errors.is_empty(), "{journal:?}");
        assert_eq!(journal.restarted_clients, vec![before.clients[0].id]);
        assert!(journal.disconnected_clients.is_empty());
        assert!(
            journal.migration.as_ref().unwrap().remote_hosts.is_empty(),
            "these are separate sessions, with no remote panes"
        );
        assert!(journal.restart_required.is_empty());
        assert_eq!(journal.participants.len(), 2);
        assert!(journal.participants.iter().all(|p| p.state == "committed"));
        assert_eq!(installed_version(&fixture.local.binary), fixture.version);
        assert_ne!(
            fs::metadata(&fixture.local.binary).unwrap().ino(),
            installed_before
        );
        // The actual old client stamps the newer helper's inventory. Stage two
        // of local-first is the installed-new/running-old SSH client regression.
        let client_participant = journal
            .participants
            .iter()
            .find(|p| matches!(p.route, crate::live_update::journal::Route::Client(_)))
            .unwrap();
        assert_eq!(
            client_participant.installation.running_version,
            yinz_runtime::consts::VERSION
        );
        assert_eq!(
            client_participant.installation.installed_version,
            if stage == 0 {
                yinz_runtime::consts::VERSION
            } else {
                &fixture.version
            }
        );
        assert_eq!(
            client_participant.installation.executable,
            fixture.local.binary
        );
        assert_eq!(client_participant.installation.data_dir, fixture.local.data);
        assert_eq!(
            running_inode(client_pids[target]),
            fs::metadata(&fixture.local.binary).unwrap().ino()
        );
        assert_eq!(
            installed_version(Path::new(&format!("/proc/{}/exe", client_pids[target]))),
            fixture.version
        );
        client_inodes[target] = running_inode(client_pids[target]);
        assert_eq!(running_inode(client_pids[other]), client_inodes[other]);
        let after = fixture.peer(far).0.inventory(&operation);
        assert_ne!(after.server_instance, before.server_instance);
        assert_eq!(
            journal
                .replacement_identity
                .as_ref()
                .unwrap()
                .server_instance,
            after.server_instance
        );
        assert_eq!(after.running_version, fixture.version);
        assert_ne!(after.running_binary_sha256, before.running_binary_sha256);
        assert_eq!(
            after.running_binary_sha256.as_deref(),
            Some(
                crate::live_update::artifacts::digest_file(&fixture.candidate.join("yinz"))
                    .unwrap()
                    .as_str()
            )
        );
        assert_eq!(after.clients.len(), 1);
        assert_eq!(
            after.clients[0].resumed,
            Some((operation.clone(), before.clients[0].id))
        );
        let untouched = fixture.peer(!far).0.inventory(&operation);
        assert_eq!(
            untouched.running_binary_sha256,
            inventories[other].running_binary_sha256
        );
        assert_eq!(untouched.server_pid, inventories[other].server_pid);
        assert_eq!(
            untouched.server_instance,
            inventories[other].server_instance
        );
        assert_eq!(
            untouched.running_version,
            inventories[other].running_version
        );
        inventories[target] = after;
        clients[target].wait_for(&format!("REPLAY-{}", replay[target]));
        if far {
            assert_ne!(forwarding_paths(&fixture.local), forwards_before);
        } else {
            assert_eq!(forwarding_paths(&fixture.local), forwards_before);
        }
        for (index, client) in clients.iter_mut().enumerate() {
            fixture.alive(index == 1, pids[index]);
            replay[index] += 1;
            fixture.signal(index == 1, pids[index]);
            client.wait_for(&format!("REPLAY-{}", replay[index]));
            echo(client, index == 1, &format!("AFTER-{stage}-{index}"));
        }
        for (index, &replay_count) in replay.iter().enumerate() {
            let dump = String::from_utf8(fixture.command(
                index == 1,
                &[
                    "--session",
                    SESSION,
                    "action",
                    "dump-screen",
                    "--full",
                    "--pane-id",
                    "terminal_0",
                ],
            ))
            .unwrap();
            let actual: Vec<_> = dump
                .lines()
                .filter_map(|line| {
                    line.trim()
                        .strip_prefix("REPLAY-")
                        .and_then(|number| number.parse::<u32>().ok())
                })
                .collect();
            assert_eq!(
                actual,
                (1..=replay_count).collect::<Vec<_>>(),
                "ordered terminal replay: {dump}"
            );
        }
        let transaction =
            Transaction::open(&fixture.local.binary, &fixture.local.data, &operation).unwrap();
        assert_eq!(
            transaction.status(),
            crate::update::transaction::Status::Committed
        );
        drop(transaction); // Release the exclusive builtin lock before plugin loads.
        for remote_host in [false, far]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let receipt: serde_json::Value = serde_json::from_str(&fixture.read(
                remote_host,
                &if remote_host {
                    fixture.far.join(".yinz.yinz-update.json")
                } else {
                    fixture
                        .local
                        .binary
                        .with_file_name(".yinz.yinz-update.json")
                },
            ))
            .unwrap();
            assert_eq!(receipt["operation"], operation);
            assert_eq!(receipt["status"], "committed");
            for key in ["binary_root", "data_root"] {
                let path = receipt[key].as_str().unwrap();
                if remote_host {
                    remote(&format!("test ! -e {}", quote(path)));
                } else {
                    assert!(!Path::new(path).exists());
                }
            }
        }
        assert!(!fixture
            .local
            .data
            .join("plugins/.yinz-update-active.json")
            .exists());
        if far && stage == 0 {
            assert_eq!(
                inventories[0].running_version,
                yinz_runtime::consts::VERSION
            );
            fixture.local.success(&[
                "--session",
                SESSION,
                "plugins",
                "start",
                "yinz:session-palette",
                "--json",
            ]);
            let before_reload = observe(
                "old local server loads its retained builtin with its retained worker",
                || {
                    fixture
                        .builtin_observation()
                        .filter(|log| log.contains("SEQUENTIAL_BUILTIN_OLD"))
                },
            );
            assert!(!before_reload.contains("SEQUENTIAL_BUILTIN_NEW"));
            fixture.local.success(&[
                "--session",
                SESSION,
                "plugins",
                "reload",
                "yinz:session-palette",
                "--json",
            ]);
            let reloaded = observe("retained builtin reload", || {
                fixture.builtin_observation().filter(|log| {
                    log.matches("SEQUENTIAL_BUILTIN_OLD").count()
                        > before_reload.matches("SEQUENTIAL_BUILTIN_OLD").count()
                })
            });
            assert!(!reloaded.contains("SEQUENTIAL_BUILTIN_NEW"));
            echo(&mut clients[0], false, "AFTER-RETAINED-BUILTIN");
        }
        if !far && remote_first {
            observe(
                "replacement local server activates the published builtin generation",
                || {
                    fixture
                        .builtin_observation()
                        .filter(|log| log.contains("SEQUENTIAL_BUILTIN_NEW"))
                },
            );
        }
    }
    // Reattach both hosts through the same name. Fresh SSH bootstrapping must
    // reach the migrated remote session, not the same-named local session.
    for client in &mut clients {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(client.child.id() as i32),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        assert!(client.child.wait().unwrap().success());
    }
    drop(clients);
    assert!(forwarding_paths(&fixture.local).is_empty());
    for far in [false, true] {
        let mut client = fixture.attach(far, &format!("AFTER-1-{}", usize::from(far)));
        echo(&mut client, far, "REATTACHED");
        fixture.alive(far, pids[usize::from(far)]);
        assert_eq!(
            fixture.peer(far).0.inventory(&operation).server_instance,
            inventories[usize::from(far)].server_instance
        );
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(client.child.id() as i32),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        assert!(client.child.wait().unwrap().success());
        assert_eq!(
            nix::sys::termios::tcgetattr(client.slave.as_ref().unwrap()).unwrap(),
            client.original
        );
        assert!(forwarding_paths(&fixture.local).is_empty());
    }
}
